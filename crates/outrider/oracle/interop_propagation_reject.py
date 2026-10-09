"""Outrider submits to stock lxmd under a stamp below its floor, then properly.

lxmd must answer the first with ERROR_INVALID_STAMP, which Outrider reports as
Rejected, and prove the second. The stock side is then judged directly: a stock
fetch from lxmd returns the accepted message and never the rejected one, and a
second fetch finds nothing left.
"""

from __future__ import annotations

import atexit
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

import LXMF
import RNS


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LXMD = Path(sys.executable).parent / ("lxmd.exe" if os.name == "nt" else "lxmd")
RECEIVER_SEED = bytes([0x62] * 64)


def fetch(router: LXMF.LXMRouter, identity: RNS.Identity) -> int | None:
    """Run one stock fetch from the outbound propagation node; its message count."""
    router.request_messages_from_propagation_node(identity)
    deadline = time.time() + 60
    while time.time() < deadline:
        if router.propagation_transfer_state == LXMF.LXMRouter.PR_COMPLETE:
            return router.propagation_transfer_last_result
        time.sleep(0.2)
    return None


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_PROPAGATION_REJECT")
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
            "stock_propagation_reject",
        ]
    )
    sender = subprocess.Popen(
        command,
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump_sender() -> None:
        assert sender.stdout is not None
        for raw in sender.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [outrider] {line}")

    threading.Thread(target=pump_sender, daemon=True).start()
    port = None
    deadline = time.time() + 180
    while time.time() < deadline and port is None:
        for line in list(lines):
            match = re.fullmatch(r"LISTENING (\d+)", line)
            if match:
                port = int(match.group(1))
        if sender.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None:
        sender.kill()
        return 1

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-reject-"))
    client_rns = root / "client-rns"
    receiver_store = root / "receiver-store"
    node_rns = root / "node-rns"
    node_config = root / "node"
    for directory in (client_rns, receiver_store, node_rns, node_config):
        directory.mkdir()
    interface_config = (
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
        f"target_port={port}\n"
    )
    (client_rns / "config").write_text(interface_config, encoding="utf-8")
    (node_rns / "config").write_text(interface_config, encoding="utf-8")
    (node_config / "config").write_text(
        "[propagation]\n"
        "enable_node=yes\n"
        "node_name=Stock Propagation Oracle\n"
        "announce_at_start=yes\n"
        "autopeer=no\n"
        "propagation_stamp_cost_target=8\n"
        "peering_cost=8\n"
        "\n[lxmf]\n"
        "display_name=Stock Delivery Oracle\n"
        "announce_at_start=no\n"
        "\n[logging]\n"
        "loglevel=5\n",
        encoding="utf-8",
    )

    exit_code = 1
    daemon = None
    RNS.Reticulum(configdir=str(client_rns))
    try:
        receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        receiver_router = LXMF.LXMRouter(
            identity=receiver_identity, storagepath=str(receiver_store)
        )
        receiver_router.register_delivery_identity(
            receiver_identity, display_name="Propagation Receiver", stamp_cost=None
        )
        node_seen = threading.Event()
        delivered: list[bytes] = []

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                if node_seen.is_set():
                    return
                receiver_router.set_outbound_propagation_node(bytes(destination_hash))
                print(f"  stock: learned propagation node {destination_hash.hex()}")
                node_seen.set()

        def on_delivery(message) -> None:
            delivered.append(bytes(message.hash))
            print(f"  stock: fetched {message.hash.hex()} {bytes(message.title)!r}")

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        receiver_router.register_delivery_callback(on_delivery)
        daemon = subprocess.Popen(
            [
                str(LXMD),
                "-p",
                "--config",
                str(node_config),
                "--rnsconfig",
                str(node_rns),
                "--verbose",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )

        def pump_daemon() -> None:
            assert daemon is not None and daemon.stdout is not None
            for raw in daemon.stdout:
                print(f"  [lxmd] {raw.rstrip()}")

        threading.Thread(target=pump_daemon, daemon=True).start()
        if not node_seen.wait(timeout=45):
            print("stock learned propagation node: FAIL")
            return 1

        submit_deadline = time.time() + 180
        while time.time() < submit_deadline:
            if any(re.match(r"(NOT_)?PROVED ", line) for line in lines):
                break
            if sender.poll() is not None:
                break
            time.sleep(0.2)

        def reported(tag: str) -> bytes | None:
            for line in lines:
                match = re.fullmatch(tag + r" ([0-9a-f]{64})( .*)?", line)
                if match:
                    return bytes.fromhex(match.group(1))
            return None

        rejected_id = reported("REJECTED")
        accepted_id = reported("PROVED")
        first = fetch(receiver_router, receiver_identity)
        time.sleep(1)
        second = fetch(receiver_router, receiver_identity)
        print(f"  stock: fetched {first!r} then {second!r} messages")

        rejected_ok = rejected_id is not None and rejected_id not in delivered
        accepted_ok = accepted_id is not None and delivered == [accepted_id]
        drained_ok = first == 1 and second == 0
        print(f"Outrider reported Rejected, stock never served it: {'PASS' if rejected_ok else 'FAIL'}")
        print(f"Outrider reported proved, stock fetched exactly it: {'PASS' if accepted_ok else 'FAIL'}")
        print(f"a second stock fetch found nothing: {'PASS' if drained_ok else 'FAIL'}")
        ok = rejected_ok and accepted_ok and drained_ok
        print(f"OUTRIDER_PROPAGATION_REJECTED: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        if daemon is not None:
            daemon.terminate()
            try:
                daemon.wait(timeout=5)
            except subprocess.TimeoutExpired:
                daemon.kill()
        sender.terminate()
        try:
            sender.wait(timeout=5)
        except subprocess.TimeoutExpired:
            sender.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
