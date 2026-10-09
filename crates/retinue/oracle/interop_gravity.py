"""Gravity gate: the same emission heard later on a higher-gravity interface takes the path.

A stock announcer reaches Retinue through two stock transports, X1 and X2, on Retinue
interfaces of gravity 0 and 5. A stock node in the same spot has the same two interfaces and
gravities. The announcer sends one announce through X1 only, then the identical packet
through X2, so both arrive at 1 hop, gravity-0 side first. RNS moves the path to the
higher-gravity interface (`Transport.py` 2229-2251); Retinue must do the same.

Build: cargo build -p retinue --examples --all-features
Run:   python -u interop_gravity.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import time

from ingress_harness import Retinue, Stock, free_port, kill_all, tcp, verdict, wait_until

TITLE = "GRAVITY INTEROP"


def main() -> int:
    p1, p2 = free_port(), free_port()
    x1 = Stock("x1", True, tcp("server", "x1", p1))
    x2 = Stock("x2", True, tcp("server", "x2", p2))
    retinue = Retinue({"RETINUE_CONNECT": f"127.0.0.1:{p1},127.0.0.1:{p2}", "RETINUE_GRAVITY": "0,5"})
    stock = Stock("stock", False, tcp("client", "x1", p1) + tcp("client", "x2", p2, gravity=5))
    announcer = Stock("announcer", False, tcp("client", "x1", p1) + tcp("client", "x2", p2))
    for node in (stock, announcer):
        wait_until(lambda: node.call("online", names=["x1", "x2"]), 20)
    time.sleep(1.0)
    low, high = retinue.ifaces[0], retinue.ifaces[1]

    dest = announcer.call("dests", n=1, aspect="gravity")[0]
    announcer.call("announce_hold", index=0, iface="x1")
    first_retinue = wait_until(lambda: retinue.route(dest), 10)
    first_stock = wait_until(lambda: stock.call("next_hop", dest=dest), 10)
    time.sleep(1.0)
    announcer.call("resend_held", iface="x2")
    final_retinue = wait_until(lambda: (lambda r: r if r and r[0] == high else None)(retinue.route(dest)), 10) \
        or retinue.route(dest)
    final_stock = wait_until(lambda: (lambda h: h if h == "x2" else None)(stock.call("next_hop", dest=dest)), 10) \
        or stock.call("next_hop", dest=dest)
    published = len(retinue.matches(rf"ANNOUNCE {dest} .*"))

    print("\n" + "=" * 72)
    ok = verdict("first heard on the gravity-0 side", first_retinue == (low, 1) and first_stock == "x1",
                 f"retinue {first_retinue}, stock {first_stock}")
    ok &= verdict("stock moved the path to the gravity-5 interface", final_stock == "x2", f"{final_stock}")
    ok &= verdict("Retinue moved its route there too", final_retinue == (high, 1), f"{final_retinue}")
    ok &= verdict("the repeat was not published again", published == 1, f"{published}")
    print("=" * 72)
    print(f"{TITLE}: {'PASS' if ok else 'FAIL'}", flush=True)
    kill_all()
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
