"""Do interface modes gate announces as RNS 1.5.7 does (Transport.py 1458-1516)?

Topology: three stock TCPServer nodes, each its own RNS process, dialled by one transport:

    A (announcer) --+
    A2 (observer) --+-- transport (retinue mode_transport, or stock RNS as the control)
    B (observer) ---+

The transport relays A's announce and announces a destination of its own. Two scenarios:

  ap       B's interface is ACCESS_POINT: B learns neither destination.
  roaming  A's and B's interfaces are ROAMING: B learns only the transport's own
           destination; a roaming-to-roaming relay is blocked.

A2 (FULL) must learn both in every scenario. Each matrix is read from the stock observers'
own RNS.Transport.has_path, and must match both the expected matrix and the one a stock RNS
transport produces in retinue's place.

Run from the oracle/ directory:  ./.venv/bin/python -u interop_interface_modes.py
"""
from __future__ import annotations

import atexit, os, queue, shutil, socket, subprocess, sys, tempfile, threading, time
from pathlib import Path

import RNS

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
PREFIX = "@@ "
WINDOW = 15.0
SETTLE = 6.0  # past a relay's retry (5 s grace + 0.5 s window), so a late leak shows
SCENARIOS = {
    # name: (A's mode, B's mode, expected {(observer, which): has_path})
    "ap": ("full", "access_point",
           {("A2", "relayed"): True, ("A2", "own"): True,
            ("B", "relayed"): False, ("B", "own"): False}),
    "roaming": ("roaming", "roaming",
                {("A2", "relayed"): True, ("A2", "own"): True,
                 ("B", "relayed"): False, ("B", "own"): True}),
}


def say(line: str) -> None:
    print(PREFIX + line, flush=True)


def child_config(extra: str, transport: bool) -> str:
    cfg = Path(tempfile.mkdtemp(prefix="retinue-modes-"))
    atexit.register(shutil.rmtree, cfg, ignore_errors=True)
    (cfg / "config").write_text(
        f"[reticulum]\n  enable_transport = {'Yes' if transport else 'No'}\n"
        "  share_instance = No\n  panic_on_interface_error = No\n"
        f"\n[logging]\n  loglevel = 2\n\n[interfaces]\n{extra}", encoding="utf-8")
    return str(cfg)


def serve_commands(destination_factory) -> None:
    """Answer `announce` and `has <hex>` lines on stdin until it closes."""
    destination = None
    for line in sys.stdin:
        cmd = line.split()
        if cmd[:1] == ["announce"]:
            destination = destination or destination_factory()
            destination.announce()
            say(f"DEST {destination.hash.hex()}")
        elif cmd[:1] == ["has"]:
            say(f"HAS {cmd[1]} {int(RNS.Transport.has_path(bytes.fromhex(cmd[1])))}")
    RNS.exit(0)


def run_server(port: int) -> None:
    RNS.Reticulum(configdir=child_config(
        "  [[server]]\n    type = TCPServerInterface\n    enabled = yes\n"
        f"    listen_ip = 127.0.0.1\n    listen_port = {port}\n", transport=False))
    say("READY")
    serve_commands(lambda: RNS.Destination(RNS.Identity(), RNS.Destination.IN,
                                           RNS.Destination.SINGLE, "modes_gate", "announcer"))


def run_control(peers: str) -> None:
    extra = ""
    for index, peer in enumerate(peers.split(",")):
        port, mode = peer.split(":")
        extra += (f"  [[peer{index}]]\n    type = TCPClientInterface\n    enabled = yes\n"
                  f"    target_host = 127.0.0.1\n    target_port = {port}\n    mode = {mode}\n")
    RNS.Reticulum(configdir=child_config(extra, transport=True))
    deadline = time.time() + 10
    while time.time() < deadline and not all(i.online for i in RNS.Transport.interfaces):
        time.sleep(0.1)
    own = RNS.Destination(RNS.Identity(), RNS.Destination.IN, RNS.Destination.SINGLE,
                          "modes_gate", "transport")
    print(f"MODE_TRANSPORT_UP {own.hash.hex()}", flush=True)
    for line in sys.stdin:
        if line.strip() == "announce":
            own.announce()
            print("ANNOUNCED", flush=True)
    RNS.exit(0)


