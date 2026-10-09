"""Stock LXMF clients against Outrider's propagation node, judged by stock state.

Not a gate itself: the interop_propagation_* gates that drive the node import it.
"""

from __future__ import annotations

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
SENDER_SEED = bytes([0x61] * 64)
RECEIVER_SEED = bytes([0x62] * 64)


class StockNodeHarness:
    """Run the node example, one stock sender and one stock receiver over TCP."""

    def __init__(self, env: dict[str, str] | None = None) -> None:
        print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
        executable = os.environ.get("OUTRIDER_PROPAGATION_SERVER")
        command = (
            [executable]
            if executable
            # Release: a stock link packet's proof is due within six RTTs (`Packet.py` 430),
            # and on loopback a debug build's stamp check outlasts that.
            else ["cargo", "run", "--quiet", "--release", "-p", "outrider", "--example", "stock_propagation_server"]
        )
        self.server = subprocess.Popen(
            command,
            cwd=REPO,
            env={**os.environ, **(env or {})},
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        self.lines: list[str] = []
        threading.Thread(target=self._pump, daemon=True).start()
        port = self._match(r"LISTENING (\d+)", 180)
        destination = self._match(r"PROPAGATION_DESTINATION ([0-9a-f]{32})", 10)
        if port is None or destination is None:
            self.server.kill()
            raise SystemExit("Outrider node did not start")
        self.node_destination = bytes.fromhex(destination)

        self.root = Path(tempfile.mkdtemp(prefix="outrider-node-"))
        rns_config = self.root / "rns"
        rns_config.mkdir()
        (rns_config / "config").write_text(
            "[reticulum]\nenable_transport=No\nshare_instance=No\npanic_on_interface_error=No\n"
            "\n[logging]\nloglevel=5\n"
            "\n[interfaces]\n[[outrider]]\ntype=TCPClientInterface\nenabled=yes\n"
            f"target_host=127.0.0.1\ntarget_port={port}\n",
            encoding="utf-8",
        )
        RNS.Reticulum(configdir=str(rns_config))
        self.sender_identity = RNS.Identity.from_bytes(SENDER_SEED)
        self.receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        self.sender = self._router(self.sender_identity, "sender", "Propagation Sender")
        self.receiver = self._router(self.receiver_identity, "receiver", "Propagation Receiver")
        self.delivered: list[LXMF.LXMessage] = []
        self.receiver.register_delivery_callback(self.delivered.append)
        self.offers: list[list[bytes]] = []
        list_response = self.receiver.message_list_response

        def record_offer(receipt) -> None:
            if isinstance(receipt.response, list):
                self.offers.append(list(receipt.response))
            list_response(receipt)

        # Record what the stock client itself was offered, before it acts on it.
        self.receiver.message_list_response = record_offer

    def _pump(self) -> None:
        assert self.server.stdout is not None
        for raw in self.server.stdout:
            line = raw.rstrip()
            self.lines.append(line)
            print(f"  [outrider] {line}", flush=True)

    def _match(self, pattern: str, timeout: float) -> str | None:
        deadline = time.time() + timeout
        while time.time() < deadline and self.server.poll() is None:
            for line in list(self.lines):
                found = re.fullmatch(pattern, line)
                if found:
                    return found.group(1)
            time.sleep(0.1)
        return None

    def _router(self, identity, name: str, display_name: str):
        store = self.root / name
        store.mkdir()
        router = LXMF.LXMRouter(identity=identity, storagepath=str(store))
        router.register_delivery_identity(identity, display_name=display_name, stamp_cost=None)
        return router

    def learn_node(self) -> bool:
        seen = threading.Event()
        harness = self

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(self, destination_hash, announced_identity, app_data) -> None:
                if destination_hash == harness.node_destination and not seen.is_set():
                    harness.sender.set_outbound_propagation_node(bytes(destination_hash))
                    harness.receiver.set_outbound_propagation_node(bytes(destination_hash))
                    seen.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())
        ok = seen.wait(timeout=45)
        print(f"stock learned Outrider node: {'PASS' if ok else 'FAIL'}")
        # The sender's source must be known for the receiver to verify it.
        sender_source = self.sender.delivery_destinations[
            RNS.Destination.hash_from_name_and_identity("lxmf.delivery", self.sender_identity)
        ]
        sender_source.announce()
        return ok

    def send(self, content: bytes, title: bytes) -> LXMF.LXMessage:
        sender_source = self.sender.delivery_destinations[
            RNS.Destination.hash_from_name_and_identity("lxmf.delivery", self.sender_identity)
        ]
        destination = RNS.Destination(
            self.receiver_identity, RNS.Destination.OUT, RNS.Destination.SINGLE, "lxmf", "delivery"
        )
        message = LXMF.LXMessage(
            destination, sender_source, content, title=title, desired_method=LXMF.LXMessage.PROPAGATED
        )
        self.sender.handle_outbound(message)
        return message

    def sync(self, timeout: float) -> list[bytes] | None:
        """Run one stock sync and return what the node offered, or None if it never completed."""
        offers = len(self.offers)
        self.receiver.request_messages_from_propagation_node(self.receiver_identity)
        complete = wait(lambda: self.receiver.propagation_transfer_state == LXMF.LXMRouter.PR_COMPLETE, timeout)
        return self.offers[offers] if complete and len(self.offers) > offers else None

    def store_entries(self) -> int | None:
        for line in reversed(self.lines):
            found = re.fullmatch(r"STORE entries=(\d+) bytes=\d+", line)
            if found:
                return int(found.group(1))
        return None

    def close(self, exit_code: int) -> None:
        self.server.terminate()
        try:
            self.server.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.server.kill()
        shutil.rmtree(self.root, ignore_errors=True)
        RNS.exit(exit_code)


def wait(predicate, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.2)
    return predicate()


def verdict(label: str, ok: bool) -> bool:
    print(f"{label}: {'PASS' if ok else 'FAIL'}", flush=True)
    return ok
