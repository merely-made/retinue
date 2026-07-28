"""Capture and explain stock RNS Interface Access Code bytes.

This follows the oracle rule used throughout this directory: run the pinned RNS package
through its public API, record its wire output, and test explicit hypotheses without
reading the reference implementation's source.

Run from the repository root:

    crates/retinue/oracle/.venv/Scripts/python.exe -u \
        crates/retinue/oracle/capture_ifac.py
"""

from __future__ import annotations

import atexit
import json
import shutil
import socket
import tempfile
import threading
import time
from pathlib import Path

import RNS


HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parent / "tests" / "fixtures"

NETWORK_NAME = "retinue-ifac"
PASSPHRASE = "wire-compatibility"
IFAC_BITS = 64
IFAC_BYTES = IFAC_BITS // 8
APP_DATA = b"retinue-ifac-fixture"

FLAG, ESC, ESC_MASK = 0x7E, 0x7D, 0x20


def hdlc_deframe(stream: bytes) -> list[bytes]:
    frames: list[bytes] = []
    current = bytearray()
    in_frame = False
    escaped = False
    for byte in stream:
        if byte == FLAG:
            if in_frame and current:
                frames.append(bytes(current))
            current = bytearray()
            in_frame = True
            escaped = False
        elif not in_frame:
            continue
        elif escaped:
            current.append(byte ^ ESC_MASK)
            escaped = False
        elif byte == ESC:
            escaped = True
        else:
            current.append(byte)
    return frames


def candidate_identities() -> list[tuple[str, RNS.Identity, bytes, bytes]]:
    name = NETWORK_NAME.encode("utf-8")
    phrase = PASSPHRASE.encode("utf-8")
    full_hash = RNS.Identity.full_hash
    origins = {
        "name || passphrase": name + phrase,
        "name || NUL || passphrase": name + b"\x00" + phrase,
        "H(name) || H(passphrase)": full_hash(name) + full_hash(phrase),
        "H(name) || passphrase": full_hash(name) + phrase,
        "name || H(passphrase)": name + full_hash(phrase),
    }

    identities = []
    for label, origin in origins.items():
        origin_hash = full_hash(origin)
        key = RNS.Cryptography.hkdf(
            length=RNS.Identity.KEYSIZE // 8,
            derive_from=origin_hash,
            salt=RNS.Reticulum.IFAC_SALT,
            context=None,
        )
        identity = RNS.Identity.from_bytes(key)
        if identity is not None:
            identities.append(
                (f"HKDF(H({label}), IFAC_SALT)", identity, key, origin_hash)
            )
    return identities


def identify_transform(frame: bytes, logical: bytes) -> dict | None:
    inputs = {
        "packet with IFAC flag clear": bytes([logical[0] & 0x7F]) + logical[1:],
        "packet with IFAC flag set": bytes([logical[0] | 0x80]) + logical[1:],
    }
    for identity_label, identity, key, origin_hash in candidate_identities():
        for input_label, signed in inputs.items():
            signature = identity.sign(signed)
            truncations = {
                "signature prefix": signature[:IFAC_BYTES],
                "signature suffix": signature[-IFAC_BYTES:],
            }
            for truncation_label, candidate in truncations.items():
                offset = frame.find(candidate)
                if offset >= 0:
                    mask_derivation = identify_mask(
                        frame, logical, key, origin_hash, candidate
                    )
                    return {
                        "identity_derivation": identity_label,
                        "signed_input": input_label,
                        "truncation": truncation_label,
                        "ifac_offset": offset,
                        "ifac_hex": candidate.hex(),
                        "mask_derivation": mask_derivation,
                        "logical_hex": logical.hex(),
                    }
    return None


