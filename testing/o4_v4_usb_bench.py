"""Bounded active-radio USB observation receipt for an already-flashed V4."""
from __future__ import annotations

import argparse
import hashlib
import json
import struct
import threading
import time
from pathlib import Path

from serial.tools.list_ports import comports

from o3_usb_bench import (
    Port, collect_events, discovery, drain, observation_request, request_cursor, request_profile,
    tx, utc_now,
)

V4_SERIAL = "44:1B:F6:6A:FA:64"
T114_SERIAL = "TULLE-T114-01"


def cursor_with_rx(port: Port, request_id: int, boot_id: int, after: int) -> tuple[dict, list[str]]:
    port.write(observation_request(request_id, boot_id, after))
    events = collect_events(
        port, 2, lambda event: event.get("kind") == "observation"
        and event["value"]["request_id"] == request_id,
    )
    replies = [e["value"] for e in events if e.get("kind") == "observation" and e["value"]["request_id"] == request_id]
    if len(replies) != 1:
        raise RuntimeError(f"observation request {request_id} did not return exactly once")
    return replies[0], [e["hex"] for e in events if e.get("kind") == "rx"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--v4", default="COM7")
    parser.add_argument("--t114", default="COM10")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {
        "started_utc": utc_now(),
        "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "imported_helper_sha256": hashlib.sha256(
            Path(__file__).with_name("o3_usb_bench.py").read_bytes()
        ).hexdigest(),
        "limits": {"identical_reads": 64, "overlap_rf_frames": 12, "gap_transmits": 10},
        "ports": [
            {"device": p.device, "vid": p.vid, "pid": p.pid, "serial": p.serial_number}
            for p in comports() if p.device in (args.v4, args.t114)
        ],
        "transcript": [],
    }
    v4 = peer = None
    try:
        ids = {p["device"]: (p["vid"], p["pid"], p["serial"]) for p in report["ports"]}
        if ids.get(args.v4) != (0x303A, 0x1001, V4_SERIAL) or ids.get(args.t114) != (0x1915, 0x521F, T114_SERIAL):
            raise RuntimeError("bench identities do not match V4 DUT and T114 peer")
        v4 = Port(args.v4, 115200, report["transcript"])
        peer = Port(args.t114, 115200, report["transcript"])
        initial, request_id = discovery(v4, 1)
        profile = request_profile(v4, request_id, initial["boot_id"], 1)
        request_id += 1
        config = bytes.fromhex(profile["config"])
        report["initial"] = initial
        report["profile"] = profile

        peer_initial, peer_request_id = discovery(peer, 1)
        peer_profile = request_profile(peer, peer_request_id, peer_initial["boot_id"], 1)
        if peer_profile["config"] != profile["config"]:
            raise RuntimeError("attached T114 and V4 do not already share the exact runtime PHY")
        report["peer_profile"] = peer_profile

        rejected = bytearray(config)
        rejected[1:5] = struct.pack("<I", 869_525_000)
        v4.write(b"\0" + rejected)
        refused_reply = collect_events(v4, 2, lambda e: e["kind"] == "config")
        if not any(e == {"kind": "config", "status": 4} for e in refused_reply):
            raise RuntimeError("V4 did not explicitly refuse out-of-region retune")
        refusal, request_id = drain(v4, initial["boot_id"], request_id, initial["newest"])
        if len(refusal) != 1 or refusal[0].get("event_tag") != 6:
            raise RuntimeError(f"expected one retained refusal, got {refusal}")
        repeated = []
        for _ in range(64):
            reply = request_cursor(v4, request_id, initial["boot_id"], initial["newest"])
            request_id += 1
            repeated.append(reply)
        stable = [{k: v for k, v in r.items() if k != "request_id"} for r in repeated]
        if any(r != stable[0] for r in stable[1:]):
            raise RuntimeError("identical observation rereads changed retained bytes")
        report["refusal"] = {"reply": refused_reply, "record": refusal[0], "identical_reads": len(repeated)}

        overlap_start = refusal[-1]["next"]
        tx_results = []
        tx_error = []
        def send_peer() -> None:
            try:
                for i in range(12):
                    tx_results.append(tx(peer, i, f"o4-overlap-{i:02}".encode()))
            except Exception as error:
                tx_error.append(f"{type(error).__name__}: {error}")
        worker = threading.Thread(target=send_peer)
        worker.start()
        overlap_reads = 0
        ordinary_rx = []
        while worker.is_alive() and overlap_reads < 64:
            _, seen = cursor_with_rx(v4, request_id, initial["boot_id"], initial["newest"])
            ordinary_rx.extend(seen)
            request_id += 1
            overlap_reads += 1
            # Host workload pacing only: keep collection present across the RF sequence.
            # Device timing conclusions use neither this sleep nor host wall-clock deltas.
            time.sleep(0.25)
        budget_exhausted = worker.is_alive() and overlap_reads == 64
        worker.join(40)
        if budget_exhausted or worker.is_alive() or tx_error or len(tx_results) != 12:
            raise RuntimeError(f"overlap transmitter failed: {tx_error}, sent={len(tx_results)}")
        ordinary_rx.extend(e["hex"] for e in collect_events(v4, 2) if e.get("kind") == "rx")
        expected_payloads = {f"o4-overlap-{i:02}".encode().hex() for i in range(12)}
        if set(ordinary_rx) != expected_payloads or len(ordinary_rx) != 12:
            raise RuntimeError(f"ordinary V4 RX payload mismatch: {ordinary_rx}")
        captured, request_id = drain(v4, initial["boot_id"], request_id, overlap_start)
        captures = sum(r.get("event_tag") == 2 for r in captured)
        if captures != 12:
            raise RuntimeError(f"expected 12 V4 captures under collection load, got {captures}")
        report["overlap"] = {"observation_reads": overlap_reads, "tx": tx_results,
                             "ordinary_rx": ordinary_rx, "captures": captures, "records": captured}

        before_v4_tx = captured[-1]["next"]
        v4_result = tx(v4, 100, b"o4-v4-tx")
        peer_rx = collect_events(peer, 3, lambda e: e["kind"] == "rx")
        if not any(e["kind"] == "rx" and bytes.fromhex(e["hex"]) == b"o4-v4-tx" for e in peer_rx):
            raise RuntimeError("T114 did not receive V4 transmit")
        lifecycle, request_id = drain(v4, initial["boot_id"], request_id, before_v4_tx)
        if [r.get("event_tag") for r in lifecycle] != [1, 4, 5, 0]:
            raise RuntimeError(f"V4 TX lifecycle mismatch: {lifecycle}")
        report["v4_tx"] = {"result": v4_result, "peer_rx": peer_rx, "lifecycle": lifecycle}

        for i in range(10):
            tx(v4, 200 + i, f"o4-gap-{i}".encode())
        gap_records, request_id = drain(v4, initial["boot_id"], request_id, 0)
        gaps = sum(r.get("gap_count", 0) for r in gap_records)
        if gaps <= 0:
            raise RuntimeError("bounded overflow produced no explicit gap")
        report["gap"] = {"transmits": 10, "gap_count": gaps, "records": gap_records}
        report["passed"] = True
        return 0
    except Exception as error:
        report["passed"] = False
        report["error"] = f"{type(error).__name__}: {error}"
        return 1
    finally:
        for port in (peer, v4):
            if port is not None:
                port.close()
        report["finished_utc"] = utc_now()
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
