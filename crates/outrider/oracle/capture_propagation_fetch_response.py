"""Prove Outrider fetches a propagated message from stock LXMF 0.9.6."""

from __future__ import annotations

import atexit
import os
import shutil
import socket
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


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    port = free_port()
    transport_executable = os.environ.get("RETINUE_TRANSPORT_NODE")
    transport_command = (
        [transport_executable]
        if transport_executable
        else ["cargo", "run", "--quiet", "-p", "retinue", "--example", "transport_node"]
    )
    transport = subprocess.Popen(
        transport_command,
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

    fetch_executable = os.environ.get("OUTRIDER_PROPAGATION_FETCH_RECEIVE")
    fetch_command = (
        [fetch_executable]
        if fetch_executable
        else [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "outrider",
            "--example",
            "stock_propagation_fetch_receive",
        ]
    )
    fetcher = subprocess.Popen(
        fetch_command,
        cwd=REPO,
        env={**os.environ, "RETINUE_ADDR": f"127.0.0.1:{port}"},
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    fetch_lines: list[str] = []

    def pump_fetcher() -> None:
        assert fetcher.stdout is not None
        for raw in fetcher.stdout:
            line = raw.rstrip()
            fetch_lines.append(line)
            print(f"  [outrider] {line}")

    threading.Thread(target=pump_fetcher, daemon=True).start()

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-fetch-response-"))
    client_rns = root / "client-rns"
    sender_store = root / "sender-store"
    node_rns = root / "node-rns"
    node_config = root / "node"
    for directory in (client_rns, sender_store, node_rns, node_config):
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

    daemon = None
    exit_code = 1
    RNS.Reticulum(configdir=str(client_rns))
    try:
        sender_identity = RNS.Identity.from_bytes(SENDER_SEED)
        receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        sender_router = LXMF.LXMRouter(identity=sender_identity, storagepath=str(sender_store))
        source = sender_router.register_delivery_identity(
            sender_identity, display_name="Propagation Sender", stamp_cost=None
        )
        node_seen = threading.Event()
        state: dict[str, bytes] = {}

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                if node_seen.is_set():
                    return
                state["node"] = bytes(destination_hash)
                sender_router.set_outbound_propagation_node(bytes(destination_hash))
                node_seen.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        daemon = subprocess.Popen(
            [
                str(Path(os.environ.get("LXMD", Path(os.sys.executable).parent / "lxmd.exe"))),
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
        ready_deadline = time.time() + 45
        while time.time() < ready_deadline and not any(
            line.startswith("FETCH_READY ") for line in fetch_lines
        ):
            if fetcher.poll() is not None:
                return 1
            time.sleep(0.1)
        for _ in range(3):
            source.announce()
            time.sleep(0.3)

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
        submit_deadline = time.time() + 180
        while time.time() < submit_deadline:
            if message.state in (LXMF.LXMessage.SENT, LXMF.LXMessage.FAILED):
                break
            time.sleep(0.2)
        submitted = message.state == LXMF.LXMessage.SENT
        assert fetcher.stdin is not None
        fetcher.stdin.write("fetch\n")
        fetcher.stdin.flush()

        response_deadline = time.time() + (180 if len(CONTENT) > 1024 else 60)
        while time.time() < response_deadline:
            if any(line.startswith("PRODUCTION_FETCH ") for line in fetch_lines):
                break
            if fetcher.poll() is not None:
                break
            time.sleep(0.2)
        fetched_id = next(
            (
                bytes.fromhex(line[11:])
                for line in fetch_lines
                if line.startswith("MESSAGE_ID ")
            ),
            None,
        )
        fetched_title = next(
            (
                bytes.fromhex(line[6:])
                for line in fetch_lines
                if line.startswith("TITLE ")
            ),
            None,
        )
        fetched_content = next(
            (
                bytes.fromhex(line[8:])
                for line in fetch_lines
                if line.startswith("CONTENT ")
            ),
            None,
        )
        fetched = "PRODUCTION_FETCH true" in fetch_lines
        decoded = (
            fetched_id == bytes(message.hash)
            and fetched_title == TITLE
            and fetched_content == CONTENT
        )
        print(f"stock submitted fixture message: {'PASS' if submitted else 'FAIL'}")
        print(f"Outrider fetched stock message: {'PASS' if fetched else 'FAIL'}")
        print(f"Outrider decoded title/body/id: {'PASS' if decoded else 'FAIL'}")
        ok = submitted and fetched and decoded
        print(f"OUTRIDER_FETCH_FROM_STOCK: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        for process in (daemon, fetcher, transport):
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
