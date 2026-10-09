"""Pinned stock LXMF sends one direct message to Outrider over Retinue, then resends it.

Passes only when stock itself records both sends as DELIVERED, which needs Outrider's
proofs, and Outrider reports the resend as a duplicate rather than a second message.

This is an external-oracle driver, not an implementation input. It uses only
the public LXMF and RNS APIs, while the Rust example captures and verifies the
wire object independently.
"""

from __future__ import annotations

import atexit
import hashlib
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
LARGE = os.environ.get("OUTRIDER_LARGE") == "1"
TITLE = b"STOCK LARGE TITLE" if LARGE else b"STOCK TITLE"
CONTENT = (
    b"".join(hashlib.sha256(i.to_bytes(4, "big")).digest() for i in range(128))
    if LARGE
    else b"STOCK BODY"
)
TIMESTAMP = 1_753_603_201.5
SENDER_SEED = bytes([0x77] * 64)
DELIVERY_DEADLINE = 30


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_DIRECT_RECEIVE")
    command = (
        [executable]
        if executable
        else ["cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_direct_receive"]
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

    config = Path(tempfile.mkdtemp(prefix="outrider-direct-send-"))
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
        display_name = (
            None
            if os.environ.get("OUTRIDER_ANONYMOUS_ANNOUNCE") == "1"
            else "Stock Oracle"
        )
        source = router.register_delivery_identity(
            sender_identity, display_name=display_name, stamp_cost=8
        )
        source.announce()

        sent = threading.Event()
        receiver_identity = []
        messages: list[LXMF.LXMessage] = []

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

        def send() -> None:
            outbound = RNS.Destination(
                receiver_identity[0],
                RNS.Destination.OUT,
                RNS.Destination.SINGLE,
                "lxmf",
                "delivery",
            )
            message = LXMF.LXMessage(outbound, source, CONTENT, title=TITLE)
            message.timestamp = TIMESTAMP
            router.handle_outbound(message)
            messages.append(message)
            print(f"  stock: queued {message.hash.hex()}")

        def delivered(message: LXMF.LXMessage) -> bool:
            deadline = time.time() + DELIVERY_DEADLINE
            while time.time() < deadline:
                if message.state == LXMF.LXMessage.DELIVERED:
                    return True
                time.sleep(0.1)
            print(f"  stock: state {message.state} after {DELIVERY_DEADLINE}s")
            return False

        first_delivered = second_delivered = False
        if sent.wait(timeout=60):
            send()
            first_delivered = delivered(messages[0])
            # The same message again, as a stock retry would carry it.
            send()
            second_delivered = delivered(messages[1])

        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        message_hash = bytes(messages[0].hash) if messages else None
        captured = [bytes.fromhex(line[11:]) for line in lines if line.startswith("MESSAGE_ID ")]
        duplicates = [bytes.fromhex(line[10:]) for line in lines if line.startswith("DUPLICATE ")]
        once = (
            len(messages) == 2
            and messages[1].hash == messages[0].hash
            and captured == [message_hash]
            and duplicates == [message_hash]
        )
        title_ok = f"TITLE {TITLE.hex()}" in lines
        content_ok = f"CONTENT {CONTENT.hex()}" in lines
        signature_ok = "SIGNATURE_VERIFIED true" in lines
        packed = next((line[7:] for line in lines if line.startswith("PACKED ")), None)
        expected_transport = "resource" if LARGE else "data"
        transport_ok = f"TRANSPORT {expected_transport}" in lines
        stamp_ok = any(
            line.startswith("STAMP_VALUE ") and int(line[12:]) >= 8 for line in lines
        )
        ok = (
            sent.is_set()
            and title_ok
            and content_ok
            and signature_ok
            and packed is not None
            and transport_ok
            and stamp_ok
            and first_delivered
            and second_delivered
            and once
        )
        print(f"stock queued direct message: {'PASS' if sent.is_set() else 'FAIL'}")
        print(f"Outrider decoded title/body: {'PASS' if title_ok and content_ok else 'FAIL'}")
        print(f"Outrider verified signature: {'PASS' if signature_ok else 'FAIL'}")
        print(f"captured complete wire object: {'PASS' if packed is not None else 'FAIL'}")
        print(f"stock chose {expected_transport}: {'PASS' if transport_ok else 'FAIL'}")
        print(f"Outrider reported the stamp value: {'PASS' if stamp_ok else 'FAIL'}")
        print(f"stock message DELIVERED: {'PASS' if first_delivered else 'FAIL'}")
        print(f"stock resend DELIVERED: {'PASS' if second_delivered else 'FAIL'}")
        print(f"one MESSAGE_ID, resend a DUPLICATE: {'PASS' if once else 'FAIL'}")
        print(f"STOCK_TO_OUTRIDER_DIRECT: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
