"""Capture a Meshtastic-compatible device's config stream, black-box.

Sends want_config (field 3 of the request message, discovered empirically by probing which
field number triggers a response — no schema was read) framed in the publicly documented
Stream API (0x94 0xc3, big-endian u16 length). Collects the FromRadio frames the device
streams back and saves them raw. The field NUMBERS and wire TYPES are recorded; their
application meanings are not asserted here.

Usage: python capture_config.py COM7 [output.json] [label]
"""
import argparse
import datetime
import hashlib
import json
import time
from pathlib import Path
import serial

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("port", nargs="?", default="COM7")
parser.add_argument("output", nargs="?", type=Path,
                    default=Path(__file__).parent.parent / "tests" / "fixtures" / "meshtastic_config.json")
parser.add_argument("label", nargs="?", default="config")
parser.add_argument("--producer-version", help="Installed stock version from separate device identification")
parser.add_argument("--producer-evidence", type=Path, help="Device-identification receipt; not firmware source or schemas")
args = parser.parse_args()
PORT, OUTPUT, LABEL = args.port, args.output, args.label
if bool(args.producer_version) != bool(args.producer_evidence):
    parser.error("--producer-version and --producer-evidence must be supplied together")
producer = {"version": None, "identity_basis": "unknown"}
if args.producer_evidence:
    producer = {
        "version": args.producer_version,
        "identity_basis": "caller-supplied separate device-identification receipt",
        "evidence": str(args.producer_evidence),
        "evidence_sha256": hashlib.sha256(args.producer_evidence.read_bytes()).hexdigest(),
    }
WANT_CONFIG_FIELD = 3  # discovered by empirical probe, not read from any schema

if OUTPUT.exists():
    raise SystemExit(f"refusing to overwrite existing capture: {OUTPUT}")


def frame(pb):
    return bytes([0x94, 0xC3]) + len(pb).to_bytes(2, "big") + pb


def varint(v):
    out = bytearray()
    while True:
        b = v & 0x7F
        v >>= 7
        out.append(b | 0x80 if v else b)
        if not v:
            return bytes(out)


def deframe(buf):
    frames, i = [], 0
    while i < len(buf) - 3:
        if buf[i] == 0x94 and buf[i + 1] == 0xC3:
            ln = (buf[i + 2] << 8) | buf[i + 3]
            if i + 4 + ln <= len(buf):
                frames.append(bytes(buf[i + 4 : i + 4 + ln]))
                i += 4 + ln
                continue
        i += 1
    return frames


s = serial.Serial(PORT, 115200, timeout=0.2)
time.sleep(0.4)
s.reset_input_buffer()
s.write(frame(varint((WANT_CONFIG_FIELD << 3) | 0) + varint(0x11223344)))
s.flush()

buf = bytearray()
last = time.time()
deadline = time.monotonic() + 15
byte_limit = 64 * 1024
while time.time() - last < 1.5 and time.monotonic() < deadline and len(buf) < byte_limit:
    c = s.read(min(8192, byte_limit - len(buf)))
    if c:
        buf.extend(c)
        last = time.time()
s.close()
stop_reason = "byte_limit" if len(buf) >= byte_limit else "deadline" if time.monotonic() >= deadline else "quiet"

frames = deframe(buf)
print(f"captured {len(buf)} bytes, {len(frames)} FromRadio frames")

OUTPUT.write_text(
    json.dumps(
        {
            "_comment": (
                "Meshtastic-compatible device config stream, captured black-box "
                f"{datetime.date.today().isoformat()}. "
                "want_config request = field 3 (discovered by empirically probing which field "
                "number triggers a response; no schema was read). Stream API framing (0x94 0xc3 "
                "+ BE u16 len). frames = the raw FromRadio payloads streamed back, hex. Field "
                "NUMBERS/wire TYPES are observable facts; meanings are NOT asserted here."
            ),
            "label": LABEL,
            "producer": producer,
            "port": PORT,
            "want_config_field": WANT_CONFIG_FIELD,
            "frame_count": len(frames),
            "capture_status": "frames_received" if frames else "no_frames",
            "stop_reason": stop_reason,
            "frames": [f.hex() for f in frames],
        },
        indent=1,
    ),
    encoding="utf-8",
)
print(f"wrote {OUTPUT}")
if not frames:
    raise SystemExit("no frames received; empty capture preserved as a failed attempt")
if stop_reason != "quiet":
    raise SystemExit(f"capture reached {stop_reason}; partial attempt preserved")
