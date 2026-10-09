"""IFAC default-size gate: a stock TCPServerInterface with no `ifac_size` line.

Stock TCP interfaces default to 16-byte codes (TCPInterface.py 466, Reticulum.py
1041-1042), so Retinue must join with `Ifac::for_stream`:
  * `stream`: stock `RNS.Transport.has_path(retinue_dest)` turns true, and Retinue
    validates both stock announces, one with 330 bytes of app data (497 logical bytes,
    513 on the wire: past the bare 500-byte MTU);
  * `serial` (negative): Retinue attaches with 8-byte codes and stock receives its frames
    (server rxb > 0), yet has_path stays false for 10 s and Retinue validates nothing.

Run: python -u interop_ifac_default.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import time

from interop_ifac_resource import SEED, Example, free_port, start_stock_server, supervise
from library_gate import payload, verdict

TITLE = "IFAC DEFAULT SIZE INTEROP"
RETINUE_SEED = bytes([0x4E] * 64)  # examples/ifac_resource.rs
RNS_SEED = bytes([0x5E] * 64)
LARGE_APP_DATA = 330
NEGATIVE_HOLD = 10.0
CODE_SIZE = {"stream": 16, "serial": 8}
STOCK_OK = r"RNS_ANNOUNCE_OK \S+"
LARGE_OK = f"RNS_LARGE_ANNOUNCE {LARGE_APP_DATA} OK"
ANY_RNS = r"RNS_(LARGE_)?ANNOUNCE.*"


def peer(size: str) -> int:
    import RNS

    port = free_port()
    _, server = start_stock_server(port, None)
    expect_path = size == "stream"
    hold = 25 if expect_path else int(NEGATIVE_HOLD) + 3
    retinue = Example(["announce", str(port), size, str(hold)])
    exit_code = 1
    try:
        retinue_dest = RNS.Destination.hash(RNS.Identity.from_bytes(RETINUE_SEED), "retinue", "ifac-default")
        identity = RNS.Identity.from_bytes(RNS_SEED)
        stock, large = (RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE, "retinue", aspect)
                        for aspect in ("ifac_stock", "ifac_large"))
        large_data = payload(LARGE_APP_DATA, SEED + 3)
        attached = retinue.wait_for("ATTACHED", 30) is not None
        started = time.monotonic()
        window = 20.0 if expect_path else NEGATIVE_HOLD
        while attached and time.monotonic() - started < window:
            stock.announce()
            large.announce(app_data=large_data)
            if expect_path and RNS.Transport.has_path(retinue_dest) \
                    and retinue.find(STOCK_OK) and retinue.find(LARGE_OK):
                break
            time.sleep(1.0)
        has_path = RNS.Transport.has_path(retinue_dest)
        received = server.rxb
        process_code = retinue.finish(hold + 15)

        print("\n" + "=" * 72)
        print(f"Retinue Ifac::for_{size}; stock server ifac_size {server.ifac_size} bytes, rxb {received}")
        ok = verdict("stock default ifac_size is 16 bytes", server.ifac_size == 16)
        ok &= verdict("Retinue attached", attached)
        ok &= verdict(f"Retinue sealed with {CODE_SIZE[size]}-byte codes",
                      retinue.find(f"IFAC_SIZE {CODE_SIZE[size]}") is not None)
        ok &= verdict("stock received Retinue's frames", received > 0, f"{received} bytes")
        if expect_path:
            ok &= verdict("stock has_path(retinue) true", has_path)
            ok &= verdict("Retinue validated stock's announce", retinue.find(STOCK_OK) is not None)
            ok &= verdict(f"Retinue validated the {LARGE_APP_DATA}-byte app data announce",
                          retinue.find(LARGE_OK) is not None)
        else:
            ok &= verdict(f"stock has_path(retinue) false after {NEGATIVE_HOLD:.0f} s", not has_path)
            ok &= verdict("Retinue never validated a stock announce", retinue.find(ANY_RNS) is None)
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
