"""Outrider sends to a stock receiver that announces a stamp cost but no name.

Stock registers the destination with display_name=None and stamp_cost=8 and announces it
through its router, so the app data is [nil, 8, [0]]. Outrider must read the cost from that
announce and stamp to it. The acceptance result is the stock router's own verdict: the
message delivered with a valid, checked stamp under enforce_stamps.
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
import RNS.vendor.umsgpack as msgpack


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
RECEIVER_SEED = bytes([0x46] * 64)


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_DIRECT_SEND")
    command = [executable] if executable else [
        "cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_direct_send"]
    process = subprocess.Popen(command, cwd=REPO, stdout=subprocess.PIPE,
                               stderr=subprocess.STDOUT, text=True, bufsize=1)
    lines: list[str] = []

    def pump() -> None:
        assert process.stdout is not None
        for raw in process.stdout:
            lines.append(raw.rstrip())
            print(f"  [outrider] {raw.rstrip()}")

    threading.Thread(target=pump, daemon=True).start()
    port = None
    deadline = time.time() + 180
    while time.time() < deadline and port is None:
        port = next((int(m.group(1)) for line in list(lines)
                     if (m := re.fullmatch(r"LISTENING (\d+)", line))), None)
        if process.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None:
        process.kill()
        return 1

    config = Path(tempfile.mkdtemp(prefix="outrider-nameless-receiver-"))
    (config / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=5\n\n[interfaces]\n"
        "  [[outrider]]\n    type=TCPClientInterface\n    enabled=yes\n"
        f"    target_host=127.0.0.1\n    target_port={port}\n",
        encoding="utf-8",
    )

    exit_code = 1
    RNS.Reticulum(configdir=str(config))
    received: dict[str, object] = {}
    complete = threading.Event()
    try:
        identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        router = LXMF.LXMRouter(identity=identity, storagepath=str(config), enforce_stamps=True)
        destination = router.register_delivery_identity(identity, display_name=None, stamp_cost=8)

        def on_delivery(message) -> None:
            received.update(hash=bytes(message.hash), stamp_valid=message.stamp_valid,
                            stamp_checked=message.stamp_checked)
            print(f"  stock: received {message.hash.hex()} stamp_valid={message.stamp_valid}")
            complete.set()

        router.register_delivery_callback(on_delivery)
        app_data = router.get_announce_app_data(destination.hash)
        router.announce(destination.hash)

        complete.wait(timeout=60)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        shape_ok = msgpack.unpackb(app_data)[:2] == [None, 8]
        sent_id = next((bytes.fromhex(line[11:]) for line in lines
                        if line.startswith("MESSAGE_ID ")), None)
        id_ok = sent_id is not None and received.get("hash") == sent_id
        stamp_ok = received.get("stamp_valid") is True and received.get("stamp_checked") is True
        ok = shape_ok and complete.is_set() and id_ok and stamp_ok
        print(f"stock announced [nil, 8, ...]: {'PASS' if shape_ok else 'FAIL'}")
        print(f"stock delivered the message: {'PASS' if complete.is_set() and id_ok else 'FAIL'}")
        print(f"stock checked a valid stamp: {'PASS' if stamp_ok else 'FAIL'}")
        print(f"OUTRIDER_TO_NAMELESS_STOCK: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
