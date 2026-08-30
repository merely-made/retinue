"""Prove pinned stock LXMF submits to and fetches from Outrider's server."""

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
TITLE = b"PROPAGATION TITLE"
CONTENT = (
    bytes((value * 73 + 19) & 0xFF for value in range(4096))
    if os.environ.get("OUTRIDER_LARGE") == "1"
    else b"PROPAGATION BODY"
)
TIMESTAMP = 1_753_603_204.5
SENDER_SEED = bytes([0x61] * 64)
RECEIVER_SEED = bytes([0x62] * 64)


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    executable = os.environ.get("OUTRIDER_PROPAGATION_SERVER")
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
            "stock_propagation_server",
        ]
    )
    server = subprocess.Popen(
        command,
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert server.stdout is not None
        for raw in server.stdout:
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
        if server.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None or node_destination is None:
        server.kill()
        return 1

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-server-"))
    rns_config = root / "rns"
    sender_store = root / "sender-store"
    receiver_store = root / "receiver-store"
    for directory in (rns_config, sender_store, receiver_store):
        directory.mkdir()
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
        node_seen = threading.Event()
        delivered: dict[str, object] = {}
        received = threading.Event()

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                if destination_hash != node_destination or node_seen.is_set():
                    return
                sender_router.set_outbound_propagation_node(bytes(destination_hash))
                receiver_router.set_outbound_propagation_node(bytes(destination_hash))
                node_seen.set()

        def on_delivery(message) -> None:
            delivered["title"] = bytes(message.title)
            delivered["content"] = bytes(message.content)
            delivered["hash"] = bytes(message.hash)
            received.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        receiver_router.register_delivery_callback(on_delivery)
        if not node_seen.wait(timeout=45):
            print("stock learned Outrider node: FAIL")
            return 1
        source.announce()

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
        submit_deadline = time.time() + (180 if len(CONTENT) > 1024 else 60)
        while time.time() < submit_deadline:
            if any(line.startswith("SERVER_STORED ") for line in lines):
                break
            time.sleep(0.2)
        submitted = any(line.startswith("SERVER_STORED ") for line in lines)

        receiver_router.request_messages_from_propagation_node(receiver_identity)
        received.wait(timeout=120 if len(CONTENT) > 1024 else 60)
        try:
            server.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server.kill()

        title_ok = delivered.get("title") == TITLE
        content_ok = delivered.get("content") == CONTENT
        id_ok = delivered.get("hash") == bytes(message.hash)
        stored = any(
            line.startswith("SERVER_STORED inserted=1 rejected=0") for line in lines
        )
        served = any(
            line.startswith("SERVER_SERVED offered=1 served=1") for line in lines
        )
        print(f"stock submitted to Outrider: {'PASS' if submitted and stored else 'FAIL'}")
        print(f"stock fetched from Outrider: {'PASS' if received.is_set() and served else 'FAIL'}")
        print(f"stock decoded title/body/id: {'PASS' if title_ok and content_ok and id_ok else 'FAIL'}")
        ok = submitted and stored and received.is_set() and served and title_ok and content_ok and id_ok
        print(f"OUTRIDER_PROPAGATION_SERVER: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        server.terminate()
        try:
            server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            server.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
