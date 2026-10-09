"""Stock LXMF sends opportunistically to an Outrider destination that advertises no ratchet.

Stock then encrypts to the identity key; Outrider accepts it, as stock does unless ratchets
are enforced, and proves it. Passes only when stock records the message as DELIVERED.
"""

import os

os.environ["OUTRIDER_NO_RATCHETS"] = "1"

from interop_opportunistic_send import main  # noqa: E402

if __name__ == "__main__":
    raise SystemExit(main())
