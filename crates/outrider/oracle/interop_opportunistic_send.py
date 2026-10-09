"""Pinned stock LXMF sends one opportunistic message to Outrider, then resends it.

Passes only when stock itself records both sends as DELIVERED, which needs Outrider's
proofs, and Outrider reports the resend as a duplicate rather than a second message.

`OUTRIDER_SILENT_SENDER=1` keeps the sender from announcing, so its first copy arrives
unverifiable and must stay unproved until a retry verifies. `OUTRIDER_NO_RATCHETS=1` has
Outrider advertise no ratchet, so stock encrypts to its identity key.
"""

from __future__ import annotations

import atexit
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import LXMF
import RNS


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
TITLE = b"STOCK OPPORTUNISTIC TITLE"
CONTENT = b"STOCK OPPORTUNISTIC BODY"
TIMESTAMP = 1_753_603_209.5
SENDER_SEED = bytes([0x77]) * 64
DELIVERY_DEADLINE = 30
SILENT_SENDER = os.environ.get("OUTRIDER_SILENT_SENDER") == "1"
NO_RATCHETS = os.environ.get("OUTRIDER_NO_RATCHETS") == "1"


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_OPPORTUNISTIC_RECEIVE")
    command = (
        [executable]
        if executable
        else [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "outrider",
            "--example",
            "stock_opportunistic_receive",
        ]
    )
    process = subprocess.Popen(
        command,
        cwd=REPO,
        env={**os.environ, "OUTRIDER_EXPECT_DUPLICATES": "1"},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert process.stdout is not None
        for raw in process.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [outrider] {line}")

    threading.Thread(target=pump, daemon=True).start()
    port = None
    destination = None
    deadline = time.time() + 180
    while time.time() < deadline and (port is None or destination is None):
        for line in list(lines):
            port_match = re.fullmatch(r"LISTENING (\d+)", line)
            if port_match:
                port = int(port_match.group(1))
            destination_match = re.fullmatch(r"DESTINATION ([0-9a-f]{32})", line)
            if destination_match:
                destination = bytes.fromhex(destination_match.group(1))
        if process.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None or destination is None:
        process.kill()
        return 1

    config = Path(tempfile.mkdtemp(prefix="outrider-opportunistic-send-"))
    (config / "config").write_text(
        "[reticulum]\n"
        "  enable_transport=No\n"
        "  share_instance=No\n"
        "  panic_on_interface_error=No\n"
        "\n[logging]\n"
        "  loglevel=5\n"
        "\n[interfaces]\n"
        "  [[outrider]]\n"
        "    type=TCPClientInterface\n"
        "    enabled=yes\n"
        "    target_host=127.0.0.1\n"
        f"    target_port={port}\n",
        encoding="utf-8",
    )

    exit_code = 1
    RNS.Reticulum(configdir=str(config))
    try:
        sender_identity = RNS.Identity.from_bytes(SENDER_SEED)
        router = LXMF.LXMRouter(identity=sender_identity, storagepath=str(config))
        source = router.register_delivery_identity(
            sender_identity,
            display_name="Stock Opportunistic Sender",
            stamp_cost=None,
        )
        if not SILENT_SENDER:
            source.announce()
        sent = threading.Event()
        messages: list[LXMF.LXMessage] = []

        def send(announced_identity) -> None:
            outbound = RNS.Destination(
                announced_identity,
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "lxmf",
                "delivery",
            )
            message = LXMF.LXMessage(
                outbound,
                source,
                CONTENT,
                title=TITLE,
                desired_method=LXMF.LXMessage.OPPORTUNISTIC,
            )
            message.timestamp = TIMESTAMP
            router.handle_outbound(message)
            messages.append(message)
            print(f"  stock: queued {message.hash.hex()} opportunistically")

        def delivered(message: LXMF.LXMessage) -> bool:
            deadline = time.time() + DELIVERY_DEADLINE
            while time.time() < deadline:
                if message.state == LXMF.LXMessage.DELIVERED:
                    return True
                time.sleep(0.1)
            print(f"  stock: state {message.state} after {DELIVERY_DEADLINE}s")
            return False

        receiver_identity = []

        class DeliveryAnnounce:
            aspect_filter = "lxmf.delivery"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                if destination_hash != destination or sent.is_set():
                    return
                receiver_identity.append(announced_identity)
                sent.set()

        RNS.Transport.register_announce_handler(DeliveryAnnounce())

        first_delivered = second_delivered = False
        if sent.wait(timeout=90):
            send(receiver_identity[0])
            first_delivered = delivered(messages[0])
            # The same message again: stock re-encrypts it, so only its id repeats.
            send(receiver_identity[0])
            second_delivered = delivered(messages[1])
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        message_hash = bytes(messages[0].hash) if messages else None
        captured = [bytes.fromhex(line[11:]) for line in lines if line.startswith("MESSAGE_ID ")]
        duplicates = [bytes.fromhex(line[10:]) for line in lines if line.startswith("DUPLICATE ")]
        decoded = f"TITLE {TITLE.hex()}" in lines and f"CONTENT {CONTENT.hex()}" in lines
        once = captured == [message_hash] and duplicates == [message_hash]
        unverified_first = not SILENT_SENDER or (
            message_hash is not None and f"UNVERIFIED {message_hash.hex()}" in lines
        )
        ratchet_ok = ("USED_RATCHET none" in lines) == NO_RATCHETS and any(
            line.startswith("USED_RATCHET ") for line in lines
        )
        ok = (
            len(messages) == 2
            and messages[1].hash == messages[0].hash
            and decoded
            and once
            and first_delivered
            and second_delivered
            and "SIGNATURE_VERIFIED true" in lines
            and "STAMP_POLICY none" in lines
            and unverified_first
            and ratchet_ok
        )
        print(f"stock queued opportunistically: {'PASS' if sent.is_set() else 'FAIL'}")
        print(f"Outrider decoded title/body: {'PASS' if decoded else 'FAIL'}")
        print(f"Outrider verified signature: {'PASS' if 'SIGNATURE_VERIFIED true' in lines else 'FAIL'}")
        print(f"stock message DELIVERED: {'PASS' if first_delivered else 'FAIL'}")
        print(f"stock resend DELIVERED: {'PASS' if second_delivered else 'FAIL'}")
        print(f"one MESSAGE_ID, resend a DUPLICATE: {'PASS' if once else 'FAIL'}")
        if SILENT_SENDER:
            print(f"unknown sender held unproved first: {'PASS' if unverified_first else 'FAIL'}")
        print(f"ratchet {'absent' if NO_RATCHETS else 'used'}: {'PASS' if ratchet_ok else 'FAIL'}")
        print(f"STOCK_TO_OUTRIDER_OPPORTUNISTIC: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
