"""A stock submission larger than Outrider's store admits never reaches SENT.

Judged by stock state: the node announces and enforces its store's limit, so a message
over it is refused at its Resource advertisement, or left unproven on the packet path,
rather than proven and dropped. The stock sender never sees SENT, the node stores
nothing, and a stock sync is offered nothing; a message within the limit still lands.
"""

from __future__ import annotations

import time

import LXMF

from stock_node_harness import StockNodeHarness, record_states, verdict, wait

# Bodies over the default store's 240-byte message limit: one fits a link packet, one
# needs a Resource.
OVERSIZE = {"packet": 60, "resource": 600}
WATCH = 45


def main() -> int:
    states: list[tuple[int, int]] = []
    record_states(states)
    harness = StockNodeHarness({"OUTRIDER_PROPAGATION_COST": "8"})
    exit_code = 1
    try:
        if not harness.learn_node():
            return 1
        oversized = {path: harness.send(b"x" * size, path.encode()) for path, size in OVERSIZE.items()}
        time.sleep(WATCH)
        seen = {
            path: {state for target, state in states if target == id(message)}
            for path, message in oversized.items()
        }
        attempted = all(message.delivery_attempts >= 1 for message in oversized.values())
        representation = {"packet": LXMF.LXMessage.PACKET, "resource": LXMF.LXMessage.RESOURCE}
        carried = all(oversized[path].representation == kind for path, kind in representation.items())
        never_sent = {
            path: not assigned & {LXMF.LXMessage.SENT, LXMF.LXMessage.DELIVERED} for path, assigned in seen.items()
        }
        empty = harness.store_entries() == 0
        offered = harness.sync(60)

        small = harness.send(b"within the limit", b"small")
        small_sent = wait(lambda: small.state == LXMF.LXMessage.SENT, 90)
        stored = wait(lambda: harness.store_entries() == 1, 10)

        ok = all(
            [
                verdict("stock attempted both oversized messages", attempted),
                verdict("stock carried one as a packet and one as a Resource", carried),
                *(
                    verdict(f"oversized {path} submission never reached SENT", never_sent[path])
                    for path in OVERSIZE
                ),
                verdict("Outrider stored nothing oversized", empty),
                verdict("stock sync was offered nothing", offered == []),
                verdict("a message within the limit reached SENT", small_sent),
                verdict("Outrider stored it", stored),
            ]
        )
        print(f"states assigned: { {path: sorted(assigned) for path, assigned in seen.items()} }")
        print(f"OUTRIDER_PROPAGATION_NODE_OVERSIZE: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        harness.close(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
