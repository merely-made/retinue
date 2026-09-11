"""Guarded two-board MC3 host-personality bench; no flash or durable settings writes.

Uses existing direct-PHY runtime configuration and restores the captured baseline
in finally. A boot nonce before/after establishes continuity for this run, not an
installed-firmware binary hash or complete protocol compatibility.
"""
from __future__ import annotations
import argparse
import hashlib
import json
import os
import subprocess
import time
from pathlib import Path
import serial
from serial.tools.list_ports import comports
from o3_usb_bench import Port, collect_events, discovery, request_profile, utc_now

EXPECTED = {"dut": (0x303A, 0x1001, "44:1B:F6:6A:FA:64"),
            "peer": (0x1915, 0x521F, "TULLE-T114-01")}

class BenchPort(Port):
    def __init__(self, name, dtr, transcript):
        self.name, self.transcript = name, transcript
        self.buffer = bytearray()
        self.serial = serial.Serial(port=None, baudrate=115200, timeout=.10, write_timeout=2)
        self.serial.port = name
        self.serial.rts = False
        self.serial.dtr = dtr
        self.serial.open()
        time.sleep(.3)
        initial = self.serial.read(2048)
        if initial: self.record("rx", initial)

    def text_probe(self, command):
        self.write(command.encode()+b"\n")
        data = bytearray()
        until = time.monotonic()+1.2
        while time.monotonic()<until: data.extend(self.read())
        return data.decode("utf-8", errors="replace")


def snapshot(name, dtr, transcript, previous=None):
    port = BenchPort(name, dtr, transcript)
    try:
        status = port.text_probe("status")
        sync = port.text_probe("sync")
        if "phy online" not in status or "2b 24b4" not in sync:
            raise RuntimeError(f"{name}: expected live direct-PHY baseline sync 2b, got {status!r} / {sync!r}")
        cursor, next_id = discovery(port, 1701)
        if previous is not None:
            if cursor["boot_id"] != previous["cursor"]["boot_id"] or status != previous["status"] or sync != previous["sync"]:
                raise RuntimeError(f"{name}: saved baseline continuity changed")
            profile = request_profile(port, next_id, cursor["boot_id"], previous["profile"]["profile_id"])
            if profile["config"] != previous["profile"]["config"]:
                raise RuntimeError(f"{name}: saved profile descriptor changed")
            port.write(bytes.fromhex(profile["config"]))
            replies = collect_events(port, 3, lambda e: e.get("kind")=="config")
            if [e.get("status") for e in replies if e.get("kind")=="config"] != [0]:
                raise RuntimeError(f"{name}: saved baseline reapply unacknowledged")
        elif cursor["profile_count"] != 1:
            raise RuntimeError(f"{name}: baseline current profile ambiguous; {cursor['profile_count']} descriptors")
        else:
            profile = request_profile(port, next_id, cursor["boot_id"], 1)
        return {"status": status, "sync": sync, "cursor": cursor, "profile": profile}
    finally: port.close()


def restore_and_check(name, dtr, baseline, transcript):
    port = BenchPort(name, dtr, transcript)
    try:
        command = bytes.fromhex(baseline["profile"]["config"])
        if len(command)!=16 or command[0]!=2: raise RuntimeError("invalid baseline configuration")
        port.write(command)
        replies = collect_events(port, 3, lambda e: e.get("kind")=="config")
        acks = [e for e in replies if e.get("kind")=="config"]
        if len(acks)!=1 or acks[0]["status"]!=0: raise RuntimeError(f"baseline restore unacknowledged: {acks}")
        cursor, _ = discovery(port, 1801)
        status, sync = port.text_probe("status"), port.text_probe("sync")
        result={"restore_ack": acks[0], "cursor": cursor, "status":status, "sync":sync,
                "same_boot": cursor["boot_id"]==baseline["cursor"]["boot_id"],
                "same_status": status==baseline["status"], "same_sync":sync==baseline["sync"]}
        if not result["same_boot"] or not result["same_sync"]: raise RuntimeError(f"boot/profile continuity failed: {result}")
        return result
    finally: port.close()


