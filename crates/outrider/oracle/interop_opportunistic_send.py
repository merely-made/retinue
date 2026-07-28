"""Pinned stock LXMF sends one stamped opportunistic message to Outrider."""

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
        source.announce()
        sent = threading.Event()
        message_hash = None

        class DeliveryAnnounce:
            aspect_filter = "lxmf.delivery"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                nonlocal message_hash
                if destination_hash != destination or sent.is_set():
                    return
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
                sent.set()
                router.handle_outbound(message)
                message_hash = bytes(message.hash)
                print(f"  stock: queued {message.hash.hex()} opportunistically")

        RNS.Transport.register_announce_handler(DeliveryAnnounce())

        deadline = time.time() + 90
        while time.time() < deadline:
            if process.poll() is not None:
                break
            if "SIGNATURE_VERIFIED true" in lines:
                break
            time.sleep(0.1)
        try:
            process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            process.kill()

        captured_id = next(
            (
                bytes.fromhex(line[11:])
                for line in lines
                if line.startswith("MESSAGE_ID ")
            ),
            None,
        )
        ok = (
            sent.is_set()
            and f"TITLE {TITLE.hex()}" in lines
            and f"CONTENT {CONTENT.hex()}" in lines
            and captured_id == message_hash
            and "SIGNATURE_VERIFIED true" in lines
            and "STAMP_POLICY none" in lines
            and any(line.startswith("USED_RATCHET ") for line in lines)
        )
        print(f"stock queued opportunistically: {'PASS' if sent.is_set() else 'FAIL'}")
        print(f"Outrider decoded title/body: {'PASS' if f'TITLE {TITLE.hex()}' in lines and f'CONTENT {CONTENT.hex()}' in lines else 'FAIL'}")
        print(f"Outrider verified signature: {'PASS' if 'SIGNATURE_VERIFIED true' in lines else 'FAIL'}")
        print(f"STOCK_TO_OUTRIDER_OPPORTUNISTIC: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
