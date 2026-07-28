"""Outrider sends one direct message to pinned stock LXMF.

The stock side is used strictly as a public-API oracle. Receipt through its
delivery callback is the acceptance result.
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
TITLE = b"OUTRIDER LARGE TITLE" if LARGE else b"OUTRIDER TITLE"
CONTENT = (
    b"".join(hashlib.sha256(i.to_bytes(4, "big")).digest() for i in range(128))
    if LARGE
    else b"OUTRIDER BODY"
)
RECEIVER_SEED = bytes([0x44] * 64)


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_DIRECT_SEND")
    command = (
        [executable]
        if executable
        else ["cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_direct_send"]
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

    config = Path(tempfile.mkdtemp(prefix="outrider-direct-receive-"))
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
    received: dict[str, object] = {}
    complete = threading.Event()
    try:
        receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        router = LXMF.LXMRouter(identity=receiver_identity, storagepath=str(config))
        destination = router.register_delivery_identity(
            receiver_identity, display_name="Stock Receiver", stamp_cost=8
        )

        def on_delivery(message) -> None:
            received["title"] = bytes(message.title)
            received["content"] = bytes(message.content)
            received["hash"] = bytes(message.hash)
            print(f"  stock: received {message.hash.hex()}")
            complete.set()

        router.register_delivery_callback(on_delivery)
        destination.announce()

        complete.wait(timeout=45)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        title_ok = received.get("title") == TITLE
        content_ok = received.get("content") == CONTENT
        sent_id = next(
            (
                bytes.fromhex(line[11:])
                for line in lines
                if line.startswith("MESSAGE_ID ")
            ),
            None,
        )
        id_ok = received.get("hash") == sent_id
        announce_seen = any(line.startswith("STOCK_ANNOUNCE ") for line in lines)
        expected_transport = "Resource" if LARGE else "Data"
        transport_ok = f"TRANSPORT {expected_transport}" in lines
        ok = (
            complete.is_set()
            and title_ok
            and content_ok
            and id_ok
            and announce_seen
            and transport_ok
        )
        print(f"stock delivery callback fired: {'PASS' if complete.is_set() else 'FAIL'}")
        print(f"stock decoded title/body: {'PASS' if title_ok and content_ok else 'FAIL'}")
        print(f"stock agreed on message id: {'PASS' if id_ok else 'FAIL'}")
        print(f"Outrider captured announce data: {'PASS' if announce_seen else 'FAIL'}")
        print(f"Outrider chose {expected_transport}: {'PASS' if transport_ok else 'FAIL'}")
        print(f"OUTRIDER_TO_STOCK_DIRECT: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
