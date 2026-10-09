"""Shared plumbing for the announce-ingress gates.

Each stock RNS node runs in its own child process (RNS state is process-global), started as
`python -u ingress_harness.py --node CONFIG_DIR` and driven with one JSON command per stdin
line; it answers `REPLY <json>`. The Retinue side is the prebuilt `ingress_probe` example
(`cargo build -p retinue --examples --all-features`), found under CARGO_TARGET_DIR.
"""
from __future__ import annotations

import atexit
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
EXPECTED_RNS = "1.5.7"
_children: list["Child"] = []


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def verdict(label: str, ok: bool, detail: str = "") -> bool:
    print(f"  [{'PASS' if ok else 'FAIL'}] {label}{': ' + detail if detail else ''}", flush=True)
    return ok


def wait_until(predicate, timeout: float, step: float = 0.1):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(step)
    return predicate()


class Child:
    """A line-oriented child process with captured, echoed output."""

    def __init__(self, argv: list[str], label: str, env: dict | None = None) -> None:
        self.label = label
        self.proc = subprocess.Popen(argv, cwd=REPO, env={**os.environ, **(env or {})},
                                     stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.lines: list[str] = []
        self.stamps: list[float] = []
        self.lock = threading.Lock()
        threading.Thread(target=self._pump, daemon=True).start()
        _children.append(self)

    def _pump(self) -> None:
        for raw in self.proc.stdout:
            line = raw.rstrip()
            with self.lock:
                self.lines.append(line)
                self.stamps.append(time.monotonic())
            if not line.startswith("REPLY "):
                print(f"  [{self.label}] {line}", flush=True)

    def send(self, line: str) -> None:
        self.proc.stdin.write(line + "\n")
        self.proc.stdin.flush()

    def matches(self, pattern: str, start: int = 0) -> list[re.Match]:
        with self.lock:
            lines = self.lines[start:]
        return [m for m in (re.fullmatch(pattern, line) for line in lines) if m]

    def first_seen(self, pattern: str) -> float | None:
        """When a line matching `pattern` first arrived, on the monotonic clock."""
        with self.lock:
            rows = list(zip(self.stamps, self.lines))
        return next((at for at, line in rows if re.fullmatch(pattern, line)), None)

    def wait_for(self, pattern: str, timeout: float, start: int = 0) -> re.Match | None:
        found = wait_until(lambda: self.matches(pattern, start) or self.proc.poll() is not None,
                           timeout, 0.05)
        hits = self.matches(pattern, start) if found else []
        return hits[0] if hits else None

    def mark(self) -> int:
        with self.lock:
            return len(self.lines)

    def kill(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            pass


class Retinue(Child):
    """The ingress_probe example."""

    def __init__(self, env: dict) -> None:
        target = Path(os.environ.get("CARGO_TARGET_DIR", REPO.parent.parent / "target"))
        binary = target / "debug" / "examples" / "ingress_probe"
        if not binary.is_file():
            raise FileNotFoundError(f"build the ingress_probe example first: {binary}")
        super().__init__([str(binary)], "retinue", env)
        up = self.wait_for(r"UP ([0-9a-f]{32})", 30)
        if up is None:
            raise RuntimeError("ingress_probe did not start")
        self.identity = up.group(1)
        self.ifaces = {int(m.group(1)): int(m.group(2)) for m in self.matches(r"IFACE (\d+) (\d+)")}
        port = self.matches(r"LISTENING (\d+)")
        self.port = int(port[0].group(1)) if port else None

    def ask(self, command: str, pattern: str, timeout: float = 5) -> list[re.Match]:
        start = self.mark()
        self.send(command)
        self.wait_for(pattern, timeout, start)
        return self.matches(pattern, start)

    def route(self, dest: str):
        m = self.ask(f"route {dest}", rf"ROUTE {dest} (\d+|none)(?: (\d+))?")
        return None if not m or m[0].group(1) == "none" else (int(m[0].group(1)), int(m[0].group(2)))

    def counters(self) -> dict[int, tuple[int, int, int, int]]:
        """Per interface id: (observed, held, released, held_dropped)."""
        start = self.mark()
        self.send("counters")
        self.wait_for(r"ROUTING .*", 5, start)
        rows = self.matches(r"COUNTERS (\d+) (\d+) (\d+) (\d+) (\d+)", start)
        return {int(m.group(1)): tuple(int(m.group(i)) for i in range(2, 6)) for m in rows}

    def announces(self) -> list[tuple[str, int, int, int]]:
        """(dest, iface, hops, released) for every published announce, in order."""
        return [(m.group(1), int(m.group(2)), int(m.group(3)), int(m.group(4)))
                for m in self.matches(r"ANNOUNCE ([0-9a-f]{32}) (\d+) (\d+) (\d+)")]


def tcp(kind: str, name: str, port: int, **options) -> str:
    """One [[interface]] section: kind is 'server' or 'client'."""
    lines = [f"  [[{name}]]", "    enabled = yes"]
    if kind == "server":
        lines += ["    type = TCPServerInterface", "    listen_ip = 127.0.0.1", f"    listen_port = {port}"]
    else:
        lines += ["    type = TCPClientInterface", "    target_host = 127.0.0.1", f"    target_port = {port}"]
    lines += [f"    {key} = {value}" for key, value in options.items()]
    return "\n".join(lines) + "\n"


class Stock(Child):
    """A stock RNS node in a child process."""

    def __init__(self, label: str, transport: bool, interfaces: str) -> None:
        self.config = Path(tempfile.mkdtemp(prefix=f"retinue-ingress-{label}-"))
        atexit.register(shutil.rmtree, self.config, ignore_errors=True)
        (self.config / "config").write_text(
            f"[reticulum]\n  enable_transport = {'Yes' if transport else 'No'}\n"
            "  share_instance = No\n  panic_on_interface_error = No\n\n"
            f"[logging]\n  loglevel = 2\n\n[interfaces]\n{interfaces}", encoding="utf-8")
        super().__init__([sys.executable, "-u", str(Path(__file__)), "--node", str(self.config)], label)
        ready = self.wait_for(r"READY ([0-9a-f]{32})", 30)
        if ready is None:
            raise RuntimeError(f"stock node {label} did not start")
        self.transport_id = ready.group(1)

    def call(self, op: str, timeout: float = 10, **args):
        start = self.mark()
        self.send(json.dumps({"op": op, **args}))
        reply = self.wait_for(r"REPLY (.*)", timeout, start)
        if reply is None:
            raise RuntimeError(f"{self.label}: no reply to {op}")
        return json.loads(reply.group(1))


def kill_all() -> None:
    for child in _children:
        child.kill()


atexit.register(kill_all)


def node_main(config_dir: str) -> None:
    """The child side of Stock: RNS plus stdin commands, with stock-state taps."""
    import RNS

    assert RNS.__version__ == EXPECTED_RNS, RNS.__version__
    relayed: dict[str, set] = {}
    path_requests: list[str] = []
    path_responses: list[str] = []
    blob_at = RNS.Identity.KEYSIZE // 8 + RNS.Identity.NAME_HASH_LENGTH // 8

    def parse(raw):
        packet = RNS.Packet(None, raw)
        return packet if packet.unpack() and packet.packet_type == RNS.Packet.ANNOUNCE else None

    inbound, transmit, path_request = RNS.Transport.inbound, RNS.Transport.transmit, RNS.Transport.path_request

    def tap_inbound(raw, *args, **kwargs):
        packet = parse(raw)
        if packet is not None and packet.transport_id:
            blob = packet.data[blob_at:blob_at + 10]
            relayed.setdefault(packet.transport_id.hex(), set()).add((packet.destination_hash, blob))
        return inbound(raw, *args, **kwargs)

    def tap_transmit(interface, raw):
        packet = parse(raw)
        if packet is not None and packet.context == RNS.Packet.PATH_RESPONSE:
            path_responses.append(packet.destination_hash.hex())
        return transmit(interface, raw)

    def tap_path_request(destination_hash, *args, **kwargs):
        path_requests.append(destination_hash.hex())
        return path_request(destination_hash, *args, **kwargs)

    RNS.Transport.inbound = tap_inbound
    RNS.Transport.transmit = tap_transmit
    RNS.Transport.path_request = tap_path_request
    reticulum = RNS.Reticulum(configdir=config_dir)
    identity = RNS.Identity()
    destinations: list = []
    held_packet = None

    def interface(name):
        return next((i for i in RNS.Transport.interfaces if getattr(i, "name", None) == name), None)

    def handle(req):
        nonlocal held_packet
        op = req["op"]
        if op == "online":
            return all(getattr(interface(n), "online", False) for n in req["names"])
        if op == "dests":
            for i in range(req["n"]):
                destinations.append(RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                                                    "ingressgate", req["aspect"], str(i)))
            return [d.hash.hex() for d in destinations[-req["n"]:]]
        if op == "announce":
            iface = interface(req["iface"]) if req.get("iface") else None
            destinations[req["index"]].announce(attached_interface=iface)
            return True
        if op == "announce_hold":
            held_packet = destinations[req["index"]].announce(attached_interface=interface(req["iface"]),
                                                              send=False)
            held_packet.send()
            return True
        if op == "resend_held":
            held_packet.attached_interface = interface(req["iface"])
            held_packet.resend()
            return True
        dest = bytes.fromhex(req.get("dest", ""))
        if op == "has_path":
            return RNS.Transport.has_path(dest)
        if op == "next_hop":
            hop = RNS.Transport.next_hop_interface(dest)
            return getattr(hop, "name", None)
        if op == "relayed":
            return sorted(blob.hex() for d, blob in relayed.get(req["transport"], ()) if d == dest)
        if op == "path_log":
            return {"requests": path_requests, "responses": path_responses}
        if op == "stats":
            return [{k: s[k] for k in ("short_name", "held_announces", "burst_active", "burst_count")}
                    for s in reticulum.get_interface_stats()["interfaces"]]
        raise ValueError(op)

    print(f"READY {RNS.Transport.identity.hash.hex()}", flush=True)
    for line in sys.stdin:
        try:
            result = handle(json.loads(line))
        except Exception as e:  # report, keep serving
            result = {"error": repr(e)}
        print("REPLY " + json.dumps(result), flush=True)
    RNS.exit(0)


if __name__ == "__main__" and sys.argv[1:2] == ["--node"]:
    node_main(sys.argv[2])
