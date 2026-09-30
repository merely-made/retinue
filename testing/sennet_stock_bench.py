"""Bounded RF receipt against an already identified/configured stock peer.

Never flashes or configures the stock radio. Firmware identity is a separate
receipt. Uses the previously observed serial handshake and numbered fields;
the untouched stock CLI runs separately and no upstream schemas are loaded.
Only matching received text and public node-info frames enter this report.
"""
from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import re
import subprocess
import sys
import time
from pathlib import Path

import serial

from o3_usb_bench import Port, tx

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "crates/sennet/capture"))
from compare_config import parse_message  # noqa: E402

PUBLIC_KEY = "d4f1bb3a20290759f0bcffabcf4e6901"


def deframe(data):
    frames, i = [], 0
    while i + 4 <= len(data):
        if data[i:i+2] != b"\x94\xc3":
            i += 1
            continue
        length = int.from_bytes(data[i+2:i+4], "big")
        if i + 4 + length > len(data):
            break
        frames.append(bytes(data[i+4:i+4+length]))
        i += 4 + length
    return frames


def leaf(data, number, wire):
    return next((value for n, w, value in parse_message(data) or []
                 if n == number and w == wire), None)


def matching(frame, source, text, packet_id=None):
    packet = leaf(frame, 2, 2)
    if packet is None or leaf(packet, 1, 5) != source.to_bytes(4, "little"):
        return False
    # The observed fixed32 field 6 equals the packet ID in the paired RF/client
    # captures already retained in direct_phy_capture.rs.
    if packet_id is not None and leaf(packet, 6, 5) != packet_id.to_bytes(4, "little"):
        return False
    application = leaf(packet, 4, 2)
    return application is not None and leaf(application, 1, 0) == 1 \
        and leaf(application, 2, 2) == text.encode()


def collect(port, seconds):
    data = bytearray()
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        data.extend(port.read(min(8192, 65536-len(data))))
        if len(data) >= 65536:
            raise RuntimeError("stock serial capture exceeded 64 KiB")
    return deframe(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--phy", required=True)
    parser.add_argument("--stock", required=True)
    parser.add_argument("--examples", type=Path, required=True)
    parser.add_argument("--state", type=Path, required=True)
    parser.add_argument("--producer-evidence", type=Path, required=True)
    parser.add_argument("--producer-version", required=True)
    parser.add_argument("--text", required=True)
    parser.add_argument("--duplicates", type=int, choices=range(4), default=0)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("refusing to overwrite receipt")
    evidence = args.producer_evidence.read_bytes()
    if args.producer_version.encode() not in evidence:
        parser.error("producer version absent from separate identification receipt")
    report = {"producer_version": args.producer_version,
              "started_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "producer_evidence_sha256": hashlib.sha256(evidence).hexdigest(),
              "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "phy": args.phy, "stock": args.stock, "text": args.text,
              "source": "f66afb28", "duplicates_requested": args.duplicates,
              "phy_profile": {"frequency_hz": 906875000, "bandwidth_hz": 250000,
                              "spreading_factor": 11, "coding_rate_denominator": 5,
                              "preamble_symbols": 16, "sync_word": "2b"},
              "public_node_info_frames": [], "matching_client_frames": []}
    source = 0xf66afb28
    process = None
    try:
        with serial.Serial(port=None, baudrate=115200, timeout=.15,
                           write_timeout=2) as stock:
            stock.port, stock.dtr, stock.rts = args.stock, True, False
            stock.open()
            time.sleep(.8)
            stock.reset_input_buffer()
            # Empirically observed want_config field 3 and nonce 0x11223344.
            stock.write(bytes.fromhex("94c3000618c4e6888901"))
            config = collect(stock, 3)
            report["public_node_info_frames"] = [f.hex() for f in config
                                                if leaf(f, 4, 2) is not None]
            command = [str(args.examples / "direct_phy_text.exe"), args.phy,
                       str(args.state), f"{source:08x}", "b9300000", "08",
                       PUBLIC_KEY, args.text]
            process = subprocess.Popen(command, stdout=subprocess.PIPE,
                                       stderr=subprocess.STDOUT, text=True)
            live = collect(stock, 20)
            stdout, _ = process.communicate(timeout=5)
            report.update(rust_command=command, rust_exit=process.returncode,
                          rust_output=stdout)
            match = re.search(r"packet_id=([0-9a-f]{8})", stdout)
            if not match:
                raise RuntimeError("Rust transmit did not expose its reserved packet ID")
            packet_id = int(match.group(1), 16)
            report["matching_client_frames"] = [f.hex() for f in live
                                                if matching(f, source, args.text, packet_id)]
            report["packet_id"] = match.group(1)
            if process.returncode:
                raise RuntimeError("Rust transmit/rebroadcast receipt failed")
            if len(report["matching_client_frames"]) != 1:
                raise RuntimeError("stock peer did not expose exactly one matching text")
            if args.duplicates:
                sealed = subprocess.check_output([
                    str(args.examples / "seal_text.exe"), f"{source:08x}",
                    match.group(1), "08", PUBLIC_KEY, args.text], text=True)
                frame = bytes.fromhex(sealed.strip())
                transcript = []
                phy = Port(args.phy, 115200, transcript)
                try:
                    report["duplicate_transmits"] = [tx(phy, i, frame)
                                                     for i in range(args.duplicates)]
                finally:
                    phy.close()
                extra = collect(stock, 8)
                report["duplicate_client_frames"] = [f.hex() for f in extra
                                                      if matching(f, source, args.text, packet_id)]
                if report["duplicate_client_frames"]:
                    raise RuntimeError("stock peer exposed duplicate application delivery")
        report["passed"] = True
    except Exception as error:
        report.update(passed=False, error=str(error))
    finally:
        if process is not None and process.poll() is None:
            process.kill()
            process.communicate()
        args.output.write_text(json.dumps(report, indent=2)+"\n", encoding="utf-8")
    print(json.dumps({k:v for k,v in report.items()
                      if k in ("passed", "error", "packet_id", "producer_version")}))
    return int(not report["passed"])


if __name__ == "__main__":
    raise SystemExit(main())
