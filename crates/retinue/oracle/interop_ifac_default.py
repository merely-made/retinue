"""IFAC default-size gate: a stock TCPServerInterface with no `ifac_size` line.

Stock TCP interfaces default to 16-byte codes (TCPInterface.py 466, Reticulum.py
1041-1042), so Retinue must join with `Ifac::for_stream`:
  * `stream`: stock `RNS.Transport.has_path(retinue_dest)` turns true, and Retinue
    validates stock's announce;
  * `serial` (negative): with 8-byte codes, has_path stays false for 10 s and Retinue
    never validates stock's announce.

Run: python -u interop_ifac_default.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import time

from interop_ifac_resource import Example, free_port, start_stock_server, supervise
from library_gate import verdict

TITLE = "IFAC DEFAULT SIZE INTEROP"
RETINUE_SEED = bytes([0x4E] * 64)  # examples/ifac_resource.rs
RNS_SEED = bytes([0x5E] * 64)
NEGATIVE_HOLD = 10.0


def peer(size: str) -> int:
    import RNS

    port = free_port()
    _, server = start_stock_server(port, None)
    expect_path = size == "stream"
    hold = 20 if expect_path else int(NEGATIVE_HOLD) + 3
    retinue = Example(["announce", str(port), size, str(hold)])
    exit_code = 1
    try:
        retinue_dest = RNS.Destination.hash(RNS.Identity.from_bytes(RETINUE_SEED), "retinue", "ifac-default")
        stock = RNS.Destination(RNS.Identity.from_bytes(RNS_SEED), RNS.Destination.IN,
                                RNS.Destination.SINGLE, "retinue", "ifac_stock")
        retinue.wait_for("ATTACHED", 30)
        started = time.monotonic()
        window = 20.0 if expect_path else NEGATIVE_HOLD
        while time.monotonic() - started < window:
            stock.announce()
            if RNS.Transport.has_path(retinue_dest) and expect_path:
                break
            time.sleep(1.0)
        has_path = RNS.Transport.has_path(retinue_dest)
        if expect_path:
            retinue.wait_for(r"RNS_ANNOUNCE_OK \S+", 10)
        process_code = retinue.finish(hold + 15)
        got_announce = retinue.find(r"RNS_ANNOUNCE_OK \S+") is not None

        print("\n" + "=" * 72)
        print(f"Retinue Ifac::for_{size}; stock server ifac_size {server.ifac_size} bytes")
        ok = verdict("stock default ifac_size is 16 bytes", server.ifac_size == 16)
        if expect_path:
            ok &= verdict("stock has_path(retinue) true", has_path)
            ok &= verdict("Retinue validated stock's announce", got_announce)
        else:
            ok &= verdict(f"stock has_path(retinue) false after {NEGATIVE_HOLD:.0f} s", not has_path)
            ok &= verdict("Retinue never validated stock's announce", not got_announce)
        ok &= verdict("Retinue process exited zero", process_code == 0, str(process_code))
        print("=" * 72, flush=True)
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        retinue.finish(1)
        RNS.exit(exit_code)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--peer"]:
        raise SystemExit(peer(sys.argv[2]))
    raise SystemExit(supervise(__file__, TITLE, [["stream"], ["serial"]], 120))
