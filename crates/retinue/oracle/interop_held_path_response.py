"""Held path-response gate: a path response is not held behind an announce burst.

Topology: a stock transport hub (ingress control off, so it relays the flood at once), a
stock target T that announces before Retinue attaches, and a stock flooder that announces
40 destinations in 2 s. Retinue attaches to the hub as a client, so the flood trips its
new-interface burst threshold, and then asks for T's path. RNS exempts a destination with a
pending path request from ingress holds for 45 s (`Transport.py` 1815-1821), so Retinue
must learn T within 5 s while announces are still held. The hub must have logged the
request and sent the path response.

Build: cargo build -p retinue --examples --all-features
Run:   python -u interop_held_path_response.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import time

from ingress_harness import Retinue, Stock, free_port, kill_all, tcp, verdict, wait_until

TITLE = "HELD PATH RESPONSE INTEROP"


def main() -> int:
    port = free_port()
    hub = Stock("hub", True, tcp("server", "hub", port, ingress_control="no"))
    target = Stock("target", False, tcp("client", "up", port))
    flood = Stock("flood", False, tcp("client", "up", port))
    for node in (target, flood):
        wait_until(lambda: node.call("online", names=["up"]), 20)
    t = target.call("dests", n=1, aspect="target")[0]
    target.call("announce", index=0)
    hub_knows = wait_until(lambda: hub.call("has_path", dest=t), 10)

    retinue = Retinue({"RETINUE_CONNECT": f"127.0.0.1:{port}"})
    wire = retinue.ifaces[0]
    time.sleep(1.0)
    flood_dests = flood.call("dests", n=40, aspect="flood")
    for index in range(len(flood_dests)):
        flood.call("announce", index=index)
        time.sleep(0.05)

    def holding():
        observed, held, released, _ = retinue.counters().get(wire, (0, 0, 0, 0))
        return held - released > 0 and (held, released)
    burst = wait_until(holding, 10, 0.05)

    asked = retinue.ask(f"request {t}", rf"REQUESTED {t} (true|false)")
    requested_at = time.monotonic()
    learned = wait_until(lambda: retinue.route(t), 5, 0.05)
    elapsed = time.monotonic() - requested_at
    still_holding = holding()
    log = wait_until(lambda: (lambda l: t in l["responses"] and l)(hub.call("path_log")), 5) \
        or hub.call("path_log")

    print("\n" + "=" * 72)
    ok = verdict("the hub learned T before Retinue attached", bool(hub_knows))
    ok &= verdict("the flood is held by Retinue", bool(burst), f"(held, released) = {burst}")
    ok &= verdict("Retinue sent the path request", bool(asked) and asked[0].group(1) == "true")
    ok &= verdict("Retinue learned T within 5 s", bool(learned) and elapsed <= 5.0,
                  f"{elapsed:.2f} s, route {learned}")
    ok &= verdict("announces were still held when it did", bool(still_holding), f"{still_holding}")
    ok &= verdict("the hub logged the path request", t in log["requests"])
    ok &= verdict("the hub sent the path response", t in log["responses"])
    print("=" * 72)
    print(f"{TITLE}: {'PASS' if ok else 'FAIL'}", flush=True)
    kill_all()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
