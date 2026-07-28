"""Outrider sends one opportunistic message to pinned stock LXMF."""

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
TITLE = b"OUTRIDER OPPORTUNISTIC TITLE"
CONTENT = b"OUTRIDER OPPORTUNISTIC BODY"
RECEIVER_SEED = bytes([0x44]) * 64


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_OPPORTUNISTIC_SEND")
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
            "stock_opportunistic_send",
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
    deadline = time.time() + 180
    while time.time() < deadline and port is None:
        for line in list(lines):
            match = re.fullmatch(r"LISTENING (\d+)", line)
            if match:
                port = int(match.group(1))
        if process.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None:
        process.kill()
        return 1

    config = Path(tempfile.mkdtemp(prefix="outrider-opportunistic-receive-"))
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
    received: dict[str, bytes] = {}
    complete = threading.Event()
    try:
        receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        router = LXMF.LXMRouter(identity=receiver_identity, storagepath=str(config))
        destination = router.register_delivery_identity(
            receiver_identity,
            display_name="Stock Opportunistic Receiver",
            stamp_cost=None,
        )

        def on_delivery(message) -> None:
            received["title"] = bytes(message.title)
            received["content"] = bytes(message.content)
            received["hash"] = bytes(message.hash)
            print(f"  stock: received {message.hash.hex()}")
            complete.set()

        router.register_delivery_callback(on_delivery)
        for _ in range(3):
            destination.announce()
            if complete.wait(timeout=2):
                break

        complete.wait(timeout=45)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        sent_id = next(
            (
                bytes.fromhex(line[11:])
                for line in lines
                if line.startswith("MESSAGE_ID ")
            ),
            None,
        )
        ok = (
            complete.is_set()
            and received.get("title") == TITLE
            and received.get("content") == CONTENT
            and received.get("hash") == sent_id
            and any(line.startswith("RATCHET ") for line in lines)
            and "QUEUED 1" in lines
        )
        print(f"stock delivery callback fired: {'PASS' if complete.is_set() else 'FAIL'}")
        print(f"stock decoded title/body: {'PASS' if received.get('title') == TITLE and received.get('content') == CONTENT else 'FAIL'}")
        print(f"stock agreed on message id: {'PASS' if received.get('hash') == sent_id else 'FAIL'}")
        print(f"OUTRIDER_TO_STOCK_OPPORTUNISTIC: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
