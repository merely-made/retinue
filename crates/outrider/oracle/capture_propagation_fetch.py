"""Capture a stock LXMF 0.9.6 propagation fetch request through Retinue."""

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
RECEIVER_SEED = bytes([0x62] * 64)


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_PROPAGATION_FETCH_CAPTURE")
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
            "stock_propagation_fetch_request",
        ]
    )
    capture = subprocess.Popen(
        command,
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert capture.stdout is not None
        for raw in capture.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [outrider] {line}")

    threading.Thread(target=pump, daemon=True).start()
    port = None
    node_destination = None
    deadline = time.time() + 180
    while time.time() < deadline and (port is None or node_destination is None):
        for line in list(lines):
            port_match = re.fullmatch(r"LISTENING (\d+)", line)
            if port_match:
                port = int(port_match.group(1))
            destination_match = re.fullmatch(
                r"PROPAGATION_DESTINATION ([0-9a-f]{32})", line
            )
            if destination_match:
                node_destination = bytes.fromhex(destination_match.group(1))
        if capture.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None or node_destination is None:
        capture.kill()
        return 1

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-fetch-"))
    rns_config = root / "rns"
    store = root / "store"
    rns_config.mkdir()
    store.mkdir()
    (rns_config / "config").write_text(
        "[reticulum]\n"
        "enable_transport=No\n"
        "share_instance=No\n"
        "panic_on_interface_error=No\n"
        "\n[logging]\n"
        "loglevel=5\n"
        "\n[interfaces]\n"
        "[[outrider]]\n"
        "type=TCPClientInterface\n"
        "enabled=yes\n"
        "target_host=127.0.0.1\n"
        f"target_port={port}\n",
        encoding="utf-8",
    )

    exit_code = 1
    RNS.Reticulum(configdir=str(rns_config))
    try:
        receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        router = LXMF.LXMRouter(identity=receiver_identity, storagepath=str(store))
        router.register_delivery_identity(
            receiver_identity, display_name="Propagation Receiver", stamp_cost=None
        )
        node_seen = threading.Event()

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                if destination_hash != node_destination or node_seen.is_set():
                    return
                router.set_outbound_propagation_node(bytes(destination_hash))
                node_seen.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        if not node_seen.wait(timeout=45):
            print("stock learned capture node: FAIL")
            return 1
        maximum = os.environ.get("OUTRIDER_FETCH_MAX")
        requested = router.request_messages_from_propagation_node(
            receiver_identity, None if maximum is None else int(maximum)
        )
        print(f"  stock: fetch requested {requested!r}")

        result_deadline = time.time() + 60
        while time.time() < result_deadline:
            if any(line.startswith("FOLLOWUP_PACKED ") for line in lines):
                break
            if capture.poll() is not None:
                break
            time.sleep(0.2)
        try:
            capture.wait(timeout=10)
        except subprocess.TimeoutExpired:
            capture.kill()

        packed_hex = next(
            (line[15:] for line in lines if line.startswith("REQUEST_PACKED ")), None
        )
        if packed_hex is not None:
            request = msgpack.unpackb(bytes.fromhex(packed_hex))
            print(f"  stock: decoded request {request!r}")
        followup_hex = next(
            (
                line[16:]
                for line in lines
                if line.startswith("FOLLOWUP_PACKED ")
            ),
            None,
        )
        if followup_hex is not None:
            followup = msgpack.unpackb(bytes.fromhex(followup_hex))
            print(f"  stock: decoded followup {followup!r}")
        captured = packed_hex is not None and followup_hex is not None
        print(f"stock fetch request captured: {'PASS' if captured else 'FAIL'}")
        print(f"PROPAGATION_FETCH_CAPTURE: {'PASS' if captured else 'FAIL'}")
        exit_code = 0 if captured else 1
        return exit_code
    finally:
        capture.terminate()
        try:
            capture.wait(timeout=5)
        except subprocess.TimeoutExpired:
            capture.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