def identify_mask(
    frame: bytes, logical: bytes, key: bytes, origin_hash: bytes, ifac: bytes
) -> str | None:
    expected = RNS.Cryptography.hkdf(
        length=len(frame),
        derive_from=ifac,
        salt=key,
        context=None,
    )
    recovered = bytes(
        byte ^ expected[index]
        for index, byte in enumerate(frame)
        if index <= 1 or index > 1 + IFAC_BYTES
    )
    print(f"HKDF(IFAC, key) recovers: {recovered.hex()}")

    materials = {
        "IFAC key": key,
        "IFAC origin hash": origin_hash,
        "IFAC": ifac,
        "H(IFAC key)": RNS.Identity.full_hash(key),
        "H(IFAC origin hash)": RNS.Identity.full_hash(origin_hash),
        "H(IFAC)": RNS.Identity.full_hash(ifac),
    }
    for label, material in materials.items():
        mask = (material * ((len(frame) + len(material) - 1) // len(material)))[: len(frame)]
        if mask_matches(frame, logical, mask):
            return f"repeated {label}"

    salts = {
        "none": None,
        "IFAC_SALT": RNS.Reticulum.IFAC_SALT,
        "IFAC key": key,
        "IFAC origin hash": origin_hash,
        "IFAC": ifac,
    }
    for source_label, source in materials.items():
        for salt_label, salt in salts.items():
            candidate = RNS.Cryptography.hkdf(
                length=len(frame),
                derive_from=source,
                salt=salt,
                context=None,
            )
            if mask_matches(frame, logical, candidate):
                return f"HKDF({source_label}, salt={salt_label})"
    return None


def mask_matches(frame: bytes, logical: bytes, mask: bytes) -> bool:
    if frame[0] != ((logical[0] | 0x80) ^ mask[0]):
        return False
    if frame[1] != (logical[1] ^ mask[1]):
        return False
    return all(
        wire == (plain ^ mask[index])
        for index, (wire, plain) in enumerate(
            zip(frame[2 + IFAC_BYTES :], logical[2:]),
            start=2 + IFAC_BYTES,
        )
    )


def main() -> int:
    print(f"RNS {RNS.__version__}")

    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind(("127.0.0.1", 0))
    server.listen(1)
    port = server.getsockname()[1]

    recorded = bytearray()
    stop = threading.Event()

    def recorder() -> None:
        server.settimeout(0.25)
        while not stop.is_set():
            try:
                connection, address = server.accept()
            except TimeoutError:
                continue
            print(f"recorder accepted {address}")
            connection.settimeout(0.25)
            while not stop.is_set():
                try:
                    chunk = connection.recv(65536)
                    if not chunk:
                        break
                    recorded.extend(chunk)
                    print(f"recorder received {len(chunk)} bytes")
                except TimeoutError:
                    continue
            connection.close()
        server.close()

    threading.Thread(target=recorder, daemon=True).start()

    config_dir = Path(tempfile.mkdtemp(prefix="retinue-ifac-"))
    (config_dir / "config").write_text(
        "[reticulum]\n"
        "  enable_transport = No\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "\n[logging]\n"
        "  loglevel = 7\n"
        "\n[interfaces]\n"
        "  [[IFAC Recorder]]\n"
        "    type = TCPClientInterface\n"
        "    enabled = yes\n"
        "    target_host = 127.0.0.1\n"
        f"    target_port = {port}\n"
        f"    network_name = {NETWORK_NAME}\n"
        f"    passphrase = {PASSPHRASE}\n"
        f"    ifac_size = {IFAC_BITS}\n",
        encoding="utf-8",
    )

    RNS.Reticulum(configdir=str(config_dir))
    exit_code = 1
    try:
        destination = RNS.Destination(
            None,
            RNS.Destination.OUT,
            RNS.Destination.PLAIN,
            "retinue",
            "ifac",
        )
        time.sleep(2.5)
        for interface in RNS.Transport.interfaces:
            print(
                f"interface {interface}: online={interface.online}, "
                f"ifac_size={interface.ifac_size}"
            )
        packet = RNS.Packet(destination, APP_DATA, create_receipt=False)
        packet.pack()
        logical = bytes(packet.raw)
        print(f"logical {len(logical)} bytes: {logical.hex()}")
        packet.send()
        time.sleep(1.5)
        stop.set()
        time.sleep(0.3)

        frames = hdlc_deframe(bytes(recorded))
        print(f"recorded {len(recorded)} bytes in frames {[len(frame) for frame in frames]}")
        for index, frame in enumerate(frames):
            print(f"frame {index}: {frame.hex()}")
        matches = [frame for frame in frames if len(frame) == len(logical) + IFAC_BYTES]
        if not matches:
            print("FAILED: no IFAC announce reached the recorder")
            return 1

        frame = matches[0]
        transform = identify_transform(frame, logical)
        print(f"captured {len(frame)} bytes: {frame.hex()}")
        print(json.dumps(transform, indent=2))
        if transform is None:
            print("FAILED: the captured bytes falsified every stated derivation hypothesis")
            return 1

        FIXTURES.mkdir(parents=True, exist_ok=True)
        (FIXTURES / "ifac_packet.bin").write_bytes(frame)
        (FIXTURES / "ifac_packet.json").write_text(
            json.dumps(
                {
                    "rns_version": RNS.__version__,
                    "network_name": NETWORK_NAME,
                    "passphrase": PASSPHRASE,
                    "ifac_bits": IFAC_BITS,
                    "destination_hash": destination.hash.hex(),
                    "wire_hex": frame.hex(),
                    **transform,
                },
                indent=2,
            )
            + "\n",
            encoding="utf-8",
        )
        print("wrote ifac_packet.bin and ifac_packet.json")
        exit_code = 0
        return 0
    finally:
        stop.set()
        atexit.register(shutil.rmtree, config_dir, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
