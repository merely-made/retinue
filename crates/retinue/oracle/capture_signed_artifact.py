"""Capture RNS 1.4.2's signed artifacts (.rsg / .rsm) as byte-exact vectors.

Black-box, and deliberately at arm's length: this drives the `rnid` executable RNS ships,
as an operator would, and reads the files it writes. Nothing here imports RNS, so the
vectors are evidence about the shipped tool rather than about a library call we chose.

Two identities are captured for each shape:

  * retinue's own fixture identity (the one in tests/fixtures/manifest.json), which makes
    these vectors independent oracle evidence like every other fixture in that directory; and
  * the identity Prns uses in its own signed-artifact tests, which makes the same run an
    independent check of Prns's published constants. Agreement there is worth recording
    precisely because a donor's self-tests cannot confirm themselves.

Ed25519 is deterministic, so a correct implementation reproduces these bytes exactly. That
is the point: `hash` alone would only prove we can call SHA-256.

Writes ../tests/fixtures/rns_signed_artifact.json.

    ./.venv/Scripts/python.exe -u capture_signed_artifact.py
"""

from __future__ import annotations

import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
FIXTURES = HERE.parent / "tests" / "fixtures"
RNID = HERE / ".venv" / "Scripts" / "rnid.exe"
if not RNID.exists():  # POSIX layout
    RNID = HERE / ".venv" / "bin" / "rnid"

# The identity every other fixture in this directory is derived from.
RETINUE_SECRET = "f0ecbba49e783dee14ffc6c9f1e1251efa7d7629e0fa32413c5c59ec2e0f6d6c" * 2
# The identity Prns's signed-artifact tests use: 32 bytes of 0x22 then 32 of 0x11.
PRNS_SECRET = "22" * 32 + "11" * 32

# Ordered metadata, as (key, configobj-spec type, literal, json type tag). The spec is what
# gives RNS's configobj reader a type to coerce to; without it every value is a string.
METADATA = [
    ("name", "string", "Prns", "str"),
    ("version", "integer", "3", "uint"),
    ("tags", "string_list", "one, two", "str_list"),
    ("stable", "boolean", "True", "bool"),
]


def rnid(work: Path, *args: str) -> None:
    result = subprocess.run(
        [str(RNID), "--config", str(work / "cfg"), *args],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise SystemExit(f"rnid {args} failed:\n{result.stdout}\n{result.stderr}")


def rnid_version() -> str:
    result = subprocess.run([str(RNID), "--version"], capture_output=True, text=True)
    return (result.stdout + result.stderr).strip()


def identity_file(work: Path, name: str, secret_hex: str) -> Path:
    path = work / f"{name}.id"
    path.write_bytes(bytes.fromhex(secret_hex))
    return path


def metadata_files(work: Path) -> tuple[Path, Path]:
    values = work / "meta.cfg"
    spec = work / "meta.spec"
    values.write_text("".join(f"{key} = {literal}\n" for key, _, literal, _ in METADATA))
    spec.write_text("".join(f"{key} = {kind}\n" for key, kind, _, _ in METADATA))
    return values, spec


def capture_rsg(work: Path, identity: Path, message: bytes) -> str:
    """A detached signature: `rnid --sign`, which writes <input>.rsg beside its input."""
    target = work / "detached.bin"
    target.write_bytes(message)
    rnid(work, "-i", str(identity), "-s", str(target), "-f")
    return target.with_suffix(target.suffix + ".rsg").read_bytes().hex()


def capture_rsm(work: Path, identity: Path, message: bytes, with_metadata: bool) -> str:
    """An embedded signed message: `rnid --sign-message`, optionally with metadata."""
    target = work / "embedded.bin"
    target.write_bytes(message)
    out = work / "embedded.rsm"
    args = ["-i", str(identity), "-S", "-r", str(target), "-w", str(out), "-f"]
    if with_metadata:
        values, spec = metadata_files(work)
        args += ["-E", str(values), "--meta-spec", str(spec)]
    rnid(work, *args)
    return out.read_bytes().hex()


def main() -> None:
    if not RNID.exists():
        raise SystemExit(f"rnid not found at {RNID}; create the oracle venv first")

    cases = []
    with tempfile.TemporaryDirectory() as raw:
        work = Path(raw)
        for label, secret in (("retinue", RETINUE_SECRET), ("prns", PRNS_SECRET)):
            identity = identity_file(work, label, secret)
            cases.append(
                {
                    "name": f"rsg_{label}_identity",
                    "shape": "rsg",
                    "secret_hex": secret,
                    "message_utf8": "artifact-oracle",
                    "embed": False,
                    "metadata": [],
                    "artifact_hex": capture_rsg(work, identity, b"artifact-oracle"),
                }
            )
            cases.append(
                {
                    "name": f"rsm_{label}_identity",
                    "shape": "rsm",
                    "secret_hex": secret,
                    "message_utf8": "message-oracle",
                    "embed": True,
                    "metadata": [
                        {"key": key, "type": tag, "literal": literal}
                        for key, _, literal, tag in METADATA
                    ],
                    "artifact_hex": capture_rsm(work, identity, b"message-oracle", True),
                }
            )
            cases.append(
                {
                    "name": f"rsm_{label}_identity_bare",
                    "shape": "rsm",
                    "secret_hex": secret,
                    "message_utf8": "message-oracle",
                    "embed": True,
                    "metadata": [],
                    "artifact_hex": capture_rsm(work, identity, b"message-oracle", False),
                }
            )

    fixture = {
        "description": (
            "RNS 1.4.2 signed artifacts captured by driving the shipped `rnid` executable. "
            "artifact = ed25519_signature(64) || msgpack envelope; the signature covers the "
            "envelope, and the envelope commits to sha256(message). retinue must reproduce "
            "every artifact_hex byte for byte."
        ),
        "source": "RNS 1.4.2 rnid, run as a subprocess; no RNS module is imported here",
        "rnid_version": rnid_version(),
        "envelope_layout": {
            "hashtype": "the string sha256",
            "hash": "sha256(message), 32 bytes",
            "meta": "map opening with signer=identity_hash(16) and pubkey=public_identity(64)",
            "message": "present only in an .rsm",
        },
        "metadata_types": {
            "str": "msgpack string",
            "uint": "msgpack unsigned integer",
            "str_list": "msgpack array of strings, comma-separated in the literal",
            "bool": "msgpack boolean",
        },
        "cases": cases,
    }

    out = FIXTURES / "rns_signed_artifact.json"
    out.write_text(json.dumps(fixture, indent=2) + "\n")
    print(f"wrote {out} with {len(cases)} cases", file=sys.stderr)
    for case in cases:
        print(f"  {case['name']}: {len(case['artifact_hex']) // 2} bytes", file=sys.stderr)


if __name__ == "__main__":
    main()
