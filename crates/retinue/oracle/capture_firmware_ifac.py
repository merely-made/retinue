"""Observe stock RNS 1.5.4 IFAC frames through public APIs and TCP capture.

TCP deliberately carries 256-byte frames too: the 255-byte radio admission rule
is Retinue's responsibility and is qualified by the Rust fixture replay.
No reference implementation source is read and no IFAC transform is recreated.
"""
from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib.metadata
import json
import platform
import socket
import subprocess
import sys
import tempfile
import time
import traceback
from pathlib import Path

import RNS

from capture_ifac import hdlc_deframe

NETWORK = "retinue-firmware-ifac"
PASSPHRASE = "public-oracle-fixture"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path(__file__).resolve().parent.parent / "tests/fixtures/rns_ifac_1_5_4.json")
    parser.add_argument("--child-config", type=Path, help=argparse.SUPPRESS)
    args = parser.parse_args()
    if RNS.__version__ != "1.5.4" or importlib.metadata.version("rns") != "1.5.4":
        raise RuntimeError("This version-labelled fixture requires actual stock RNS 1.5.4")
    if args.output.exists():
        raise FileExistsError(f"Refusing to overwrite retained capture: {args.output}")
    if args.child_config is None:
        # RNS.exit terminates the interpreter; only the supervised child calls it.
        # The parent owns the temporary directory and removes it after child exit.
        with tempfile.TemporaryDirectory(prefix="retinue-firmware-ifac-") as directory:
            process = subprocess.Popen([sys.executable, "-u", str(Path(__file__).resolve()),
                                        "--output", str(args.output.resolve()), "--child-config", directory])
            try:
                result = process.wait(timeout=40)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
                raise
        print(f"TEMP_CONFIG_REMOVED={not Path(directory).exists()}", flush=True)
        return result
    server = socket.socket()
    server.bind(("127.0.0.1", 0))
    server.listen(1)
    server.settimeout(10)
    with contextlib.nullcontext(str(args.child_config)) as directory:
        Path(directory, "config").write_text(
            "[reticulum]\n  enable_transport = No\n  share_instance = No\n"
            "[logging]\n  loglevel = 2\n[interfaces]\n  [[capture]]\n"
            "    type = TCPClientInterface\n    enabled = yes\n"
            "    target_host = 127.0.0.1\n"
            f"    target_port = {server.getsockname()[1]}\n"
            f"    network_name = {NETWORK}\n    passphrase = {PASSPHRASE}\n"
            "    ifac_size = 64\n", encoding="utf-8")
        RNS.Reticulum(configdir=directory)
        connection, _ = server.accept()
        connection.settimeout(5)
        time.sleep(0.5)
        destination = RNS.Destination(None, RNS.Destination.OUT, RNS.Destination.PLAIN, "retinue", "firmware_ifac")
        cases = []
        for logical_size in (231, 232, 247, 248):
            payload = bytes((index * 17 + logical_size) % 256 for index in range(logical_size - 19))
            packet = RNS.Packet(destination, payload, attached_interface=RNS.Transport.interfaces[0], create_receipt=False)
            packet.pack()
            logical = bytes(packet.raw)
            assert packet.send() is not False, "Stock RNS refused transmission"
            received = bytearray()
            frames = []
            deadline = time.monotonic() + 5
            while not any(len(frame) == len(logical) + 8 for frame in frames):
                if time.monotonic() >= deadline:
                    raise TimeoutError("No matching IFAC frame within five seconds")
                chunk = connection.recv(65536)
                if not chunk:
                    raise EOFError("Stock TCP interface closed during capture")
                received.extend(chunk)
                frames = hdlc_deframe(bytes(received))
            wire = next(frame for frame in frames if len(frame) == len(logical) + 8)
            assert len(logical) == logical_size
            assert len(wire) == len(logical) + 8
            case = {"name": f"type1_{logical_size}", "logical_hex": logical.hex(),
                    "wire_hex": wire.hex(), "logical_size": len(logical), "wire_size": len(wire),
                    "wire_sha256": hashlib.sha256(wire).hexdigest(), "fits_physical_255": len(wire) <= 255}
            cases.append(case)
            print(f"CAPTURE {case['name']}: logical={len(logical)} carrier={len(wire)} fits255={case['fits_physical_255']}", flush=True)
        result = {"producer": {"package": "rns", "rns_version": RNS.__version__, "distribution_version": importlib.metadata.version("rns"),
                               "python": platform.python_version(), "script": "crates/retinue/oracle/capture_firmware_ifac.py",
                               "method": "public RNS.Packet API; configured stock TCPClientInterface; HDLC socket capture",
                               "scope": "stock egress bytes; physical carrier admission is tested in Rust, not observed over radio"},
                  "network_name": NETWORK, "passphrase": PASSPHRASE, "ifac_bytes": 8, "physical_frame_budget": 255, "cases": cases}
        with args.output.open("x", encoding="utf-8") as output:
            output.write(json.dumps(result, indent=2) + "\n")
        print(f"WROTE {args.output}", flush=True)
        connection.close()
        server.close()
        RNS.exit(0)
    return 0


if __name__ == "__main__":
    try:
        exit_code = main()
    except Exception:
        traceback.print_exc()
        if "--child-config" in sys.argv:
            RNS.exit(1)
        raise SystemExit(1)
    raise SystemExit(exit_code)
