"""Run bounded concurrent RF and unread-observation traffic on the O3 bench.

Requires exclusive T114/V4 access. Copies the T114 PHY to V4 runtime settings.
Host timestamps demonstrate concurrent workload, not device execution latency.
"""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import threading
import time

import serial
import o3_usb_bench as bench


def run(args, report):
    trace = report["transcript"]
    radio_ready = threading.Event()
    port = bench.Port(args.t114, 115200, trace)
    try:
        initial, request_id = bench.discovery(port, 1)
        if initial["profile_count"] != 1:
            raise RuntimeError("requires one unambiguous T114 PHY")
        profile = bench.request_profile(port, request_id, initial["boot_id"], 1)
        config = bytes.fromhex(profile["config"])
        report.update(initial=initial, profile=profile)

        def transmit():
            peer = bench.Port(args.v4, 115200, trace)
            results = []
            try:
                peer.write(b"\x00" + config)
                events = bench.collect_events(peer, 2, lambda e: e["kind"] == "config")
                if not any(e == {"kind": "config", "status": 0} for e in events):
                    raise RuntimeError("V4 PHY configuration refused")
                radio_ready.set()
                for i in range(12):
                    started = time.perf_counter()
                    outcome = bench.tx(peer, i, f"o3-overlap-{i:02}".encode())
                    results.append(dict(outcome, started=started, finished=time.perf_counter()))
                return results
            finally:
                peer.close()

        with ThreadPoolExecutor(max_workers=1) as executor:
            radio = executor.submit(transmit)
            if not radio_ready.wait(5):
                raise RuntimeError("RF peer did not become ready")
            report["pressure_started"] = time.perf_counter()
            count = 0
            deadline = time.monotonic() + 5
            for i in range(256):
                if time.monotonic() >= deadline:
                    break
                try:
                    port.write(bench.observation_request(100 + i))
                    count += 1
                except serial.SerialTimeoutException:
                    break
            report["pressure_requests"] = count
            # Keep replies unread while the bounded RF workload finishes.
            report["transmissions"] = radio.result(timeout=35)
            report["unread_finished"] = time.perf_counter()
            end = time.monotonic() + 3
            drained = 0
            while time.monotonic() < end:
                drained += len(port.read())
            report["pressure_bytes_drained"] = drained
            try:
                report["same_session_reply"] = bench.request_cursor(port, 1000)
            except (TimeoutError, serial.SerialTimeoutException) as error:
                report["same_session_failure"] = type(error).__name__
    finally:
        port.close()

    port = bench.Port(args.t114, 115200, trace)
    try:
        recovered, request_id = bench.discovery(port, 1001)
        report["recovered"] = recovered
        if recovered["boot_id"] != initial["boot_id"]:
            raise RuntimeError("board rebooted during pressure")
        records, _ = bench.drain(port, initial["boot_id"], request_id, initial["newest"])
        report["records"] = records
        report["captures"] = sum(r.get("event_tag") == 2 for r in records)
        report["gaps"] = sum(r.get("gap_count", 0) for r in records)
        if report["captures"] != 12 or report["gaps"]:
            raise RuntimeError("concurrent RF capture count or continuity failed")
    finally:
        port.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--t114", default="COM10")
    parser.add_argument("--v4", default="COM6")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {"started_utc": bench.utc_now(), "transcript": [], "ports": vars(args) | {"output": str(args.output)}}
    try:
        run(args, report)
        report["passed"] = True
    except Exception as error:
        report["passed"] = False
        report["error"] = {"type": type(error).__name__, "message": str(error)}
    finally:
        report["finished_utc"] = bench.utc_now()
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
