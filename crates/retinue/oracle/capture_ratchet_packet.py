"""Capture the R9 single-packet ratchet boundary from stock RNS.

BLACK-BOX DISCIPLINE
--------------------
This script calls documented runtime APIs and observes their bytes. It never
reads the Python implementation source.

The outgoing destination's documented ``encrypt`` operation is instrumented
to call the documented ``Identity.encrypt(..., ratchet=...)`` API with a fixed
ratchet public key. This keeps the ratchet private key known to the oracle
while leaving token construction and packet packing to stock RNS.
"""

from __future__ import annotations

import hashlib
import hmac
import json

import RNS
from cryptography.hazmat.primitives import hashes, padding
from cryptography.hazmat.primitives.asymmetric.x25519 import (
    X25519PrivateKey,
    X25519PublicKey,
)
from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.kdf.hkdf import HKDF


IDENTITY_SECRET = bytes([0x62]) * 64
RATCHET_SECRET = bytes([0x71]) * 32
PLAINTEXT = b"R9-WIRE-PACKET"
APP = "retinue"
ASPECTS = ("ratchet",)


def main() -> int:
    identity = RNS.Identity.from_bytes(IDENTITY_SECRET)
    ratchet_private = X25519PrivateKey.from_private_bytes(RATCHET_SECRET)
    ratchet_public = ratchet_private.public_key().public_bytes_raw()
    ratchet_id = hashlib.sha256(ratchet_public).digest()[:10]

    outgoing = RNS.Destination(
        identity,
        RNS.Destination.OUT,
        RNS.Destination.SINGLE,
        APP,
        *ASPECTS,
    )
    stock_encrypt = outgoing.encrypt
    outgoing.encrypt = lambda plaintext: identity.encrypt(
        plaintext, ratchet=ratchet_public
    )
    try:
        packet = RNS.Packet(
            outgoing,
            PLAINTEXT,
            context=RNS.Packet.NONE,
            create_receipt=False,
        )
        packet.pack()
    finally:
        outgoing.encrypt = stock_encrypt

    unpacked = RNS.Packet(None, None)
    unpacked.raw = packet.raw
    assert unpacked.unpack()
    assert identity.decrypt(
        unpacked.data,
        ratchets=[RATCHET_SECRET],
        enforce_ratchets=True,
    ) == PLAINTEXT

    ephemeral_public = unpacked.data[:32]
    body = unpacked.data[32:-32]
    tag = unpacked.data[-32:]
    shared = ratchet_private.exchange(
        X25519PublicKey.from_public_bytes(ephemeral_public)
    )
    derived = HKDF(
        algorithm=hashes.SHA256(),
        length=64,
        salt=identity.hash,
        info=b"",
    ).derive(shared)
    assert hmac.compare_digest(
        hmac.new(derived[:32], body, hashlib.sha256).digest(), tag
    )
    decryptor = Cipher(
        algorithms.AES(derived[32:]),
        modes.CBC(body[:16]),
    ).decryptor()
    padded = decryptor.update(body[16:]) + decryptor.finalize()
    unpadder = padding.PKCS7(128).unpadder()
    assert unpadder.update(padded) + unpadder.finalize() == PLAINTEXT

    fixture = {
        "stock": {
            "rns_version": RNS.__version__,
            "method": "Identity.encrypt(plaintext, ratchet=public_key)",
        },
        "identity_secret_hex": IDENTITY_SECRET.hex(),
        "identity_hash_hex": identity.hash.hex(),
        "destination_hash_hex": outgoing.hash.hex(),
        "ratchet_secret_hex": RATCHET_SECRET.hex(),
        "ratchet_public_hex": ratchet_public.hex(),
        "ratchet_id_hex": ratchet_id.hex(),
        "plaintext_hex": PLAINTEXT.hex(),
        "packet_hex": packet.raw.hex(),
        "packet": {
            "flags": packet.raw[0],
            "context": unpacked.context,
            "context_flag": bool(unpacked.context_flag),
            "payload_len": len(unpacked.data),
        },
        "facts": {
            "token_layout": "ephemeral_public(32) || iv(16) || ciphertext || hmac_sha256(32)",
            "shared_secret": "x25519(ephemeral_secret, ratchet_public)",
            "hkdf_salt": "recipient identity hash",
            "hkdf_info": "empty",
            "derived_key_layout": "hmac_key(32) || aes256_key(32)",
            "ratchet_id_on_wire": False,
            "receiver_selection": "trial retained ratchet secrets until HMAC verifies",
        },
    }
    print(f"RNS {RNS.__version__}")
    print(f"destination {outgoing.hash.hex()}")
    print(f"ratchet id {ratchet_id.hex()} (not carried on wire)")
    print(f"packet {len(packet.raw)} bytes, token {len(unpacked.data)} bytes")
    print("stock decrypt with retained ratchet: PASS")
    print("independent HKDF/HMAC/AES decrypt: PASS")
    print("FIXTURE_JSON")
    print(json.dumps(fixture, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
