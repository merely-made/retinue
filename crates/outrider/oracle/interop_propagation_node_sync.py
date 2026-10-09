"""Three stock submissions arrive at a stock receiver in one sync from Outrider's node.

Judged by stock state: each sender message reaches SENT, the receiver is offered and
delivered all three in one sync, the node's store empties on the acknowledgement, and a
second sync on the same link is offered nothing.
"""

from __future__ import annotations

import LXMF

from stock_node_harness import StockNodeHarness, verdict, wait

COUNT = 3


def main() -> int:
    harness = StockNodeHarness({"OUTRIDER_PROPAGATION_COST": "8"})
    exit_code = 1
    try:
        if not harness.learn_node():
            return 1
        messages = [harness.send(f"body {index}".encode() * (index + 1), f"title {index}".encode()) for index in range(COUNT)]
        sent = wait(lambda: all(message.state == LXMF.LXMessage.SENT for message in messages), 180)
        stored = wait(lambda: harness.store_entries() == COUNT, 10)
        first = harness.sync(90)
        first_result = harness.receiver.propagation_transfer_last_result
        delivered = [bytes(message.hash) for message in harness.delivered]
        emptied = wait(lambda: harness.store_entries() == 0, 20)
        second = harness.sync(30)

        ok = all(
            [
                verdict("stock senders reached SENT", sent),
                verdict(f"Outrider stored {COUNT} entries", stored),
                verdict(f"one stock sync was offered {COUNT}", first is not None and len(first) == COUNT),
                verdict(
                    f"one stock sync delivered all {COUNT}",
                    first_result == COUNT
                    and sorted(delivered) == sorted(bytes(message.hash) for message in messages),
                ),
                verdict("acknowledgement emptied the store", emptied),
                verdict("second stock sync offered nothing", second == []),
            ]
        )
        print(f"OUTRIDER_PROPAGATION_NODE_SYNC: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        harness.close(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
