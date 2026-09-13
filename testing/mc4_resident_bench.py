"""Guarded MC4c resident bench entrypoint.

It validates the concrete V4/T114 identities, captures the current peer
direct-PHY descriptor and restores that peer in ``finally``.  Resident setup
is intentionally one-shot: the DUT is never sent a legacy configuration after
0x07.  A successful run therefore records that an operator reset is required
before another setup attempt.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
from pathlib import Path

from serial.tools.list_ports import comports
from mc3_personality_bench import EXPECTED, snapshot, restore_and_check, utc_now


def digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--exe", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--dut", default="COM7")
    parser.add_argument("--peer", default="COM10")
    parser.add_argument("--peer-baseline-receipt", type=Path)
    args = parser.parse_args()
    if args.output.exists():
        parser.error("choose a new receipt path; existing evidence is never overwritten")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    ports = {p.device: (p.vid, p.pid, p.serial_number) for p in comports()}
    report = {"started_utc": utc_now(), "passed": False, "baseline": {}, "restoration": {},
              "dut_reset_required": False, "transcript": [],
              "scope": "MC4c resident setup; peer restoration only after one-shot DUT setup",
              "runner_sha256": digest(Path(__file__)), "exe_sha256": digest(args.exe),
              "ports": {name: list(value) for name, value in ports.items()}}
    sources = ["Cargo.lock", "crates/retinue/Cargo.toml", "crates/retinue/examples/resident_probe.rs",
               "crates/retinue/examples/resident/retinue.rs", "crates/radio-hand/src/resident_wire.rs",
               "crates/radio-hand/src/resident_command.rs", "crates/radio-hand/src/instances.rs",
               "firmware/heltec-v4-phy/src/resident.rs"]
    report["source_sha256"] = {path: digest(Path(path)) for path in sources}
    report["base_revision"] = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    try:
        if ports.get(args.dut) != EXPECTED["dut"]:
            raise RuntimeError(f"DUT identity mismatch on {args.dut}: {ports.get(args.dut)!r}")
        if ports.get(args.peer) != EXPECTED["peer"]:
            raise RuntimeError(f"peer identity mismatch on {args.peer}: {ports.get(args.peer)!r}")
        # Deassert V4 control lines before opening. A fresh DUT has one profile;
        # resident mode must have been exited by an explicit external reset.
        report["baseline"]["dut"] = snapshot(args.dut, False, report["transcript"])
        previous = None
        if args.peer_baseline_receipt:
            previous_doc = json.loads(args.peer_baseline_receipt.read_text())
            previous = previous_doc["baseline"]["peer"]
            report["peer_baseline_receipt_sha256"] = digest(args.peer_baseline_receipt)
        report["baseline"]["peer"] = snapshot(args.peer, True, report["transcript"], previous)
        child = args.output.with_name(args.output.stem + "-host.json")
        report["dut_reset_required"] = True
        run = subprocess.run([str(args.exe.resolve()), args.dut, args.peer, str(child.resolve())],
                             capture_output=True, text=True, timeout=150)
        report.update(host_exit=run.returncode, host_stdout=run.stdout, host_stderr=run.stderr)
        if digest(args.exe) != report["exe_sha256"]:
            raise RuntimeError("host executable changed during run")
        if any(digest(Path(path)) != expected for path, expected in report["source_sha256"].items()):
            raise RuntimeError("bench source changed during run")
        if child.exists():
            report["host"] = json.loads(child.read_text())
        # Do not claim an RF acceptance merely because setup/status succeeded.
        # The completed host harness must produce all on-air visit proofs.
        if run.returncode != 0 or not report.get("host", {}).get("qualified"):
            raise RuntimeError("resident host probe failed")
        report["passed"] = True
    except Exception as error:
        report["error"] = repr(error)
    finally:
        if "peer" in report["baseline"]:
            try:
                report["restoration"]["peer"] = restore_and_check(
                    args.peer, True, report["baseline"]["peer"], report["transcript"])
            except Exception as error:
                report["restoration"]["peer"] = {"error": repr(error)}
                report["passed"] = False
        report["finished_utc"] = utc_now()
        args.output.write_text(json.dumps(report, indent=2))
        print(json.dumps({"passed": report["passed"], "error": report.get("error"),
                          "dut_reset_required": report["dut_reset_required"]}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
