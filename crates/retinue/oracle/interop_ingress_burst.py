"""Ingress burst gate: per-interface holds, fewest-hops release, a quiet neighbour.

Topology: a stock flooder announces 60 unknown destinations at 20 Hz, alternately straight
into a stock transport hub and through a second stock transport behind it, so they reach the
hub's clients at 1 and 2 hops. Both transports run with ingress control off so they pass the
burst on. Retinue and a stock observer sit in the same spot, as clients of the hub, with the
same shortened burst timings. The observer also connects to Retinue's listener, a second
Retinue interface, and announces there during the burst.

Pass when Retinue holds part of the burst, learns every destination, releases a fewest-hops
entry first (`Interface.py` 278-297), and learns the quiet neighbour promptly; and when the
stock observer's own interface stats show it held or burst on the same traffic.

Build: cargo build -p retinue --examples --all-features
Run:   python -u interop_ingress_burst.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import time

from ingress_harness import Retinue, Stock, free_port, kill_all, tcp, verdict, wait_until

TITLE = "INGRESS BURST INTEROP"
FLOOD = 60
# ic_burst_hold, ic_burst_penalty and ic_held_release_interval, on both sides.
HOLD_S, PENALTY_S, RELEASE_S = 3, 3, 0.5


def main() -> int:
    hub_port, relay_port = free_port(), free_port()
    hub = Stock("hub", True, tcp("server", "hub", hub_port, ingress_control="no"))
    relay = Stock("relay", True, tcp("server", "relay", relay_port, ingress_control="no")
                  + tcp("client", "up", hub_port, ingress_control="no"))
    retinue = Retinue({"RETINUE_CONNECT": f"127.0.0.1:{hub_port}", "RETINUE_LISTEN": "0",
                       "RETINUE_INGRESS": f"{HOLD_S * 1000},{PENALTY_S * 1000},{int(RELEASE_S * 1000)}"})
    timings = dict(ic_burst_hold=HOLD_S, ic_burst_penalty=PENALTY_S, ic_held_release_interval=RELEASE_S)
    observer = Stock("observer", False, tcp("client", "hub", hub_port, **timings)
                     + tcp("client", "quiet", retinue.port))
    flood = Stock("flood", False, tcp("client", "hub", hub_port) + tcp("client", "relay", relay_port))
    for node, names in ((relay, ["up"]), (observer, ["hub", "quiet"]), (flood, ["hub", "relay"])):
        wait_until(lambda: node.call("online", names=names), 20)
    time.sleep(1.0)

    dests = flood.call("dests", n=FLOOD, aspect="flood")
    quiet = observer.call("dests", n=1, aspect="quiet")[0]
    peak = {"held": 0, "burst": False}
    quiet_at = None
    started = time.monotonic()
    for index in range(FLOOD):
        flood.call("announce", index=index, iface="hub" if index % 2 == 0 else "relay")
        if index == FLOOD // 2:
            quiet_at = time.monotonic()
            observer.call("announce", index=0, iface="quiet")
        time.sleep(max(0.0, started + (index + 1) / 20 - time.monotonic()))
    for _ in range(30):  # the observer's view while the burst lands
        stats = next(s for s in observer.call("stats") if s["short_name"] == "hub")
        peak["held"] = max(peak["held"], stats["held_announces"])
        peak["burst"] |= bool(stats["burst_active"])
        time.sleep(0.1)

    wire = retinue.ifaces[0]
    learned = wait_until(lambda: {a[0] for a in retinue.announces()} >= set(dests), 120, 0.5)
    announces = retinue.announces()
    quiet_seen = [a for a in announces if a[0] == quiet]
    quiet_learned_at = retinue.first_seen(rf"ANNOUNCE {quiet} .*")
    quiet_lag = quiet_learned_at - quiet_at if quiet_learned_at and quiet_at else None
    # A release is the announce published as its interface's release count moved.
    released, count = [], 0
    for a in announces:
        if a[1] == wire and a[3] != count:
            released.append(a)
            count = a[3]
    first = released[0] if released else None
    release_hops = [a[2] for a in released]
    hops = {a[2] for a in announces if a[0] in dests}
    observed, held, releases, dropped = retinue.counters().get(wire, (0, 0, 0, 0))

    print("\n" + "=" * 72)
    ok = verdict("Retinue held part of the burst", held > 0 and releases > 0,
                 f"held {held}, released {releases}, dropped {dropped}")
    ok &= verdict("Retinue learned every flooded destination", bool(learned),
                  f"{len({a[0] for a in announces} & set(dests))}/{FLOOD}")
    ok &= verdict("the burst arrived at 1 and 2 hops", hops == {1, 2}, f"{sorted(hops)}")
    ok &= verdict("the first release is a fewest-hops entry", first is not None and first[2] == 1,
                  f"{first}")
    ok &= verdict("releases run fewest hops first", release_hops == sorted(release_hops),
                  f"{release_hops}")
    ok &= verdict("the quiet interface's announce was learned within 2 s, mid-burst",
                  bool(quiet_seen) and quiet_seen[0][1] != wire and quiet_lag is not None
                  and quiet_lag < 2.0, f"{quiet_seen[:1]} after {quiet_lag} s")
    ok &= verdict("stock in the same spot held or burst on the same traffic",
                  peak["held"] > 0 or peak["burst"], f"{peak}")
    print("=" * 72)
    print(f"{TITLE}: {'PASS' if ok else 'FAIL'}", flush=True)
    kill_all()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
