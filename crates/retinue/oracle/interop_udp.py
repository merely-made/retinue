"""UDP gate: stock RNS UDPInterface and Retinue's UDP carrier, judged on stock's state.

Loopback: stock listens on P1 and forwards to P2, Retinue the reverse. Stock must hold a
path to Retinue, bring a link ACTIVE, see its 64 KiB Resource COMPLETE and its link packet
DELIVERED, and receive Retinue's Resource back intact. The `ifac` run gives stock
`network_name`/`passphrase` with no `ifac_size`, proving the 16-byte datagram default.

`--lan DEVICE` (the datagram lane passes en1): stock runs `device = DEVICE` on port P, so it
binds the subnet broadcast; Retinue listens on 0.0.0.0:P with SO_REUSEADDR and forwards to
the same broadcast, hearing its own frames. Retinue asks once for a path to a stock
destination; stock must see exactly that one request, Retinue must count own echoes, and
Retinue must learn the path.

    .venv/bin/python -u interop_udp.py [--lan DEVICE]
"""

from __future__ import annotations

import sys
import time

from datagram_gate import (conclude, config_dir, exercise, free_udp_port, ifac_lines,
                           supervise, wait)
from pty_bridge import Retinue, rns_config

SCENARIOS = ("plain", "ifac")


def stock_udp(body: str) -> object:
    import RNS

    rns_config(config_dir(), "  [[udp]]\n    type = UDPInterface\n    enabled = yes\n" + body)
    RNS.Reticulum(configdir=str(config_dir()))
    return next(i for i in RNS.Transport.interfaces if "udp" in str(i))


def loopback(name: str) -> dict[str, bool]:
    p1, p2 = free_udp_port(), free_udp_port()
    args = [f"127.0.0.1:{p2}", f"127.0.0.1:{p1}"] + (["--ifac"] if name == "ifac" else [])
    retinue = Retinue("udp_interop", "auto", args)
    if retinue.wait_for(r"^ATTACHED", 300) is None:
        return {"retinue attached": False}
    try:
        iface = stock_udp(f"    listen_ip = 127.0.0.1\n    listen_port = {p1}\n"
                          f"    forward_ip = 127.0.0.1\n    forward_port = {p2}\n"
                          + ifac_lines(name == "ifac"))
        results = exercise(retinue)
        if name == "ifac":
            results["stock defaulted ifac_size to 16 bytes"] = iface.ifac_size == 16
        retinue.close()
        counters = retinue.wait_for(r"COUNTERS .*oversize=(\d+)", 5)
        results["retinue dropped no oversize frame"] = bool(counters) and counters.group(1) == "0"
        return results
    finally:
        retinue.close()


def lan(device: str) -> dict[str, bool]:
    import RNS
    from RNS.Interfaces import netinfo

    broadcast = netinfo.ifaddresses(device)[netinfo.AF_INET][0]["broadcast"]
    port = free_udp_port()
    stock_udp(f"    device = {device}\n    port = {port}\n")
    target = RNS.Destination(RNS.Identity(), RNS.Destination.IN, RNS.Destination.SINGLE,
                             "retinue", "udp-lan")
    requests = []
    inbound = RNS.Transport.inbound

    def counting(raw, interface=None):
        # Raw frames, before stock's duplicate filter would hide a repeated request.
        packet = RNS.Packet(None, raw)
        if (packet.unpack() and packet.destination_hash == RNS.Transport.path_request_destination.hash
                and packet.data[:16] == target.hash):
            requests.append(time.time())
        return inbound(raw, interface)

    RNS.Transport.inbound = counting
    retinue = Retinue("udp_interop", "auto", [f"0.0.0.0:{port}", f"{broadcast}:{port}",
                                              "--reuse", "--request", target.hash.hex()])
    try:
        results = {"retinue attached": retinue.wait_for(r"^ATTACHED", 300) is not None}
        results["stock saw Retinue's path request"] = wait(lambda: len(requests) > 0, 15)
        time.sleep(5)
        results[f"exactly one request reached stock ({len(requests)})"] = len(requests) == 1
        retinue.close()
        counters = retinue.wait_for(r"COUNTERS rx=(\d+) tx=(\d+) own_echo=(\d+)", 10)
        results["retinue dropped its own echoes"] = bool(counters) and int(counters.group(3)) > 0
        results["retinue learned the path"] = retinue.wait_for(r"^PATH_FOUND", 1) is not None
        return results
    finally:
        retinue.close()


def main() -> int:
    if sys.argv[1:2] == ["--child"]:
        name = sys.argv[2]
        return conclude(f"UDP {name}", lambda: lan(sys.argv[3]) if name == "lan" else loopback(name))
    runs = [[name] for name in SCENARIOS]
    if "--lan" in sys.argv:
        runs = [["lan", sys.argv[sys.argv.index("--lan") + 1]]]
    return supervise(__file__, "UDP INTEROP", "udp_interop", runs)


if __name__ == "__main__":
    raise SystemExit(main())
