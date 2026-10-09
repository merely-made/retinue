"""Prove pinned stock LXMF submits to and syncs from Outrider's node, judged by stock state.

The stock sender's message must reach SENT (the node proved or completed it), the stock
receiver must get it in one sync, and a second sync on the same link must be offered
nothing: the node honoured the delete acknowledgement.
"""

from __future__ import annotations

import os

import LXMF

from stock_node_harness import StockNodeHarness, verdict, wait

TITLE = b"PROPAGATION TITLE"
LARGE = os.environ.get("OUTRIDER_LARGE") == "1"
CONTENT = bytes((value * 73 + 19) & 0xFF for value in range(4096)) if LARGE else b"PROPAGATION BODY"


def main() -> int:
    harness = StockNodeHarness()
    exit_code = 1
    try:
        if not harness.learn_node():
            return 1
        message = harness.send(CONTENT, TITLE)
        sent = wait(lambda: message.state == LXMF.LXMessage.SENT, 180 if LARGE else 60)
        stored = wait(lambda: harness.store_entries() == 1, 10)
        first = harness.sync(120 if LARGE else 60)
        got = harness.delivered[0] if harness.delivered else None
        emptied = wait(lambda: harness.store_entries() == 0, 20)
        second = harness.sync(30)

        ok = all(
            [
                verdict("stock sender reached SENT", sent),
                verdict("Outrider stored one entry", stored),
                verdict("stock sync offered and delivered one", first is not None and len(first) == 1 and len(harness.delivered) == 1),
                verdict(
                    "stock decoded title/body/id",
                    got is not None and got.title == TITLE and got.content == CONTENT and got.hash == message.hash,
                ),
                verdict("acknowledgement emptied the store", emptied),
                verdict("second stock sync offered nothing", second == []),
            ]
        )
        print(f"OUTRIDER_PROPAGATION_SERVER: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        harness.close(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
