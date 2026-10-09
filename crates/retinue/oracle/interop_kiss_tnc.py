"""KISS gate: stock RNS KISSInterface and Retinue's KISS TNC carrier on ptys.

`host`: stock KISSInterface and Retinue's KISS carrier joined by a null-modem bridge. Each
ignores the other's parameter and READY frames, so traffic flows end to end. Judged on
stock's state: has_path, an ACTIVE link, a request answered, a Resource COMPLETE.

`tnc`: each side talks to its own fake TNC, which answers every DATA frame with READY
after 300 ms. With the same configuration (flow control on, `id_interval = 5`,
`id_callsign = N0CALL`) Retinue's startup bytes must equal stock's, Retinue must never
have two DATA frames unacknowledged, and exactly one ID frame, equal to stock's, must
follow the first traffic by the interval and not repeat on an idle line.

    .venv/bin/python -u interop_kiss_tnc.py
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import time
from pathlib import Path

from pty_bridge import (Bridge, FakeTnc, Retinue, exercise_stock, kiss_frames, open_pty,
                        report, rns_config)

SCENARIOS = ("host", "tnc")
CALLSIGN = b"N0CALL"
ID_INTERVAL = 5


def stock_kiss(port: str, tnc: bool) -> str:
    extra = (f"    flow_control = yes\n    id_interval = {ID_INTERVAL}\n"
             f"    id_callsign = {CALLSIGN.decode()}\n") if tnc else ""
    return ("  [[kiss]]\n    type = KISSInterface\n    enabled = yes\n"
            f"    port = {port}\n    speed = 115200\n" + extra)


def host() -> dict[str, bool]:
    stock_master, stock_port = open_pty()
    retinue_master, retinue_port = open_pty()
    Bridge(stock_master, retinue_master)
    retinue = Retinue("serial_peer", "serial", [retinue_port, "--kiss"])
    if retinue.wait_for(r"^ATTACHED", 300) is None:
        return {"retinue attached": False}
    start_stock(stock_kiss(stock_port, tnc=False))
    try:
        return exercise_stock(retinue)
    finally:
        retinue.close()


def data_frames(events) -> list[tuple[float, bytes]]:
    return [(t, f[1:]) for t, kind, f in events if kind == "host" and f[0] & 0x0F == 0]


def tnc() -> dict[str, bool]:
    import RNS

    stock_master, stock_port = open_pty()
    retinue_master, retinue_port = open_pty()
    stock_tnc, retinue_tnc = FakeTnc(stock_master), FakeTnc(retinue_master)
    retinue = Retinue("serial_peer", "serial", [
        retinue_port, "--kiss", "--flow-control", "--id", f"{CALLSIGN.decode()}:{ID_INTERVAL}",
        "--burst", "5"])
    if retinue.wait_for(r"^BURST", 300) is None:
        return {"retinue attached": False}
    start_stock(stock_kiss(stock_port, tnc=True))
    time.sleep(3)
    RNS.Destination(RNS.Identity(), RNS.Destination.IN, RNS.Destination.SINGLE,
                    "retinue", "kiss-tnc").announce()
    # Long enough for each side's first beacon and, on an idle line, a second one.
    time.sleep(3 * ID_INTERVAL + 2)
    retinue.close()

    stock_raw, stock_events = stock_tnc.snapshot()
    retinue_raw, retinue_events = retinue_tnc.snapshot()
    results = {}
    startup = 20  # TXDELAY, TXTAIL, P, SLOTTIME, READY: five 4-byte frames.
    results["startup bytes equal stock's"] = (
        len(stock_raw) >= startup and retinue_raw[:startup] == stock_raw[:startup])
    print(f"  startup: stock {stock_raw[:startup].hex()} retinue {retinue_raw[:startup].hex()}")

    outstanding, worst = 0, 0
    for _, kind, frame in retinue_events:
        if kind == "ready":
            outstanding = 0
        elif frame[0] & 0x0F == 0:
            outstanding += 1
            worst = max(worst, outstanding)
    sent = data_frames(retinue_events)
    print(f"  retinue sent {len(sent)} DATA frames, at most {worst} unacknowledged")
    results["flow control: at most one DATA unacknowledged"] = len(sent) >= 3 and worst == 1

    beacon = CALLSIGN.ljust(15, b"\0")
    ids = [t for t, payload in sent if payload == beacon]
    first = sent[0][0] if sent else 0
    stock_ids = [p for _, p in data_frames(stock_events) if p.startswith(CALLSIGN)]
    print(f"  retinue ID frames at {[round(t - first, 1) for t in ids]} s after first traffic")
    results["exactly one ID frame, after the interval"] = (
        len(ids) == 1 and ids[0] - first >= ID_INTERVAL - 0.2)
    results["ID frame bytes equal stock's"] = bool(stock_ids) and stock_ids[0] == beacon
    results["startup frames parse as KISS"] = len(kiss_frames(retinue_raw[:startup])) == 5
    return results


def start_stock(interface: str) -> None:
    import RNS

    config_dir = Path(tempfile.mkdtemp(prefix="retinue-kiss-"))
    rns_config(config_dir, interface)
    RNS.Reticulum(configdir=str(config_dir))


def main() -> int:
    if len(sys.argv) > 1 and sys.argv[1] in SCENARIOS:
        import RNS

        results = host() if sys.argv[1] == "host" else tnc()
        code = report(f"KISS TNC {sys.argv[1]}", results)
        RNS.exit(code)
        return code
    codes = [subprocess.run([sys.executable, "-u", __file__, name]).returncode
             for name in SCENARIOS]
    ok = not any(codes)
    print(f"KISS TNC INTEROP: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
