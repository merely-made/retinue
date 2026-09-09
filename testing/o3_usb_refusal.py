"""Measure rejected retune, unchanged listening, and read-only flash counts.

Requires the observation/flashcounts T114 firmware in US915 modem mode.
The deliberately out-of-region configuration must be rejected before SPI.
No persisted setting is changed. V4 receives the original runtime PHY and
sends three synthetic frames after rejection to check continued reception.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import struct
import time
from pathlib import Path

from serial.tools.list_ports import comports

from o3_usb_bench import (
    Port, collect_events, discovery, drain, request_cursor, request_profile, tx, utc_now,
)


def probe(port: Port, command: bytes, pattern: bytes) -> dict:
    port.write(command + b"\n")
    data = bytearray()
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline:
        data.extend(port.read())
        match = re.search(pattern, data)
        if match:
            return {"raw_hex": data.hex(), "fields": {
                k: int(v) for k, v in match.groupdict().items()
            }}
    raise TimeoutError(f"probe {command!r} missing expected response: {data!r}")


def flashcounts(port: Port) -> dict:
    result = probe(port, b"flashcounts", rb"flashcounts erases=(?P<erases>\d+) writes=(?P<writes>\d+)\r\n")
    if any(v == 0xFFFFFFFF for v in result["fields"].values()):
        raise RuntimeError("saturated flash counter cannot establish a delta")
    return result


def air(port: Port) -> dict:
    return probe(port, b"air", rb"air region=US915 duty=\d+ms listen=\w+ armed=(?P<armed>\d+) armfail=(?P<armfail>\d+) rxok=(?P<rxok>\d+) rxerr=(?P<rxerr>\d+) rxbad=(?P<rxbad>\d+) txok=(?P<txok>\d+) txerr=(?P<txerr>\d+)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--t114", default="COM10")
    parser.add_argument("--v4", default="COM6")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    transcript: list[dict] = []
    report = {"started_utc": utc_now(), "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "ports": [{"device": p.device, "vid": p.vid, "pid": p.pid, "serial": p.serial_number}
                        for p in comports() if p.device in (args.t114, args.v4)],
              "limits": {"observation_reads": 64, "rf_frames": 3}, "transcript": transcript}
    board = peer = None
    try:
        ids = {p["device"]: (p["vid"], p["pid"]) for p in report["ports"]}
        if args.t114 == args.v4 or ids.get(args.t114) != (0x1915, 0x521F) or ids.get(args.v4) != (0x303A, 0x1001):
            raise RuntimeError("bench port identities do not match T114 and V4")
        board = Port(args.t114, 115200, transcript)
        before, request_id = discovery(board, 1)
        report["before"] = before
        if before["profile_count"] != 1:
            raise RuntimeError("bench requires one unambiguous initial PHY")
        original = request_profile(board, request_id, before["boot_id"], 1)
        request_id += 1
        report["profile"] = original
        config = bytes.fromhex(original["config"])
        if config[0] != 2 or not 902_000_000 <= int.from_bytes(config[1:5], "little") <= 928_000_000:
            raise RuntimeError("expected an exact US915 profile")
        report["flash_before"] = flashcounts(board)
        report["air_before"] = air(board)
        rejected = bytearray(config)
        rejected[1:5] = struct.pack("<I", 869_525_000)
        board.write(b"\0" + rejected)
        result = collect_events(board, 2, lambda e: e["kind"] == "config")
        report["rejected_config"] = {"hex": rejected.hex(), "response": result}
        if not any(e == {"kind": "config", "status": 4} for e in result):
            raise RuntimeError("out-of-region profile was not explicitly refused")
        records, request_id = drain(board, before["boot_id"], request_id, before["newest"])
        report["refusal_records"] = records
        if len(records) != 1 or records[0].get("event_tag") != 6:
            raise RuntimeError("expected exactly one refusal and no listening interruption")
        raw = bytes.fromhex(records[0]["raw_record"])
        if len(raw) != 40 or raw[30:32] != bytes((1, 3)) or int.from_bytes(raw[32:36], "big") == 0:
            raise RuntimeError("expected Retune/InvalidProfile with nonzero work id")
        # Read the retained refusal repeatedly; each reply must preserve its bytes
        # and cursor metadata. Request ids are transport correlation only.
        reads = []
        started = time.perf_counter()
        for _ in range(64):
            reply = request_cursor(board, request_id, before["boot_id"], before["newest"])
            request_id += 1
            if {k: v for k, v in reply.items() if k != "request_id"} != {k: v for k, v in records[0].items() if k != "request_id"}:
                raise RuntimeError("repeated observation read changed the retained snapshot")
            reads.append(reply)
        report["repeated_reads"] = {"count": len(reads), "host_elapsed_seconds": time.perf_counter() - started,
                                    "raw_record_bytes": len(raw), "all_identical": True}
        report["flash_after_reads"] = flashcounts(board)
        report["air_after_reads"] = air(board)
        if report["flash_before"]["fields"] != report["flash_after_reads"]["fields"]:
            raise RuntimeError("flash mutation occurred during rejection/observation collection")
        if report["air_before"]["fields"] != report["air_after_reads"]["fields"]:
            raise RuntimeError("radio counters changed during rejection/observation collection")
        if request_profile(board, request_id, before["boot_id"], 1)["config"] != original["config"]:
            raise RuntimeError("rejected profile changed the registry")
        request_id += 1
        peer = Port(args.v4, 115200, transcript)
        peer.write(b"\0" + config)
        if not any(e == {"kind": "config", "status": 0} for e in collect_events(peer, 2, lambda e: e["kind"] == "config")):
            raise RuntimeError("V4 did not accept original PHY")
        report["rf_tx"] = [tx(peer, i, f"o3-refusal-recovery-{i}".encode()) for i in range(3)]
        recovered, request_id = drain(board, before["boot_id"], request_id, records[-1]["next"])
        report["recovery_records"] = recovered
        if len(recovered) != 3 or any(r.get("event_tag") != 2 for r in recovered):
            raise RuntimeError("expected three captures with uninterrupted listening after refusal")
        report["flash_after_rf"] = flashcounts(board)
        if report["flash_after_rf"]["fields"] != report["flash_before"]["fields"]:
            raise RuntimeError("RF recovery changed flash counters")
        report["passed"] = True
        return 0
    except Exception as error:
        report["passed"] = False
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        for port in (peer, board):
            if port is not None:
                port.close()
        report["finished_utc"] = utc_now()
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
