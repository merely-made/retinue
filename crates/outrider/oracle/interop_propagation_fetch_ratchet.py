"""Prove Outrider fetches a ratcheted message from stock lxmd and clears it there.

A stock sender learns the ratchet the Outrider recipient announces, submits a
propagated message to stock lxmd, and Outrider fetches and decrypts it. The
stock side then shows the message was sealed to that ratchet, lxmd's message
store is empty, and a second stock list request as the recipient returns [].
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
from LXMF.LXMPeer import LXMPeer


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LXMD = Path(sys.executable).parent / ("lxmd.exe" if os.name == "nt" else "lxmd")
TITLE = b"PROPAGATION TITLE"
CONTENT = b"PROPAGATION BODY"
TIMESTAMP = 1_753_603_204.5
SENDER_SEED = bytes([0x61] * 64)
RECEIVER_SEED = bytes([0x62] * 64)


def free_port() -> int:
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return int(listener.getsockname()[1])


def spawn(command: list[str], env: dict[str, str], prefix: str, lines: list[str], stdin=None):
    process = subprocess.Popen(
        command,
        cwd=REPO,
        env=env,
        stdin=stdin,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )

    def pump() -> None:
        assert process.stdout is not None
        for raw in process.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [{prefix}] {line}", flush=True)

    threading.Thread(target=pump, daemon=True).start()
    return process


def wait_until(predicate, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return False


def value(lines: list[str], prefix: str) -> str | None:
    return next((line[len(prefix):] for line in lines if line.startswith(prefix)), None)


def stock_list(node_hash: bytes, identity: RNS.Identity) -> object:
    """List what lxmd holds for `identity`, as a stock client's first /get does."""
    node_identity = RNS.Identity.recall(node_hash)
    destination = RNS.Destination(
        node_identity, RNS.Destination.OUT, RNS.Destination.SINGLE, "lxmf", "propagation"
    )
    result: dict[str, object] = {}
    done = threading.Event()

    def established(link) -> None:
        link.identify(identity)
        link.request(
            LXMPeer.MESSAGE_GET_PATH,
            [None, None],
            response_callback=lambda receipt: (result.update(response=receipt.response), done.set()),
            failed_callback=lambda receipt: done.set(),
        )

    link = RNS.Link(destination, established_callback=established)
    done.wait(timeout=30)
    link.teardown()
    return result.get("response", "no response")


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}", flush=True)
    port = free_port()
    env = {**os.environ, "RETINUE_PORT": str(port)}
    transport_lines: list[str] = []
    transport = spawn(
        [os.environ["RETINUE_TRANSPORT_NODE"]]
        if os.environ.get("RETINUE_TRANSPORT_NODE")
        else ["cargo", "run", "--quiet", "-p", "retinue", "--example", "transport_node"],
        env,
        "retinue",
        transport_lines,
    )
    fetcher = daemon = None
    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-fetch-ratchet-"))
    exit_code = 1
    try:
        if not wait_until(lambda: any(l.startswith("TRANSPORT_NODE_UP ") for l in transport_lines), 180):
            return 1
        fetch_lines: list[str] = []
        fetcher = spawn(
            [os.environ["OUTRIDER_PROPAGATION_FETCH_RECEIVE"]]
            if os.environ.get("OUTRIDER_PROPAGATION_FETCH_RECEIVE")
            else ["cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_propagation_fetch_receive"],
            {**os.environ, "RETINUE_ADDR": f"127.0.0.1:{port}"},
            "outrider",
            fetch_lines,
            stdin=subprocess.PIPE,
        )

        client_rns, sender_store, node_rns, node_config = (
            root / name for name in ("client-rns", "sender-store", "node-rns", "node")
        )
        for directory in (client_rns, sender_store, node_rns, node_config):
            directory.mkdir()
        interface_config = (
            "[reticulum]\nenable_transport=No\nshare_instance=No\npanic_on_interface_error=No\n"
            "\n[logging]\nloglevel=5\n"
            "\n[interfaces]\n[[retinue]]\ntype=TCPClientInterface\nenabled=yes\n"
            f"target_host=127.0.0.1\ntarget_port={port}\n"
        )
        (client_rns / "config").write_text(interface_config, encoding="utf-8")
        (node_rns / "config").write_text(interface_config, encoding="utf-8")
        (node_config / "config").write_text(
            "[propagation]\nenable_node=yes\nnode_name=Stock Propagation Oracle\n"
            "announce_at_start=yes\nautopeer=no\npropagation_stamp_cost_target=8\npeering_cost=8\n"
            "\n[lxmf]\ndisplay_name=Stock Delivery Oracle\nannounce_at_start=no\n"
            "\n[logging]\nloglevel=5\n",
            encoding="utf-8",
        )
        messagestore = node_config / "storage" / "lxmf" / "messagestore"

        RNS.Reticulum(configdir=str(client_rns))
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

            def received_announce(self, destination_hash, announced_identity, app_data) -> None:
                if not node_seen.is_set():
                    state["node"] = bytes(destination_hash)
                    sender_router.set_outbound_propagation_node(bytes(destination_hash))
                    node_seen.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        daemon = spawn(
            [str(LXMD), "-p", "--config", str(node_config), "--rnsconfig", str(node_rns), "--verbose"],
            dict(os.environ),
            "lxmd",
            [],
        )
        if not node_seen.wait(timeout=45):
            print("stock learned propagation node: FAIL")
            return 1
        if not wait_until(lambda: value(fetch_lines, "FETCH_READY ") is not None, 45):
            return 1
        for _ in range(3):
            source.announce()
            time.sleep(0.3)

        # The stock sender must hold the recipient's announced ratchet before it encrypts.
        recipient_hash = RNS.Destination.hash(receiver_identity, "lxmf", "delivery")
        ratchet_deadline = time.time() + 45
        while time.time() < ratchet_deadline and RNS.Identity.get_ratchet(recipient_hash) is None:
            RNS.Transport.request_path(recipient_hash)
            time.sleep(1)
        learned = RNS.Identity.get_ratchet(recipient_hash) is not None
        print(f"stock learned Outrider's ratchet: {'PASS' if learned else 'FAIL'}")

        destination = RNS.Destination(
            RNS.Identity.recall(recipient_hash), RNS.Destination.OUT, RNS.Destination.SINGLE,
            "lxmf", "delivery",
        )
        message = LXMF.LXMessage(
            destination, source, CONTENT, title=TITLE, desired_method=LXMF.LXMessage.PROPAGATED
        )
        message.timestamp = TIMESTAMP
        sender_router.handle_outbound(message)
        wait_until(lambda: message.state in (LXMF.LXMessage.SENT, LXMF.LXMessage.FAILED), 180)
        submitted = message.state == LXMF.LXMessage.SENT
        wait_until(lambda: messagestore.is_dir() and any(messagestore.iterdir()), 10)
        stored_before = len(list(messagestore.iterdir())) if messagestore.is_dir() else 0

        assert fetcher.stdin is not None
        fetcher.stdin.write("fetch\n")
        fetcher.stdin.flush()
        wait_until(lambda: value(fetch_lines, "PRODUCTION_FETCH ") is not None, 60)

        fetched = "PRODUCTION_FETCH true" in fetch_lines
        decoded = (
            value(fetch_lines, "MESSAGE_ID ") == message.hash.hex()
            and value(fetch_lines, "TITLE ") == TITLE.hex()
            and value(fetch_lines, "CONTENT ") == CONTENT.hex()
            and value(fetch_lines, "VERIFIED ") == "true"
        )
        stock_ratchet = message.ratchet_id.hex() if message.ratchet_id else None
        ratcheted = stock_ratchet is not None and value(fetch_lines, "RATCHET_ID ") == stock_ratchet
        acknowledged = "ACKNOWLEDGED true" in fetch_lines
        cleared = wait_until(lambda: not any(messagestore.iterdir()), 10)
        relisted = stock_list(state["node"], receiver_identity)

        checks = {
            "stock submitted to lxmd": submitted and stored_before == 1,
            "stock sealed to Outrider's ratchet": learned and ratcheted,
            "Outrider fetched and decrypted": fetched and decoded,
            "Outrider acknowledged": acknowledged,
            "lxmd message store emptied": cleared,
            "second stock list returns []": relisted == [],
        }
        for name, ok in checks.items():
            print(f"{name}: {'PASS' if ok else 'FAIL'}")
        print(f"  stock ratchet {stock_ratchet}, stored {stored_before}, relisted {relisted!r}")
        ok = all(checks.values())
        print(f"OUTRIDER_RATCHETED_FETCH_FROM_STOCK: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        for process in (daemon, fetcher, transport):
            if process is not None and process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
