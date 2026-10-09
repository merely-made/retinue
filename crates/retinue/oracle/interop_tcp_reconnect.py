"""Does a retinue TCP client survive its stock RNS hub restarting, as a stock client does?

A stock TCPServerInterface node (transport on) runs in a child process and hosts D. Retinue
dials `localhost:<port>` by name before the hub exists, so the first dial fails and must
still attach (TCPInterface.py 165-171). Once the hub is up, retinue comes online and
announces, and the hub learns the path. The hub announces D; then it is killed, and after
3 s a fresh hub with D's identity starts without announcing. Retinue must come back online
on the same InterfaceId within 15 s, keep its route to D (no path request reaches the hub),
open a link the hub's D sees ACTIVE, and one announce must give the new hub a path to it.

Run from the oracle/ directory:  ./.venv/bin/python -u interop_tcp_reconnect.py
"""
from __future__ import annotations

import os, re, shutil, signal, socket, subprocess, sys, tempfile, threading, time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
D_SEED = bytes.fromhex("9d" * 64)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Proc:
    """A child speaking a line protocol: commands on stdin, events on stdout."""

    def __init__(self, label, argv, env=None, cwd=None):
        self.label, self.lines, self.cursor = label, [], 0
        self.p = subprocess.Popen(argv, cwd=cwd, env=dict(os.environ, **(env or {})), text=True,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, bufsize=1)
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self):
        for line in self.p.stdout:
            self.lines.append(line.rstrip())
            print(f"  [{self.label}] {line.rstrip()}")

    def send(self, line):
        self.p.stdin.write(line + "\n"); self.p.stdin.flush()

    def expect(self, pattern, timeout):
        deadline = time.time() + timeout
        while time.time() < deadline:
            while self.cursor < len(self.lines):
                line = self.lines[self.cursor]; self.cursor += 1
                if m := re.match(pattern, line):
                    return m
            if self.p.poll() is not None and self.cursor >= len(self.lines):
                break
            time.sleep(0.05)
        return None

    def kill(self):
        try: self.p.send_signal(signal.SIGKILL); self.p.wait(5)
        except Exception: pass


def hub(cfg: Path, port: int) -> Proc:
    return Proc(f"hub {cfg.name}", [sys.executable, "-u", __file__, "--hub", str(cfg), str(port)])


def run_hub(cfg: str, port: int) -> None:
    import RNS
    Path(cfg).mkdir(parents=True, exist_ok=True)
    (Path(cfg) / "config").write_text(
        "[reticulum]\n  enable_transport = Yes\n  share_instance = No\n  panic_on_interface_error = No\n"
        "\n[logging]\n  loglevel = 2\n\n[interfaces]\n  [[hub]]\n    type = TCPServerInterface\n"
        f"    enabled = yes\n    listen_ip = 127.0.0.1\n    listen_port = {port}\n", encoding="utf-8")
    pr_hash = RNS.Destination.hash_from_name_and_identity("rnstransport.path.request", None)
    requested = []
    original = RNS.Transport.inbound
    def counting(raw, *args, **kwargs):
        packet = RNS.Packet(None, raw)
        if packet.unpack() and packet.destination_hash == pr_hash:
            requested.append(bytes(packet.data[:16]))
        return original(raw, *args, **kwargs)
    RNS.Transport.inbound = counting
    RNS.Reticulum(configdir=cfg)
    d = RNS.Destination(RNS.Identity.from_bytes(D_SEED), RNS.Destination.IN, RNS.Destination.SINGLE,
                        "retinue", "reconnect_oracle")
    d.set_link_established_callback(
        lambda link: print(f"LINK_ESTABLISHED {link.status == RNS.Link.ACTIVE}", flush=True))
    print(f"STOCK_DEST {d.hash.hex()}", flush=True)
    for line in sys.stdin:
        cmd = line.split()
        if cmd[:1] == ["ANNOUNCE"]:
            d.announce(); print("ANNOUNCED", flush=True)
        elif cmd[:1] == ["PATH"]:
            target, deadline = bytes.fromhex(cmd[1]), time.time() + float(cmd[2])
            while not RNS.Transport.has_path(target) and time.time() < deadline:
                time.sleep(0.1)
            print(f"PATH {RNS.Transport.has_path(target)}", flush=True)
        elif cmd[:1] == ["PRS"]:
            print(f"PATH_REQUESTS {requested.count(bytes.fromhex(cmd[1]))}", flush=True)