def main():
    ap=argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--exe", type=Path, required=True)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--dut", default="COM7")
    ap.add_argument("--peer", default="COM10")
    ap.add_argument("--baseline-receipt", type=Path, help="prior receipt with successful restoration on both same-boot boards")
    ap.add_argument("--fresh-dut-baseline", action="store_true", help="after explicit DUT reflash/reset, capture its sole current profile afresh")
    ap.add_argument("--deadline-text", choices=("after-deadline", "expiry"), default="after-deadline")
    ap.add_argument("--settle-ms", type=int, choices=range(501), default=0)
    a=ap.parse_args()
    a.output.parent.mkdir(parents=True,exist_ok=True)
    report={"started_utc":utc_now(),"scope":"host-driven retained Retinue Node link and Sennet state",
            "firmware_binary_identity":"not read back; live diagnostics and boot nonces recorded",
            "runner_sha256":hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
            "helper_sha256":hashlib.sha256(Path(__file__).with_name("o3_usb_bench.py").read_bytes()).hexdigest(),
            "exe_sha256":hashlib.sha256(a.exe.read_bytes()).hexdigest(),
            "baseline":{},"restoration":{},"transcript":[],"passed":False}
    report["harness_config"]={"deadline_text":a.deadline_text,"settle_ms":a.settle_ms,"fresh_dut_baseline":a.fresh_dut_baseline}
    source_paths=["Cargo.lock", "crates/retinue/Cargo.toml", "crates/retinue/examples/murmuration_probe.rs",
                  "crates/retinue/examples/murmuration/session.rs", "crates/retinue/src/node.rs",
                  "crates/tulle/src/personality.rs", "crates/tulle/src/personality_serial.rs",
                  "crates/tulle/src/direct_phy_serial.rs", "crates/tulle/src/lib.rs"]
    report["source_sha256"]={name:hashlib.sha256(Path(name).read_bytes()).hexdigest() for name in source_paths}
    report["base_revision"]=subprocess.check_output(["git","rev-parse","HEAD"],text=True).strip()
    ports={p.device:(p.vid,p.pid,p.serial_number) for p in comports()}
    report["ports"]={k:list(v) for k,v in ports.items()}
    hardware={"dut":(a.dut,False),"peer":(a.peer,True)}
    try:
        previous = json.loads(a.baseline_receipt.read_text()) if a.baseline_receipt else None
        if previous:
            report["baseline_receipt_sha256"] = hashlib.sha256(a.baseline_receipt.read_bytes()).hexdigest()
            for role in hardware:
                restored = previous["restoration"][role]
                if not all(restored.get(key) for key in ("same_boot", "same_status", "same_sync")) or restored["restore_ack"]["status"] != 0:
                    raise RuntimeError("previous baseline restoration was not verified")
        for role,(name,dtr) in hardware.items():
            if ports.get(name)!=EXPECTED[role]: raise RuntimeError(f"{role} identity mismatch on {name}")
            reuse = previous["baseline"][role] if previous else None
            if role == "dut" and a.fresh_dut_baseline: reuse = None
            report["baseline"][role]=snapshot(name,dtr,report["transcript"],reuse)
        child=a.output.with_name(a.output.stem+"-host.json")
        report["exe_sha256"] = hashlib.sha256(a.exe.read_bytes()).hexdigest()
        child_env=dict(os.environ,MC3_DEADLINE_TEXT=a.deadline_text,MC3_SETTLE_MS=str(a.settle_ms))
        run=subprocess.run([str(a.exe.resolve()),a.dut,a.peer,str(child.resolve())],capture_output=True,text=True,timeout=180,env=child_env)
        report["host_exit"]=run.returncode
        report["host_stdout"],report["host_stderr"]=run.stdout,run.stderr
        if hashlib.sha256(a.exe.read_bytes()).hexdigest() != report["exe_sha256"]:
            raise RuntimeError("executable changed during physical run")
        if child.exists(): report["host"]=json.loads(child.read_text())
        if run.returncode!=0: raise RuntimeError(f"host probe failed: {run.stderr[-1200:]}")
        if not report.get("host", {}).get("passed"): raise RuntimeError("host receipt absent or failed")
        report["passed"]=True
    except Exception as e:
        report["error"]=repr(e)
    finally:
        for role,baseline in report["baseline"].items():
            name,dtr=hardware[role]
            try: report["restoration"][role]=restore_and_check(name,dtr,baseline,report["transcript"])
            except Exception as e:
                report["restoration"][role]={"error":repr(e)}
                report["passed"]=False
        report["finished_utc"]=utc_now()
        a.output.write_text(json.dumps(report,indent=2))
        print(json.dumps({"passed":report["passed"],"error":report.get("error"),"restoration":report["restoration"]},indent=2))
    return 0 if report["passed"] else 1

if __name__=="__main__": raise SystemExit(main())
