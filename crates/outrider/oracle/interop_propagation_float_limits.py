"""Outrider submits through an lxmd node that announces float limits.

lxmd reads its size limits as floats, so a configured node announces e.g. 256.5 KB rather
than an integer. Outrider must decode that announce, honour the limit and submit. The
acceptance result is stock state: the node's announce really carries floats, its message
store holds the submission, a stock recipient fetches it, and after the fetch the node's
store is empty again and a second stock fetch returns nothing.
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
import RNS.vendor.umsgpack as msgpack


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LXMD = Path(sys.executable).parent / ("lxmd.exe" if os.name == "nt" else "lxmd")
RECEIVER_SEED = bytes([0x62] * 64)
TRANSFER_KB = 256.5
SYNC_KB = 10240.25


def wait_for(condition, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if condition():
            return True
        time.sleep(0.2)
    return condition()


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_PROPAGATION_SEND")
    command = [executable] if executable else [
        "cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_propagation_send"]
    sender = subprocess.Popen(command, cwd=REPO, env={**os.environ, "OUTRIDER_SUMMARY": "1"},
                              stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)
    lines: list[str] = []

    def pump_sender() -> None:
        assert sender.stdout is not None
        for raw in sender.stdout:
            lines.append(raw.rstrip())
            print(f"  [outrider] {raw.rstrip()}")

    threading.Thread(target=pump_sender, daemon=True).start()
    port = None
    deadline = time.time() + 180
    while time.time() < deadline and port is None:
        port = next((int(m.group(1)) for line in list(lines)
                     if (m := re.fullmatch(r"LISTENING (\d+)", line))), None)
        if sender.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None:
        sender.kill()
        return 1

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-float-"))
    client_rns, receiver_store, node_rns, node_config = (
        root / "client-rns", root / "receiver-store", root / "node-rns", root / "node")
    for directory in (client_rns, receiver_store, node_rns, node_config):
        directory.mkdir()
    interface_config = (
        "[reticulum]\nenable_transport=No\nshare_instance=No\npanic_on_interface_error=No\n"
        "\n[logging]\nloglevel=5\n\n[interfaces]\n[[outrider]]\ntype=TCPClientInterface\n"
        f"enabled=yes\ntarget_host=127.0.0.1\ntarget_port={port}\n")
    (client_rns / "config").write_text(interface_config, encoding="utf-8")
    (node_rns / "config").write_text(interface_config, encoding="utf-8")
    # lxmd.py 170-177 lets the message size override the transfer size, so set both.
    (node_config / "config").write_text(
        "[propagation]\nenable_node=yes\nnode_name=Stock Float Oracle\nannounce_at_start=yes\n"
        "autopeer=no\npropagation_stamp_cost_target=8\npeering_cost=8\n"
        f"propagation_transfer_max_accepted_size={TRANSFER_KB}\n"
        f"propagation_message_max_accepted_size={TRANSFER_KB}\n"
        f"propagation_sync_max_accepted_size={SYNC_KB}\n"
        "\n[lxmf]\ndisplay_name=Stock Delivery Oracle\nannounce_at_start=no\n"
        "\n[logging]\nloglevel=5\n",
        encoding="utf-8",
    )

    def stored() -> int:
        return sum(1 for store in node_config.rglob("messagestore")
                   for entry in store.iterdir() if entry.is_file())

    exit_code = 1
    daemon = None
    RNS.Reticulum(configdir=str(client_rns))
    try:
        identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        router = LXMF.LXMRouter(identity=identity, storagepath=str(receiver_store))
        delivery = router.register_delivery_identity(identity, display_name="Float Receiver", stamp_cost=None)
        node: dict[str, object] = {}
        node_seen = threading.Event()
        delivered: dict[str, bytes] = {}
        received = threading.Event()

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(self, destination_hash, announced_identity, app_data) -> None:
                if node_seen.is_set():
                    return
                node["app_data"] = bytes(app_data or b"")
                router.set_outbound_propagation_node(bytes(destination_hash))
                node_seen.set()

        def on_delivery(message) -> None:
            delivered["hash"] = bytes(message.hash)
            received.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        router.register_delivery_callback(on_delivery)
        daemon = subprocess.Popen(
            [str(LXMD), "-p", "--config", str(node_config), "--rnsconfig", str(node_rns), "--verbose"],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1)

        def pump_daemon() -> None:
            assert daemon is not None and daemon.stdout is not None
            for raw in daemon.stdout:
                print(f"  [lxmd] {raw.rstrip()}")

        threading.Thread(target=pump_daemon, daemon=True).start()
        if not node_seen.wait(timeout=45):
            print("stock learned propagation node: FAIL")
            return 1
        announced = msgpack.unpackb(node["app_data"])
        floats_ok = announced[3] == TRANSFER_KB and announced[4] == SYNC_KB and all(
            isinstance(announced[i], float) for i in (3, 4))

        # Outrider seals to the recipient's announced ratchet, so it waits for that announce.
        submit_deadline = time.time() + 180
        while time.time() < submit_deadline and sender.poll() is None:
            if any(line.startswith("SUBMITTED ") for line in lines):
                break
            if not any(line.startswith("STOCK_RECIPIENT_ANNOUNCE ") for line in lines):
                router.announce(delivery.hash)
                time.sleep(1)
            time.sleep(0.2)
        submitted = "SUBMITTED true" in lines
        stored_ok = wait_for(lambda: stored() == 1, 30)

        def fetch() -> int | None:
            router.request_messages_from_propagation_node(identity)
            wait_for(lambda: router.propagation_transfer_state == LXMF.LXMRouter.PR_COMPLETE, 60)
            result = router.propagation_transfer_last_result
            router.acknowledge_sync_completion()
            return result

        first = fetch()
        received.wait(timeout=10)
        message_id = next((bytes.fromhex(line[11:]) for line in lines
                           if line.startswith("MESSAGE_ID ")), None)
        fetched_ok = first == 1 and message_id is not None and delivered.get("hash") == message_id
        drained_ok = wait_for(lambda: stored() == 0, 30)
        second = fetch()
        print(f"stock node announced float limits: {'PASS' if floats_ok else 'FAIL'}")
        print(f"Outrider decoded and submitted: {'PASS' if submitted else 'FAIL'}")
        print(f"stock node stored the submission: {'PASS' if stored_ok else 'FAIL'}")
        print(f"stock recipient fetched it: {'PASS' if fetched_ok else 'FAIL'}")
        print(f"stock node store drained: {'PASS' if drained_ok else 'FAIL'}")
        print(f"second stock fetch found nothing: {'PASS' if second == 0 else 'FAIL'} ({second!r})")
        ok = floats_ok and submitted and stored_ok and fetched_ok and drained_ok and second == 0
        print(f"OUTRIDER_VIA_FLOAT_LIMIT_NODE: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        for process in (daemon, sender):
            if process is None:
                continue
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
