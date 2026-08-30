"""Drive pinned stock LXMF through a stock propagation node over Retinue.

This establishes the public-API workflow before Outrider captures and
implements either side of the propagation session.
"""

from __future__ import annotations

import atexit
import os
import shutil
import socket
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
LXMD = Path(sys.executable).parent / "lxmd.exe"
TITLE = b"PROPAGATION TITLE"
CONTENT = b"PROPAGATION BODY"
TIMESTAMP = 1_753_603_204.5
SENDER_SEED = bytes([0x61] * 64)
RECEIVER_SEED = bytes([0x62] * 64)


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    port = free_port()
    transport = subprocess.Popen(
        ["cargo", "run", "--quiet", "-p", "retinue", "--example", "transport_node"],
        cwd=REPO,
        env={**os.environ, "RETINUE_PORT": str(port)},
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    transport_lines: list[str] = []

    def pump_transport() -> None:
        assert transport.stdout is not None
        for raw in transport.stdout:
            line = raw.rstrip()
            transport_lines.append(line)
            print(f"  [retinue] {line}")

    threading.Thread(target=pump_transport, daemon=True).start()
    deadline = time.time() + 180
    while time.time() < deadline and not any(
        line.startswith("TRANSPORT_NODE_UP ") for line in transport_lines
    ):
        if transport.poll() is not None:
            return 1
        time.sleep(0.1)

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-stock-"))
    client_rns = root / "client-rns"
    sender_store = root / "sender-store"
    receiver_store = root / "receiver-store"
    node_rns = root / "node-rns"
    node_config = root / "node"
    for directory in (client_rns, sender_store, receiver_store, node_rns, node_config):
        directory.mkdir()
    interface_config = (
        "[reticulum]\n"
        "enable_transport=No\n"
        "share_instance=No\n"
        "panic_on_interface_error=No\n"
        "\n[logging]\n"
        "loglevel=5\n"
        "\n[interfaces]\n"
        "[[retinue]]\n"
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

    RNS.Reticulum(configdir=str(client_rns))
    node_seen = threading.Event()
    state: dict[str, object] = {}

    class PropagationAnnounce:
        aspect_filter = "lxmf.propagation"

        def received_announce(
            self, destination_hash, announced_identity, app_data
        ) -> None:
            if node_seen.is_set():
                return
            state["node"] = bytes(destination_hash)
            state["node_identity"] = announced_identity
            state["node_app_data"] = bytes(app_data) if app_data is not None else b""
            print(f"  stock: learned propagation node {destination_hash.hex()}")
            node_seen.set()

    RNS.Transport.register_announce_handler(PropagationAnnounce())

    sender_identity = RNS.Identity.from_bytes(SENDER_SEED)
    receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
    sender_router = LXMF.LXMRouter(identity=sender_identity, storagepath=str(sender_store))
    source = sender_router.register_delivery_identity(
        sender_identity, display_name="Propagation Sender", stamp_cost=None
    )
    receiver_router = LXMF.LXMRouter(
        identity=receiver_identity, storagepath=str(receiver_store)
    )
    receiver_router.register_delivery_identity(
        receiver_identity, display_name="Propagation Receiver", stamp_cost=None
    )
    delivered: dict[str, object] = {}
    received = threading.Event()

    def on_delivery(message) -> None:
        delivered["title"] = bytes(message.title)
        delivered["content"] = bytes(message.content)
        delivered["hash"] = bytes(message.hash)
        print(f"  stock: fetched {message.hash.hex()}")
        received.set()

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
    daemon_lines: list[str] = []

    def pump_daemon() -> None:
        assert daemon.stdout is not None
        for raw in daemon.stdout:
            line = raw.rstrip()
            daemon_lines.append(line)
            print(f"  [lxmd] {line}")

    threading.Thread(target=pump_daemon, daemon=True).start()
    exit_code = 1
    try:
        if not node_seen.wait(timeout=45):
            print("stock learned propagation node: FAIL")
            return 1
        node_hash = state["node"]
        assert isinstance(node_hash, bytes)
        sender_router.set_outbound_propagation_node(node_hash)
        receiver_router.set_outbound_propagation_node(node_hash)
        print(
            "  stock: states "
            f"SENT={LXMF.LXMessage.SENT} "
            f"DELIVERED={LXMF.LXMessage.DELIVERED} "
            f"FAILED={LXMF.LXMessage.FAILED}"
        )

        destination = RNS.Destination(
            receiver_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "lxmf",
            "delivery",
        )
        message = LXMF.LXMessage(
            destination,
            source,
            CONTENT,
            title=TITLE,
            desired_method=LXMF.LXMessage.PROPAGATED,
        )
        message.timestamp = TIMESTAMP
        sender_router.handle_outbound(message)
        print(f"  stock: queued propagated {message.hash.hex()}")

        submit_deadline = time.time() + 180
        while time.time() < submit_deadline:
            if message.state in (LXMF.LXMessage.SENT, LXMF.LXMessage.FAILED):
                break
            time.sleep(0.2)
        submitted = message.state == LXMF.LXMessage.SENT
        print(f"  stock: submit state {message.state}")

        requested = receiver_router.request_messages_from_propagation_node(receiver_identity)
        print(f"  stock: fetch requested {requested!r}")
        received.wait(timeout=60)

        title_ok = delivered.get("title") == TITLE
        content_ok = delivered.get("content") == CONTENT
        id_ok = delivered.get("hash") == bytes(message.hash)
        print(f"stock learned propagation node: {'PASS' if node_seen.is_set() else 'FAIL'}")
        print(f"stock submitted message: {'PASS' if submitted else 'FAIL'}")
        print(f"stock fetched message: {'PASS' if received.is_set() else 'FAIL'}")
        print(f"stock decoded title/body: {'PASS' if title_ok and content_ok else 'FAIL'}")
        print(f"stock agreed on message id: {'PASS' if id_ok else 'FAIL'}")
        ok = node_seen.is_set() and submitted and received.is_set() and title_ok and content_ok and id_ok
        print(f"STOCK_PROPAGATION_ROUND_TRIP: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        daemon.terminate()
        transport.terminate()
        for process in (daemon, transport):
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
