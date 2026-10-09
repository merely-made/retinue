"""RNode gate without hardware: stock RNodeInterface and Retinue each drive a fake RNode on
`rnode_air`, which carries DATA between devices tuned alike (SF7, BW500).

Scenarios, each judged on stock's state or on the air's byte log:

- `main` (flow control, airtime locks and an ID beacon on both; the air answers each DATA
  with READY after 200 ms): has_path, an ACTIVE link, a request answered and a multi-part
  Resource COMPLETE on stock, with Retinue up although the air echoes its airtime locks
  lossily; Retinue's ALOCK and ID frames equal stock's; the ID goes out an interval after
  traffic and never twice without traffic between; Retinue never has two DATA frames
  unacknowledged; on shutdown its device sees RADIO_STATE 00 then LEAVE FF.
- `ifac` (`ifac_size = 64`): Resources complete both ways, and a request and its response
  that each fill a link packet (507 bytes on the air) are answered.
- `mismatch` (the air echoes TX power one lower): stock stays offline and Retinue faults.
- `error` (the air answers Retinue's 3rd DATA with ERROR 0x02 and its 6th with RESET 0xF8):
  Retinue reopens its port each time, detecting again within reconnect plus settle time.
- `negative` (stock on SF7, Retinue on SF8): nothing crosses the air.

    .venv/bin/python -u interop_rnode_air.py [SCENARIO]
"""

from __future__ import annotations

import subprocess
import sys
import tempfile
import time
from pathlib import Path

from pty_bridge import Retinue, exercise_stock, report, rns_config
from rnode_air_gate import (DATA, DETECT, ERROR, LEAVE, LT_ALOCK, RADIO_STATE, READY, RESET,
                            ST_ALOCK, Air, stock_rnode, wait_until)

STOCK, RETINUE = 0, 1
CALLSIGN, ID_INTERVAL = "N0CALL", 5
# RNS reconnects 5 s after a failure and waits 2 s after opening before it detects.
RECONNECT, SETTLE = 5.0, 2.0


def start_stock(interface: str):
    import RNS

    config_dir = Path(tempfile.mkdtemp(prefix="retinue-rnode-air-"))
    rns_config(config_dir, interface)
    RNS.Reticulum(configdir=str(config_dir))
    return RNS.Transport.interfaces[0]


def main_scenario() -> dict[str, bool]:
    air = Air(2, ("--ready-flow", "200"))
    beacon = (CALLSIGN, ID_INTERVAL)
    start_stock(stock_rnode(air.ports[STOCK], flow_control=True, alock=True, beacon=beacon))
    retinue = Retinue("rnode_peer", "tulle-radio", [
        air.ports[RETINUE], "--flow-control", "--alock-st", "3350", "--alock-lt", "1000",
        "--id", f"{CALLSIGN}:{ID_INTERVAL}"])
    results = exercise_stock(retinue, resource_len=1300)
    # An idle stretch long enough for a repeat, were the beacon to repeat on idle.
    time.sleep(2 * ID_INTERVAL + 2)
    retinue.close()
    time.sleep(1)

    locks = {dev: [f for _, f in air.host_frames(dev) if f[0] in (ST_ALOCK, LT_ALOCK)]
             for dev in (STOCK, RETINUE)}
    print(f"  ALOCK stock {[f.hex() for f in locks[STOCK][:2]]} "
          f"retinue {[f.hex() for f in locks[RETINUE][:2]]}")
    results["Retinue's ALOCK bytes equal stock's"] = (
        len(locks[STOCK]) >= 2 and locks[RETINUE][:2] == locks[STOCK][:2])
    echoes = [f for _, f in air.device_frames(RETINUE, ST_ALOCK)]
    results["Retinue online with a lossy ALOCK echo"] = (
        bool(echoes) and bool(locks[RETINUE]) and echoes[0] != locks[RETINUE][0]
        and results.get("stock link ACTIVE", False))

    callsign = CALLSIGN.encode()
    ids = {dev: [(t, f) for t, f in air.host_frames(dev, DATA) if f[1:] == callsign]
           for dev in (STOCK, RETINUE)}
    results["ID frame bytes equal stock's"] = (
        bool(ids[STOCK]) and [f for _, f in ids[RETINUE][:1]] == [f for _, f in ids[STOCK][:1]])
    # RNS re-arms the ID on any other traffic, so a busy link sends several; what must hold
    # is the first one's delay, and that none follows another without traffic between.
    data = [(t, f[1:] == callsign) for t, f in air.host_frames(RETINUE, DATA)]
    first_data = next((t for t, is_id in data if not is_id), None)
    id_times = [t for t, is_id in data if is_id]
    kinds = [is_id for _, is_id in data]
    print(f"  retinue ID frames at {[round(t - (first_data or 0), 1) for t in id_times]} s "
          f"after its first DATA")
    results["ID an interval after traffic, never repeated idle"] = (
        first_data is not None and bool(id_times)
        and id_times[0] - first_data >= ID_INTERVAL - 0.2
        and not any(a and b for a, b in zip(kinds, kinds[1:])))

    outstanding, worst, sent = 0, 0, 0
    for _, dev, direction, frame in air.frames():
        if dev != RETINUE:
            continue
        if direction == "d2h" and frame[0] == READY:
            outstanding = 0
        elif direction == "h2d" and frame[0] == DATA:
            sent += 1
            outstanding += 1
            worst = max(worst, outstanding)
    print(f"  retinue sent {sent} DATA frames, at most {worst} outstanding")
    results["READY flow: never two DATA outstanding"] = sent >= 3 and worst == 1

    tail = [f for _, f in air.host_frames(RETINUE)][-2:]
    print(f"  retinue's last host frames {[f.hex() for f in tail]}")
    results["shutdown: RADIO_STATE 00 then LEAVE FF"] = tail == [
        bytes([RADIO_STATE, 0x00]), bytes([LEAVE, 0xFF])]
    air.close()
    return results


