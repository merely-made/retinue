"""Guarded V4 board-timed PHY excursion proof; restores both runtime profiles."""
from __future__ import annotations
import argparse
import hashlib
import json
import os
from pathlib import Path
import struct
import subprocess
import time
from serial.tools.list_ports import comports
from mc3_personality_bench import BenchPort, EXPECTED, snapshot, restore_and_check
from o3_usb_bench import collect_events, tx, utc_now


def excursion_event(port, timeout=3):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        port.buffer.extend(port.read())
        while port.buffer:
            marker = port.buffer[0]
            if marker == 0x87:
                size = 18
            elif marker == 0x81:
                if len(port.buffer) < 3:
                    break
                size = 7 + int.from_bytes(port.buffer[1:3], "little")
            elif marker == 0x82:
                size = 4
            elif marker in (0x83, 0x85):
                size = 2
            else:
                del port.buffer[0]
                continue
            if len(port.buffer) < size:
                break
            frame = bytes(port.buffer[:size])
            del port.buffer[:size]
            if marker == 0x87:
                return {"status": frame[1], "deadline_ms": int.from_bytes(frame[2:10], "little"),
                        "board_ms": int.from_bytes(frame[10:18], "little"), "hex": frame.hex()}
    raise TimeoutError("board excursion event absent")


def configure(port, command):
    port.write(command)
    events = collect_events(port, 3, lambda e: e["kind"] == "config")
    assert [e["status"] for e in events if e["kind"] == "config"] == [0], events


def receive_exact(port, payload):
    events = collect_events(port, 5, lambda e: e["kind"] == "rx" and e["hex"] == payload.hex())
    assert any(e["kind"] == "rx" and e["hex"] == payload.hex() for e in events), events
    return events


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--output", type=Path, required=True)
    ap.add_argument("--baseline-receipt", type=Path, required=True)
    ap.add_argument("--fresh-dut-baseline", action="store_true")
    args = ap.parse_args()
    previous = json.loads(args.baseline_receipt.read_text())
    report = {"started_utc": utc_now(), "scope": "V4 autonomous bounded PHY excursion, host protocol adapters remain separate",
              "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
              "baseline": {}, "restoration": {}, "transcript": [], "events": [], "passed": False}
    sources = [__file__, "firmware/heltec-v4-phy/src/main.rs", "firmware/heltec-v4-phy/src/murmuration.rs",
               "firmware/heltec-v4-phy/src/radio_owner.rs", "crates/selvage/src/lib.rs", "crates/selvage/src/personality.rs"]
    report["source_sha256"] = {str(p): hashlib.sha256(Path(p).read_bytes()).hexdigest() for p in sources}
    report["baseline_receipt_sha256"] = hashlib.sha256(args.baseline_receipt.read_bytes()).hexdigest()
    hardware = {"dut": ("COM7", False), "peer": ("COM10", True)}
    opened = []
    try:
        ports = {p.device: (p.vid, p.pid, p.serial_number) for p in comports()}
        for role, (name, dtr) in hardware.items():
            assert ports.get(name) == EXPECTED[role], (name, ports)
            restored = previous["restoration"][role]
            assert all(restored[k] for k in ("same_boot", "same_status", "same_sync"))
            assert restored["restore_ack"]["status"] == 0
            reuse = None if role == "dut" and args.fresh_dut_baseline else previous["baseline"][role]
            report["baseline"][role] = snapshot(name, dtr, report["transcript"], reuse)
        dut = BenchPort("COM7", False, report["transcript"])
        opened.append(dut)
        peer = BenchPort("COM10", True, report["transcript"])
        opened.append(peer)
        home = bytearray.fromhex(report["baseline"]["dut"]["profile"]["config"])
        home[-1] = 7
        configure(dut, home)
        configure(peer, home)
        away = bytearray(home)
        away[13] = 0x12
        away[0] = 0x06
        # Invalid duration must fail without taking the radio away.
        dut.write(away + struct.pack("<Q", 60_001))
        refused = excursion_event(dut)
        assert refused["status"] == 2, refused
        report["events"].append({"kind": "overbound_refused", **refused})
        request = away + struct.pack("<Q", 2_000)
        dut.write(request[:9])
        time.sleep(.05)
        dut.write(request[9:])
        accepted = excursion_event(dut)
        assert accepted["status"] == 0, accepted
        report["events"].append({"kind": "accepted", **accepted})
        # Deliberately issue neither host commands nor reads during this interval.
        silence = time.monotonic()
        time.sleep(3.5)
        report["events"].append({"kind": "host_silence", "duration_ms": (time.monotonic()-silence)*1000})
        restored = excursion_event(dut)
        assert restored["status"] == 1, restored
        assert restored["board_ms"] >= accepted["deadline_ms"], (accepted, restored)
        assert restored["board_ms"] <= accepted["deadline_ms"] + 1_500, (accepted, restored)
        report["events"].append({"kind": "board_returned", **restored})
        for from_port, to_port in ((peer, dut), (dut, peer)):
            payload = b"mc3e-home-" + os.urandom(12)
            tx(from_port, 0, payload)
            events = receive_exact(to_port, payload)
            report["events"].append({"kind": "home_rf_exact", "from": from_port.name, "to": to_port.name,
                                     "payload_hex": payload.hex(), "received": events})
        report["passed"] = True
    except Exception as error:
        report["error"] = repr(error)
    finally:
        for port in opened:
            port.close()
        for role, baseline in report["baseline"].items():
            name, dtr = hardware[role]
            try:
                result = restore_and_check(name, dtr, baseline, report["transcript"])
                report["restoration"][role] = result
                assert result["same_status"]
            except Exception as error:
                report["restoration"][role] = {"error": repr(error)}
                report["passed"] = False
        report["finished_utc"] = utc_now()
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2))
        print(json.dumps({"passed": report["passed"], "error": report.get("error"),
                          "events": report["events"], "restoration": report["restoration"]}, indent=2))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