def main() -> int:
    port = free_port()
    scratch = Path(tempfile.mkdtemp(prefix="retinue-reconnect-"))
    retinue = Proc("retinue", ["cargo", "run", "--quiet", "--example", "tcp_reconnect"],
                   {"RETINUE_HOST": "localhost", "RETINUE_PORT": str(port)}, cwd=REPO)
    hubs: list[Proc] = []
    checks: dict[str, bool] = {}
    try:
        me = retinue.expect(r"DEST ([0-9a-f]{32})", 300)
        attached = retinue.expect(r"ATTACHED (\d+) (true|false)", 30)
        checks["dial to a dead port attaches, offline"] = bool(me and attached and attached[2] == "false")
        if not checks["dial to a dead port attaches, offline"]:
            return 1
        me, iface = me[1], attached[1]

        hubs.append(hub(scratch / "first", port))
        d = hubs[0].expect(r"STOCK_DEST ([0-9a-f]{32})", 60)[1]
        online = retinue.expect(rf"ONLINE {iface} true", 20)
        hubs[0].send(f"PATH {me} 10")
        checks["first hub learns retinue's announce on connect"] = bool(
            online and hubs[0].expect(r"PATH (True|False)", 15)[1] == "True")
        hubs[0].send("ANNOUNCE")
        checks["retinue learns D"] = bool(retinue.expect(rf"LEARNED {d} {iface}", 15))

        hubs[0].kill()
        checks["retinue goes offline"] = bool(retinue.expect(rf"ONLINE {iface} false", 15))
        time.sleep(3)
        hubs.append(hub(scratch / "second", port))
        restarted = time.time()
        hubs[1].expect(r"STOCK_DEST", 60)
        back = retinue.expect(rf"ONLINE (\d+) true", 20)
        took = time.time() - restarted
        checks[f"online again on the same id within 15 s ({took:.1f} s)"] = bool(
            back and back[1] == iface and took <= 15)

        retinue.send(f"LINK {d}")
        route = retinue.expect(rf"ROUTE {d} (\S+)", 10)
        checks["route to D retained on the same interface"] = bool(route and route[1] == iface)
        linked = retinue.expect(r"LINKED|LINK_FAILED", 15)
        established = hubs[1].expect(r"LINK_ESTABLISHED (True|False)", 10)
        checks["stock D's link is ACTIVE"] = bool(
            linked and linked[0] == "LINKED" and established and established[1] == "True")
        hubs[1].send(f"PRS {d}")
        prs = hubs[1].expect(r"PATH_REQUESTS (\d+)", 10)
        checks["no path request for D reached the hub"] = bool(prs and prs[1] == "0")

        hubs[1].send(f"PATH {me} 0")
        fresh = hubs[1].expect(r"PATH (True|False)", 10)
        retinue.send("ANNOUNCE")
        hubs[1].send(f"PATH {me} 10")
        learned = hubs[1].expect(r"PATH (True|False)", 15)
        checks["new hub has_path(retinue) after one announce"] = bool(
            fresh and fresh[1] == "False" and learned and learned[1] == "True")
    finally:
        for p in [retinue, *hubs]:
            p.kill()
        shutil.rmtree(scratch, ignore_errors=True)

    print("\n" + "=" * 68)
    for name, ok in checks.items():
        print(f"{'PASS' if ok else 'FAIL'}  {name}")
    print("=" * 68)
    ok = len(checks) == 9 and all(checks.values())
    print(f"TCP RECONNECT INTEROP: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    if sys.argv[1:2] == ["--hub"]:
        run_hub(sys.argv[2], int(sys.argv[3]))
    else:
        raise SystemExit(main())
