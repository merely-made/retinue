"""Paper messages (lxm:// URIs) between pinned stock LXMF and Outrider.

Outrider writes a URI and stock's ingest_lxm_uri must deliver it, signature
validated, exactly once: a second ingest of the same URI must be refused as a
duplicate. Stock writes a URI (its message reaching the PAPER state) and Outrider
must read, decrypt and verify it, with the scheme in upper case.

Covers identity-key paper only: neither side has heard the other announce a
ratchet. Outrider opens stock's URI through its registered delivery destination,
the path fetched propagated messages take. Ratchet-sealed paper is covered by
crates/outrider/tests/paper.rs.
"""

from __future__ import annotations

import atexit
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

import LXMF
import RNS
from LXMF import LXMessage


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
SEED = bytes([0x7a] * 64)
TITLE, CONTENT = "STOCK PAPER", "printed and scanned"


def outrider(*args: str) -> dict[str, str]:
    command = ["cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_paper", "--", *args]
    result = subprocess.run(command, cwd=REPO, capture_output=True, text=True, timeout=300)
    for line in (result.stdout + result.stderr).splitlines():
        print(f"  [outrider] {line}")
    return dict(line.split(" ", 1) for line in result.stdout.splitlines() if " " in line)


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    config = Path(tempfile.mkdtemp(prefix="outrider-paper-"))
    (config / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=4\n\n[interfaces]\n",
        encoding="utf-8",
    )
    exit_code = 1
    RNS.Reticulum(configdir=str(config))
    try:
        identity = RNS.Identity.from_bytes(SEED)
        router = LXMF.LXMRouter(identity=identity, storagepath=str(config))
        source = router.register_delivery_identity(identity, display_name="Stock Paper")
        delivered: list[LXMessage] = []
        router.register_delivery_callback(delivered.append)

        # Outrider to stock.
        written = outrider("encode", identity.get_public_key().hex())
        sender_key = bytes.fromhex(written["PUBLIC"])
        sender_hash = bytes.fromhex(written["DESTINATION"])
        RNS.Identity.remember(os.urandom(32), sender_hash, sender_key)
        first = router.ingest_lxm_uri(written["URI"], signal_local_delivery="delivered", signal_duplicate="duplicate")
        again = router.ingest_lxm_uri(written["URI"], signal_local_delivery="delivered", signal_duplicate="duplicate")
        message = delivered[0] if delivered else None
        ingest_ok = (
            first == "delivered"
            and message is not None
            and message.hash.hex() == written["MESSAGE_ID"]
            and message.signature_validated
            and message.title == b"OUTRIDER PAPER"
            and message.content == b"written by hand"
        )
        duplicate_ok = again == "duplicate" and len(delivered) == 1

        # Stock to Outrider.
        recipient = RNS.Identity(create_keys=False)
        recipient.load_public_key(sender_key)
        destination = RNS.Destination(recipient, RNS.Destination.OUT, RNS.Destination.SINGLE, "lxmf", "delivery")
        paper = LXMessage(destination, source, CONTENT, title=TITLE, desired_method=LXMessage.PAPER)
        uri = paper.as_uri()
        read = outrider("decode", "LXM://" + uri[len("lxm://") :], identity.get_public_key().hex())
        read_ok = (
            paper.state == LXMessage.PAPER
            and read.get("MESSAGE_ID") == paper.hash.hex()
            and read.get("TITLE") == TITLE.encode().hex()
            and read.get("CONTENT") == CONTENT.encode().hex()
            and read.get("VERIFIED") == "true"
        )

        ok = ingest_ok and duplicate_ok and read_ok
        print(f"stock ingested and validated Outrider's URI: {'PASS' if ingest_ok else 'FAIL'}")
        print(f"stock refused the same URI again: {'PASS' if duplicate_ok else 'FAIL'}")
        print(f"Outrider read and verified stock's URI: {'PASS' if read_ok else 'FAIL'}")
        print(f"STOCK_OUTRIDER_PAPER: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
