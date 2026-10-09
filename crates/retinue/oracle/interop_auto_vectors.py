"""Offline AutoInterface vectors from stock RNS 1.5.7: no network, no root.

Builds stock `AutoInterface` objects with `devices = none0`, so nothing is adopted and no
socket opens, and records what retinue's `auto` module must reproduce byte for byte:

  * the discovery group address for 5 scopes x 2 address types x 3 group ids;
  * `descope_linklocal` for the zone, KAME and long-zero-run cases;
  * the canonical text and peering token for a spread of link-local addresses.

Writes tests/fixtures/auto_vectors.json; `auto::tests::stock_vectors` asserts it. With
`--check` it compares against the committed file instead and fails on any difference.

    .venv/bin/python -u interop_auto_vectors.py [--check]
"""

from __future__ import annotations

import ipaddress
import json
import random
import shutil
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURE = HERE.parent / "tests" / "fixtures" / "auto_vectors.json"
GROUPS = ("reticulum", "retinue-gate", "")
SCOPES = ("link", "admin", "site", "organisation", "global")
TYPES = ("temporary", "permanent")
DESCOPE = ("fe80::1%en0", "fe80:4::aede:48ff:fe00:1122", "fe80:4:0:0:1::",
           "fe80::14ec:5af8:c218:4d4e")


def stock(group: str | None = None, scope: str | None = None, kind: str | None = None):
    from RNS.Interfaces.AutoInterface import AutoInterface

    config = {"name": "vectors", "devices": "none0"}
    for key, value in (("group_id", group), ("discovery_scope", scope),
                       ("multicast_address_type", kind)):
        if value is not None:
            config[key] = value
    return AutoInterface(None, config)


def link_locals() -> list[str]:
    """Link-locals with zero runs in every position, as Python's ipaddress prints them."""
    rng = random.Random(0xA070)
    out = ["fe80::1", "fe80::aede:48ff:fe00:1122", "fe80::1:0:0:0", "fe80::1:0:0:1"]
    while len(out) < 24:
        tail = [rng.choice((0, 0, rng.randrange(1, 0x10000))) for _ in range(4)]
        out.append(str(ipaddress.IPv6Address(f"fe80::{':'.join(f'{h:x}' for h in tail)}")))
    return out


def vectors() -> dict:
    import RNS

    assert RNS.__version__ == "1.5.7", f"expected stock RNS 1.5.7, got {RNS.__version__}"
    groups = []
    for group in GROUPS:
        for scope in SCOPES:
            for kind in TYPES:
                iface = stock(group, scope, kind)
                groups.append({"group": group, "scope": scope, "type": kind,
                               "address": str(ipaddress.IPv6Address(iface.mcast_discovery_address))})
    default = stock()
    descope = [{"input": a, "stock": default.descope_linklocal(a)} for a in DESCOPE]
    tokens = [{"group": g, "text": t, "token": RNS.Identity.full_hash(g.encode() + t.encode()).hex()}
              for g in GROUPS for t in link_locals()]
    return {"rns": RNS.__version__,
            "default_group": str(ipaddress.IPv6Address(default.mcast_discovery_address)),
            "groups": groups, "descope": descope, "tokens": tokens}


def start_rns() -> Path:
    """A stock instance with no interfaces: Interface() reads its defaults from it."""
    import RNS

    config_dir = Path(tempfile.mkdtemp(prefix="retinue-auto-vectors-"))
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport = No\n  share_instance = No\n\n"
        "[logging]\n  loglevel = 0\n\n[interfaces]\n", encoding="utf-8")
    RNS.Reticulum(configdir=str(config_dir))
    return config_dir


def main() -> int:
    import RNS

    config_dir = start_rns()
    try:
        code = run()
    finally:
        shutil.rmtree(config_dir, ignore_errors=True)
    RNS.exit(code)
    return code


def run() -> int:
    got = vectors()
    text = json.dumps(got, indent=1) + "\n"
    if "--check" in sys.argv:
        ok = FIXTURE.read_text(encoding="utf-8") == text
        print(f"{len(got['groups'])} groups, {len(got['tokens'])} tokens")
        print(f"AUTO VECTORS: {'PASS' if ok else 'FAIL (stock differs from the fixture)'}")
        return 0 if ok else 1
    FIXTURE.write_text(text, encoding="utf-8")
    print(f"wrote {FIXTURE}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
