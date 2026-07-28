"""Verify a Retinue-built ratchet packet with stock RNS public APIs.

Usage:
    python verify_retinue_ratchet_packet.py PACKET_HEX

The matching deterministic packet is emitted by:
    cargo run -p retinue --example ratchet_packet_vector
"""

from __future__ import annotations

import sys

import RNS


IDENTITY_SECRET = bytes([0x62]) * 64
RATCHET_SECRET = bytes([0x71]) * 32
EXPECTED = b"RETINUE-R9-OUTBOUND"


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit("usage: verify_retinue_ratchet_packet.py PACKET_HEX")

    identity = RNS.Identity.from_bytes(IDENTITY_SECRET)
    packet = RNS.Packet(None, None)
    packet.raw = bytes.fromhex(sys.argv[1])
    assert packet.unpack()
    plaintext = identity.decrypt(
        packet.data,
        ratchets=[RATCHET_SECRET],
        enforce_ratchets=True,
    )
    assert plaintext == EXPECTED
    print(f"RNS {RNS.__version__}")
    print(f"packet {len(packet.raw)} bytes, token {len(packet.data)} bytes")
    print("stock retained-ratchet decrypt of Retinue packet: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