class Proc:
    """A child process whose output lines land in a queue (protocol lines unprefixed)."""

    def __init__(self, label: str, argv: list[str], cwd: Path, env=None):
        self.label, self.lines = label, queue.Queue()
        self.p = subprocess.Popen(argv, cwd=cwd, env=env, text=True, bufsize=1,
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  stderr=subprocess.STDOUT)
        atexit.register(self.kill)
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        for raw in self.p.stdout:
            line = raw.rstrip()
            print(f"  [{self.label}] {line}")
            self.lines.put(line[len(PREFIX):] if line.startswith(PREFIX) else line)

    def send(self, line: str):
        self.p.stdin.write(line + "\n")
        self.p.stdin.flush()

    def expect(self, prefix: str, timeout: float) -> str:
        deadline = time.time() + timeout
        while time.time() < deadline:
            try:
                line = self.lines.get(timeout=max(0.05, deadline - time.time()))
            except queue.Empty:
                break
            if line.startswith(prefix):
                return line
        raise TimeoutError(f"{self.label}: no {prefix!r} within {timeout} s")

    def kill(self):
        if self.p.poll() is None:
            self.p.kill()


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def run_scenario(name: str, stock: bool) -> dict:
    a_mode, b_mode, expected = SCENARIOS[name]
    me = [sys.executable, "-u", str(Path(__file__).resolve())]
    ports = {label: free_port() for label in ("A", "A2", "B")}
    nodes = {label: Proc(f"{name}/{label}", me + ["server", str(port)], HERE)
             for label, port in ports.items()}
    for node in nodes.values():
        node.expect("READY", 30)
    peers = f"{ports['A']}:{a_mode},{ports['A2']}:full,{ports['B']}:{b_mode}"
    if stock:
        transport = Proc(f"{name}/rnsd", me + ["control", peers], HERE)
    else:
        transport = Proc(f"{name}/retinue",
                         ["cargo", "run", "--quiet", "--example", "mode_transport"], REPO,
                         env=dict(os.environ, RETINUE_PEERS=peers))
    own = transport.expect("MODE_TRANSPORT_UP", 600).split()[1]
    time.sleep(1.5)
    transport.send("announce")
    nodes["A"].send("announce")
    relayed = nodes["A"].expect("DEST", 10).split()[1]
    targets = {"relayed": relayed, "own": own}

    def matrix() -> dict:
        result = {}
        for observer in ("A2", "B"):
            for which, dest in targets.items():
                nodes[observer].send(f"has {dest}")
                reply = nodes[observer].expect(f"HAS {dest}", 5)
                result[(observer, which)] = reply.split()[2] == "1"
        return result

    deadline = time.time() + WINDOW
    while time.time() < deadline:
        if all(matrix()[key] for key, want in expected.items() if want):
            break
        time.sleep(0.5)
    time.sleep(SETTLE)
    seen = matrix()
    for proc in (transport, *nodes.values()):
        proc.kill()
    return seen


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] == "server":
        run_server(int(sys.argv[2]))
        return 0
    if len(sys.argv) > 1 and sys.argv[1] == "control":
        run_control(sys.argv[2])
        return 0

    print(f"RNS {RNS.__version__}\n")
    ok = True
    for name, (_, _, expected) in SCENARIOS.items():
        got = run_scenario(name, stock=False)
        control = run_scenario(name, stock=True)
        passed = got == expected == control
        ok &= passed
        print("\n" + "=" * 68)
        for key in expected:
            print(f"{name:8} {key[0]:2} has_path({key[1]:7}) "
                  f"expected {expected[key]!s:5} retinue {got[key]!s:5} stock {control[key]!s:5}")
        print(f"MODES-{name.upper()}: {'PASS' if passed else 'FAIL'}")
        print("=" * 68 + "\n")
    print(f"R-IF4 RETINUE-INTERFACE-MODES-MATCH-RNS: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