def ifac_scenario() -> dict[str, bool]:
    air = Air(2)
    start_stock(stock_rnode(air.ports[STOCK], ifac_bits=64))
    retinue = Retinue("rnode_peer", "tulle-radio", [
        air.ports[RETINUE], "--ifac", "64", "--publish", "2000"])
    results = exercise_stock(retinue, resource_len=2000, publish_len=2000, request_len=400)
    retinue.close()
    # A full link packet is 499 bytes, 507 with the access code: past the old 500 cap.
    for dev, name in ((STOCK, "stock"), (RETINUE, "Retinue")):
        sizes = [len(f) - 1 for _, f in air.host_frames(dev, DATA)]
        print(f"  {name} sent DATA frames up to {max(sizes, default=0)} bytes")
        results[f"{name} sent a full link packet with IFAC (> 500 bytes)"] = max(sizes, default=0) > 500
    air.close()
    return results


def mismatch_scenario() -> dict[str, bool]:
    air = Air(2, ("--mismatch-txp", f"{STOCK},{RETINUE}"))
    stock = start_stock(stock_rnode(air.ports[STOCK]))
    retinue = Retinue("rnode_peer", "tulle-radio", [air.ports[RETINUE]])
    faulted = retinue.wait_for(r"STATUS Fault\(.*TxPower", 120) is not None
    results = {
        "Retinue faults on a TX power mismatch": faulted,
        "stock stays offline on the same mismatch": not stock.online,
        "Retinue never went online": retinue.wait_for(r"STATUS Online", 1) is None,
    }
    retinue.close()
    air.close()
    return results


def error_scenario() -> dict[str, bool]:
    air = Air(2, ("--error-after", f"{RETINUE}:3", "--reset-after", f"{RETINUE}:6"))
    retinue = Retinue("rnode_peer", "tulle-radio", [air.ports[RETINUE]])
    wait_until(lambda: len(air.device_frames(RETINUE, RESET)) > 0, 120)
    wait_until(lambda: len(air.host_frames(RETINUE, DETECT)) >= 3, 15)
    retinue.close()
    detects = [t for t, _ in air.host_frames(RETINUE, DETECT)]
    results = {}
    for name, command in (("ERROR 0x02", ERROR), ("RESET 0xF8", RESET)):
        fault = [t for t, _ in air.device_frames(RETINUE, command)]
        after = [t - fault[0] for t in detects if fault and t > fault[0]]
        print(f"  {name}: next DETECT {round(after[0], 2) if after else None} s later")
        results[f"Retinue reopens after {name}"] = (
            bool(after) and RECONNECT <= after[0] <= RECONNECT + SETTLE + 1.5)
    air.close()
    return results


def negative_scenario() -> dict[str, bool]:
    import RNS

    air = Air(2)
    start_stock(stock_rnode(air.ports[STOCK], sf=7))
    retinue = Retinue("rnode_peer", "tulle-radio", [air.ports[RETINUE], "--sf", "8"])
    match = retinue.wait_for(r"DEST ([0-9a-f]{32})", 300)
    time.sleep(20)
    dest = bytes.fromhex(match.group(1)) if match else b""
    results = {
        "both radios came up": wait_until(lambda: len(air.device_frames(STOCK, RADIO_STATE)) > 0
                                          and len(air.device_frames(RETINUE, RADIO_STATE)) > 0, 5),
        "Retinue transmitted": bool(air.host_frames(RETINUE, DATA)),
        "nothing crossed the air": not air.device_frames(STOCK, DATA)
        and not air.device_frames(RETINUE, DATA),
        "stock has no path to Retinue": bool(dest) and not RNS.Transport.has_path(dest),
    }
    retinue.close()
    air.close()
    return results


SCENARIOS = {"main": main_scenario, "ifac": ifac_scenario, "mismatch": mismatch_scenario,
             "error": error_scenario, "negative": negative_scenario}


def main() -> int:
    if len(sys.argv) > 1:
        import RNS

        code = report(f"RNODE AIR {sys.argv[1]}", SCENARIOS[sys.argv[1]]())
        if RNS.Reticulum.get_instance() is not None:  # `error` runs without stock
            RNS.exit(code)
        return code
    codes = [subprocess.run([sys.executable, "-u", __file__, name]).returncode
             for name in SCENARIOS]
    ok = not any(codes)
    print(f"RNODE AIR INTEROP: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
