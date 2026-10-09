"""A stock client's submission with a bad propagation stamp ends REJECTED at Outrider's node.

The stock sender mints a stamp under the node's floor (cost minus flexibility). The node
must answer the link packet with 0xf5, which stock maps to REJECTED, and store nothing.
Stock's packet timeout may then overwrite REJECTED with OUTBOUND (`LXMessage.py` 623-628),
so the gate records every state stock assigns rather than sampling the last one.
"""

from __future__ import annotations

import LXMF
import LXMF.LXStamper as LXStamper

from stock_node_harness import StockNodeHarness, record_states, verdict, wait

COST, FLEXIBILITY = 8, 3


def under_floor_stamp(self, target_cost, timeout=None):
    """Stand in for stock's minting with a stamp worth less than the node's floor."""
    if self.propagation_stamp is None:
        if not self.transient_id:
            self.pack()
        workblock = LXStamper.stamp_workblock(self.transient_id, expand_rounds=LXStamper.WORKBLOCK_EXPAND_ROUNDS_PN)
        nonce = 0
        while LXStamper.stamp_value(workblock, nonce.to_bytes(32, "big")) >= COST - FLEXIBILITY:
            nonce += 1
        self.propagation_stamp = nonce.to_bytes(32, "big")
    return self.propagation_stamp


def main() -> int:
    states: list[tuple[int, int]] = []
    record_states(states)
    harness = StockNodeHarness({"OUTRIDER_PROPAGATION_COST": str(COST)})
    exit_code = 1
    try:
        if not harness.learn_node():
            return 1
        LXMF.LXMessage.get_propagation_stamp = under_floor_stamp
        message = harness.send(b"under-stamped body", b"under-stamped")
        rejected = wait(lambda: (id(message), LXMF.LXMessage.REJECTED) in states, 60)
        closed = wait(lambda: any(line.startswith("LINK_CLOSED ") and " rejected=1 " in line for line in harness.lines), 10)
        empty = harness.store_entries() == 0
        ok = all(
            [
                verdict("stock sender saw REJECTED", rejected),
                verdict("Outrider refused the entry and closed the link", closed),
                verdict("Outrider stored nothing", empty),
            ]
        )
        print(f"OUTRIDER_PROPAGATION_NODE_REJECT: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        harness.close(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
