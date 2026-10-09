"""Auto sniff gate, no root: Retinue hears stock AutoInterface tokens on a real NIC.

Stock runs an AutoInterface on DEVICE (en1 on the reference host); Retinue's `auto_sniff`
joins the same discovery group beside it and must receive 3 tokens from the link-local it
adopts itself, each equal to its own `peering_token`. Stock's own adopted address must be
the same one. That proves the group address, port, canonical text and token live. The run
repeats with group `retinue-gate` at admin scope; the default run also has Retinue run a
whole AutoInterface under a private group and see its own multicast come back.

Both sides share the host's link-local, so they see each other as echoes and cannot peer:
that needs two links (interop_auto_feth.py). On macOS the application firewall must admit
incoming traffic for the oracle Python and the example binary, or no token arrives.

    .venv/bin/python -u interop_auto_sniff.py [DEVICE]
"""

from __future__ import annotations

import ipaddress
import sys

from datagram_gate import conclude, config_dir, free_udp_port, supervise
from pty_bridge import Retinue, rns_config

SCENARIOS = {"default": (None, None), "gate-admin": ("retinue-gate", "admin")}


def sniff(name: str, device: str) -> dict[str, bool]:
    import RNS

    group, scope = SCENARIOS[name]
    keys = (f"    group_id = {group}\n" if group else "") + (
        f"    discovery_scope = {scope}\n" if scope else "")
    rns_config(config_dir(), "  [[auto]]\n    type = AutoInterface\n    enabled = yes\n"
               f"    devices = {device}\n    data_port = {free_udp_port()}\n" + keys)
    RNS.Reticulum(configdir=str(config_dir()))
    stock = next(i for i in RNS.Transport.interfaces if "auto" in str(i))
    args = [device, "--count", "3"] + (["--group", group] if group else []) + (
        ["--scope", scope] if scope else []) + (["--self-test"] if name == "default" else [])
    retinue = Retinue("auto_sniff", "auto", args)
    try:
        adopted = retinue.wait_for(r"^ADOPTED (\S+) (\d+) (\S+) GROUP (\S+)", 300)
        results = {"retinue adopted the device": adopted is not None}
        if adopted is None:
            return results
        results["same link-local as stock adopted"] = (
            stock.adopted_interfaces.get(device) == adopted.group(3))
        results["same discovery group as stock"] = (
            adopted.group(4) == str(ipaddress.IPv6Address(stock.mcast_discovery_address)))
        ok = retinue.wait_for(r"^SNIFF_OK 3", 40) is not None
        results["3 stock tokens matched peering_token"] = ok
        if not ok and retinue.wait_for(r"^TOKEN ", 1) is None:
            print("no token arrived: is the macOS application firewall blocking it?", flush=True)
        if name == "default" and ok:
            echo = retinue.wait_for(r"^SELF_TEST echoed=(\w+) peers=(\d+)", 30)
            results["retinue's own AutoInterface heard its echo"] = bool(echo) and echo.group(1) == "true"
        return results
    finally:
        retinue.close()


def main() -> int:
    if sys.argv[1:2] == ["--child"]:
        return conclude(f"AUTO SNIFF {sys.argv[2]}", lambda: sniff(sys.argv[2], sys.argv[3]))
    device = sys.argv[1] if len(sys.argv) > 1 else "en1"
    return supervise(__file__, "AUTO SNIFF", "auto_sniff", [[n, device] for n in SCENARIOS])


if __name__ == "__main__":
    raise SystemExit(main())
