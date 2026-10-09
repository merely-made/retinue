"""Serial gate: stock RNS SerialInterface and Retinue's HDLC serial carrier on a pty pair.

Stock runs `SerialInterface` at 115200 on one pty with transport on; Retinue's
`serial_peer` opens the other; a null-modem bridge joins the masters. Judged on stock's
state: has_path to Retinue, an ACTIVE link, a request answered, and a multi-part Resource
COMPLETE. The run repeats with `ifac_size = 64`.

`--flood` adds the integration check that needs the per-interface ingress work: stock
announces 20 destinations within 2 s and Retinue must validate them all, holding none.

    .venv/bin/python -u interop_serial_hdlc.py [--flood]
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from pty_bridge import Bridge, Retinue, exercise_stock, open_pty, report, rns_config

SCENARIOS = {"plain": None, "ifac-64": 64}


def scenario(name: str, flood: bool) -> int:
    import RNS

    ifac_bits = SCENARIOS[name]
    stock_master, stock_port = open_pty()
    retinue_master, retinue_port = open_pty()
    Bridge(stock_master, retinue_master)

    args = [retinue_port]
    if ifac_bits:
        args += ["--ifac", str(ifac_bits)]
    retinue = Retinue("serial_peer", "serial", args)
    if retinue.wait_for(r"^ATTACHED", 300) is None:
        print("retinue never attached", file=sys.stderr)
        return 1

    config_dir = Path(tempfile.mkdtemp(prefix="retinue-serial-hdlc-"))
    access = (f"    network_name = retinue-serial-gate\n    passphrase = serial-and-radio\n"
              f"    ifac_size = {ifac_bits}\n") if ifac_bits else ""
    rns_config(config_dir, "  [[serial]]\n    type = SerialInterface\n    enabled = yes\n"
               f"    port = {stock_port}\n    speed = 115200\n" + access, transport=True)
    RNS.Reticulum(configdir=str(config_dir))
    try:
        results = exercise_stock(retinue)
        if flood:
            identities = [RNS.Identity() for _ in range(20)]
            start = time.time()
            for identity in identities:
                RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                                "retinue", "flood").announce()
            results["stock flooded 20 announces within 2 s"] = time.time() - start < 2
            time.sleep(5)
        retinue.close()
        match = retinue.wait_for(r"ANNOUNCES (\d+) HELD (\d+)", 5)
        if flood:
            results["retinue validated all 20, held none"] = (
                match is not None and int(match.group(1)) >= 20 and match.group(2) == "0")
        return report(f"SERIAL HDLC {name}", results)
    finally:
        retinue.close()
        shutil.rmtree(config_dir, ignore_errors=True)
        RNS.exit(0)


def main() -> int:
    flood = "--flood" in sys.argv
    if len(sys.argv) > 1 and sys.argv[1] in SCENARIOS:
        return scenario(sys.argv[1], flood)
    codes = []
    for name in SCENARIOS:
        extra = ["--flood"] if flood else []
        codes.append(subprocess.run([sys.executable, "-u", __file__, name, *extra]).returncode)
    ok = not any(codes)
    print(f"SERIAL HDLC INTEROP: {'PASS' if ok else 'FAIL'}")
    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
