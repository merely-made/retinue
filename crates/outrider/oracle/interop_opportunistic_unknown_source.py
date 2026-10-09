"""Stock LXMF sends opportunistically to Outrider before Outrider has heard it announce.

The first copy cannot be verified, so Outrider must leave it unproved; the path request it
sends brings the sender's announce, and stock's retry verifies and is proved. Passes only when
stock records the message as DELIVERED and Outrider delivered it once.
"""

import os

os.environ["OUTRIDER_SILENT_SENDER"] = "1"

from interop_opportunistic_send import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
