"""Does retinue drop a TCP peer that stops reading, and keep serving its stock RNS peer?

Retinue listens and routes. A stock RNS client P connects and announces a burst of
destinations through it. A raw socket with a tiny receive window connects and never reads,
while retinue floods its own announce, so the raw peer's send buffer fills and retinue's
writes stall. Retinue must forget that interface once a write makes no progress for the
egress dead time (12 s, BackboneInterface.py 63-69), a few seconds after the buffer fills.
P must keep hearing retinue throughout: P has a path to retinue and processes retinue
announces sent after the drop.

Run from the oracle/ directory:  ./.venv/bin/python -u interop_egress_stall.py
"""
from __future__ import annotations

import atexit, shutil, socket, tempfile, threading, time
from pathlib import Path

import RNS

from interop_tcp_reconnect import Proc, REPO

heard: list[float] = []


def main() -> int:
    print(f"RNS {RNS.__version__}")
    retinue = Proc("retinue", ["cargo", "run", "--quiet", "--example", "listener_mode"],
                   {"RETINUE_LISTENERS": "full"}, cwd=REPO)
    stalled = None
    checks: dict[str, bool] = {}
    try:
        me = retinue.expect(r"DEST ([0-9a-f]{32})", 300)
        port = retinue.expect(r"LISTENING full (\d+)", 30)
        if not (me and port):
            return 1
        me, port = bytes.fromhex(me[1]), int(port[1])

        cfg = Path(tempfile.mkdtemp(prefix="retinue-stall-"))
        atexit.register(shutil.rmtree, cfg, ignore_errors=True)
        (cfg / "config").write_text(
            "[reticulum]\n  enable_transport = No\n  share_instance = No\n  panic_on_interface_error = No\n"
            "\n[logging]\n  loglevel = 2\n\n[interfaces]\n  [[retinue]]\n    type = TCPClientInterface\n"
            f"    enabled = yes\n    target_host = 127.0.0.1\n    target_port = {port}\n", encoding="utf-8")
        original = RNS.Transport.inbound
        def counting(raw, *args, **kwargs):
            packet = RNS.Packet(None, raw)
            if packet.unpack() and packet.packet_type == RNS.Packet.ANNOUNCE \
                    and packet.destination_hash == me:
                heard.append(time.time())
            return original(raw, *args, **kwargs)
        RNS.Transport.inbound = counting
        RNS.Reticulum(configdir=str(cfg))
        stock = retinue.expect(r"SPAWNED full (\d+)", 15)

        stalled = socket.socket()
        stalled.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1024)
        stalled.connect(("127.0.0.1", port))
        connected = time.time()
        raw = retinue.expect(r"SPAWNED full (\d+)", 10)
        if not (stock and raw):
            return 1

        def burst():
            for n in range(40):
                identity = RNS.Identity()
                RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                                "retinue", "stall", str(n)).announce()
                time.sleep(0.1)
        threading.Thread(target=burst, daemon=True).start()
        retinue.send("FLOOD 8000 1")

        gone = retinue.expect(rf"GONE ({raw[1]}|{stock[1]})", 40)
        took = time.time() - connected
        checks[f"stalled interface forgotten within 25 s ({took:.1f} s)"] = bool(
            gone and gone[1] == raw[1] and took <= 25)
        dropped_at = time.time()
        time.sleep(3)
        retinue.send("ANNOUNCE")
        time.sleep(2)
        during = [t for t in heard if connected + 3 < t < dropped_at]
        after = [t for t in heard if t > dropped_at]
        checks[f"P heard retinue during the stall ({len(during)} announces)"] = len(during) > 0
        checks[f"P heard retinue after the drop ({len(after)} announces)"] = len(after) > 0
        checks["P has_path(retinue)"] = RNS.Transport.has_path(me)
        checks["P's interface still attached"] = not any(
            line == f"GONE {stock[1]}" for line in retinue.lines)
    finally:
        if stalled:
            stalled.close()
        retinue.kill()

    print("\n" + "=" * 68)
    for name, ok in checks.items():
        print(f"{'PASS' if ok else 'FAIL'}  {name}")
    print("=" * 68)
    ok = len(checks) == 5 and all(checks.values())
    print(f"EGRESS STALL INTEROP: {'PASS' if ok else 'FAIL'}")
    RNS.exit(0 if ok else 1)
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
