"""Announce-rate gate: Retinue and a stock transport relay a chatty destination alike.

A stock announcer re-announces one destination 8 times, 2 s apart, to a Retinue transport
and to a stock transport. Each transport has its own stock observer, which counts the
distinct emissions it hears relayed by that transport. With RNS's transport defaults
(target 3600 s, grace 5, penalty 0; `Transport.py` 2298-2338, `Reticulum.py` 968-971) both
relay the first announce and five graces, 6 of 8, and both observers keep the path, since a
blocked announce is still learned.

Build: cargo build -p retinue --examples --all-features
Run:   python -u interop_announce_rate.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import time

from ingress_harness import Retinue, Stock, free_port, kill_all, tcp, verdict, wait_until

TITLE = "ANNOUNCE RATE INTEROP"
ANNOUNCES, SPACING_S, EXPECTED = 8, 2.0, 6


def main() -> int:
    stock_port = free_port()
    stock = Stock("transport", True, tcp("server", "transport", stock_port))
    retinue = Retinue({"RETINUE_LISTEN": "0", "RETINUE_ROUTING": "1"})
    observers = {
        "retinue": (Stock("observer-r", False, tcp("client", "up", retinue.port)), retinue.identity),
        "stock": (Stock("observer-s", False, tcp("client", "up", stock_port)), stock.transport_id),
    }
    announcer = Stock("announcer", False, tcp("client", "retinue", retinue.port)
                      + tcp("client", "stock", stock_port))
    for node, names in [(o, ["up"]) for o, _ in observers.values()] + [(announcer, ["retinue", "stock"])]:
        wait_until(lambda: node.call("online", names=names), 20)
    time.sleep(1.0)

    dest = announcer.call("dests", n=1, aspect="chatty")[0]
    for _ in range(ANNOUNCES):
        announcer.call("announce", index=0)
        time.sleep(SPACING_S)
    time.sleep(8.0)  # past any relay retry

    print("\n" + "=" * 72)
    ok = True
    for name, (observer, transport) in observers.items():
        relayed = observer.call("relayed", dest=dest, transport=transport)
        has_path = observer.call("has_path", dest=dest)
        ok &= verdict(f"{name}: relayed {EXPECTED} of {ANNOUNCES} emissions", len(relayed) == EXPECTED,
                      f"{len(relayed)}")
        ok &= verdict(f"{name}: the observer holds the path", bool(has_path))
    print("=" * 72)
    print(f"{TITLE}: {'PASS' if ok else 'FAIL'}", flush=True)
    kill_all()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
