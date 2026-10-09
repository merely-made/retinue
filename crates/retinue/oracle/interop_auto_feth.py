"""Auto peering gate over a feth pair: opt-in, the pair needs root to create.

Stock and Retinue need distinct link-locals on one link, which one host has only through
a fake-ethernet pair. Once, as root:

    ifconfig feth0 create; ifconfig feth1 create; ifconfig feth0 peer feth1
    ifconfig feth0 inet6 fe80::a1%feth0 prefixlen 64 up
    ifconfig feth1 inet6 fe80::b1%feth1 prefixlen 64 up

and for the `mif` run a second pair the same way: feth2/feth3, fe80::a2 and fe80::b2.
Stock runs on feth0 (and feth2), Retinue's `auto_peer` on feth1 (and feth3). Each run is
judged on stock's state:

  * plain: stock's `AutoInterface.peers` holds Retinue within 4 s, then the datagram
    script (path, link ACTIVE, Resource COMPLETE, packet DELIVERED, Resource back); after
    Retinue is killed stock drops the peer 20-27 s later (22 s timeout, 4 s job);
  * unicast: Retinue never multicasts, so stock learns it only from Retinue's unicast
    answer to stock's multicast (reverse peering);
  * ifac: both sides with `network_name`/`passphrase` and stock's default size of 16;
  * mif: two links, so each side sees the other twice; Retinue must drop duplicates
    (`mif_duplicates` > 0) and the script must still pass.

The script refuses to run, failing, unless the pairs exist. The macOS application
firewall must admit the oracle Python and the example binary.

    .venv/bin/python -u interop_auto_feth.py [plain|unicast|ifac|mif]
"""

from __future__ import annotations

import ipaddress
import subprocess
import sys
import time

from datagram_gate import conclude, config_dir, exercise, ifac_lines, supervise, wait
from pty_bridge import Retinue, rns_config

SCENARIOS = ("plain", "unicast", "ifac", "mif")
STOCK, RETINUE = ("feth0", "feth2"), ("feth1", "feth3")


def link_local(device: str) -> str | None:
    out = subprocess.run(["ifconfig", device], capture_output=True, text=True).stdout
    found = [line.split()[1].split("%")[0] for line in out.splitlines()
             if line.strip().startswith("inet6 fe80:")]
    return str(ipaddress.IPv6Address(found[-1])) if found else None


def peered(stock, addr: str) -> bool:
    return addr in dict(stock.peers)


def run(name: str) -> dict[str, bool]:
    import RNS

    pairs = 2 if name == "mif" else 1
    stock_devs, retinue_devs = STOCK[:pairs], RETINUE[:pairs]
    retinue_lls = [link_local(d) for d in retinue_devs]
    rns_config(config_dir(), "  [[auto]]\n    type = AutoInterface\n    enabled = yes\n"
               f"    devices = {', '.join(stock_devs)}\n" + ifac_lines(name == "ifac"))
    RNS.Reticulum(configdir=str(config_dir()))
    stock = next(i for i in RNS.Transport.interfaces if "auto" in str(i))
    args = [",".join(retinue_devs)] + (["--ifac"] if name == "ifac" else []) + (
        ["--no-multicast"] if name == "unicast" else [])
    retinue = Retinue("auto_peer", "auto", args)
    try:
        results = {"retinue adopted": retinue.wait_for(r"^DEST ", 300) is not None}
        start = time.time()
        within = 4 if name != "unicast" else 12
        results[f"stock peers hold retinue within {within} s"] = wait(
            lambda: all(peered(stock, ll) for ll in retinue_lls), within)
        print(f"  stock peered in {time.time() - start:.1f} s: {list(dict(stock.peers))}", flush=True)
        if name == "ifac":
            results["stock defaulted ifac_size to 16 bytes"] = stock.ifac_size == 16
        results.update(exercise(retinue))
        if name == "mif":
            retinue.close()
            counters = retinue.wait_for(r"COUNTERS .*mif_duplicates=(\d+)", 10)
            results["retinue dropped multi-link duplicates"] = bool(counters) and int(counters.group(1)) > 0
        if name == "plain":
            retinue.proc.kill()
            killed = time.time()
            wait(lambda: not peered(stock, retinue_lls[0]), 35)
            gone = time.time() - killed
            results[f"stock dropped the dead peer after {gone:.1f} s (20-27)"] = 20 <= gone <= 27
        return results
    finally:
        retinue.close()


def main() -> int:
    if sys.argv[1:2] == ["--child"]:
        return conclude(f"AUTO FETH {sys.argv[2]}", lambda: run(sys.argv[2]))
    names = [a for a in sys.argv[1:] if a in SCENARIOS] or list(SCENARIOS)
    needed = STOCK + RETINUE if "mif" in names else STOCK[:1] + RETINUE[:1]
    missing = [d for d in needed if link_local(d) is None]
    if missing:
        print(f"AUTO FETH: REFUSED, no link-local on {', '.join(missing)}; create the pairs "
              "as root first (see this script's docstring)", flush=True)
        return 2
    return supervise(__file__, "AUTO FETH", "auto_peer", [[n] for n in names])


if __name__ == "__main__":
    raise SystemExit(main())
