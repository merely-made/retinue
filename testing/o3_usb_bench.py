"""Bounded observation and synthetic RF bench for two already-flashed boards.

This runner copies the T114's exact PHY definition into the V4's runtime
configuration, then uses observation and synthetic TX commands. It does not
change persisted settings or the region selection.
"""
from __future__ import annotations

import argparse
import json
import struct
import sys
import time
import zlib
from datetime import datetime, timezone
from pathlib import Path

import serial

CMD_TX = 0x01
EVENT_RX = 0x81
EVENT_TX = 0x82
EVENT_CONFIG = 0x83
EVENT_OBSERVATION = 0x86
OBS_VERSION = 1
MAX_PAYLOAD = 129
MAX_REPLY = 136
MAX_RECORD = 64
READ_TIMEOUT = 0.20
SCENARIO_LIMIT_S = 180.0


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def observation_request(request_id: int, boot_id: int = 0, after: int = 0, kind: int = 0) -> bytes:
    raw = struct.pack(">BBIQQ", OBS_VERSION, kind, request_id, boot_id, after)
    return b"\x00\x04" + raw.hex().encode("ascii") + b"\x00"


def crc_ok(frame: bytes) -> bool:
    return len(frame) >= 7 and struct.unpack(">I", frame[-4:])[0] == zlib.crc32(frame[:-4]) & 0xFFFFFFFF


def parse_observation(frame: bytes) -> dict:
    if len(frame) < 7 or frame[0] != EVENT_OBSERVATION:
        raise ValueError("not observation event")
    payload_len = struct.unpack(">H", frame[1:3])[0]
    if not 15 <= payload_len <= MAX_PAYLOAD or len(frame) != 7 + payload_len:
        raise ValueError("invalid observation length")
    if not crc_ok(frame):
        raise ValueError("observation CRC failure")
    payload = frame[3:-4]
    if payload[0] != OBS_VERSION or payload[1] != 0:
        raise ValueError("unsupported observation payload")
    if len(payload) < 65:
        raise ValueError("observation payload missing cursor metadata")
    status = payload[6]
    result = {
        "request_id": struct.unpack(">I", payload[2:6])[0],
        "status": status,
        "boot_id": struct.unpack(">Q", payload[7:15])[0],
        "oldest": struct.unpack(">Q", payload[15:23])[0],
        "newest": struct.unpack(">Q", payload[23:31])[0],
        "next": struct.unpack(">Q", payload[31:39])[0],
        "recorded": struct.unpack(">Q", payload[39:47])[0],
        "overwritten": struct.unpack(">Q", payload[47:55])[0],
        "encode_failed": struct.unpack(">Q", payload[55:63])[0],
        "profile_count": payload[63],
        "record_len": payload[64],
        "raw_record": payload[65:].hex(),
    }
    if result["record_len"] > MAX_RECORD or len(payload) != 65 + result["record_len"]:
        raise ValueError("invalid observation record length")
    raw = payload[65:]
    if raw:
        if len(raw) < 33 or raw[:2] != b"O\x01" or int.from_bytes(raw[3:5], "big") != len(raw):
            raise ValueError("invalid inner observation framing")
        if not crc_ok(raw):
            raise ValueError("observation record CRC failure")
        if raw[2] not in (0, 1) or (raw[2] == 0 and len(raw) < 34) or (raw[2] == 1 and len(raw) != 33):
            raise ValueError("invalid inner observation class/length")
        if int.from_bytes(raw[5:13], "big") != result["boot_id"]:
            raise ValueError("inner boot mismatch")
        result["sequence"] = int.from_bytes(raw[13:21], "big")
        if raw[2] == 0:
            result["event_tag"] = raw[29]
    result["record_class"] = raw[2] if raw else None
    if raw and raw[2] == 1 and len(raw) >= 29:
        result["gap_count"] = struct.unpack(">Q", raw[21:29])[0]
    return result


def parse_profile(frame: bytes) -> dict:
    if len(frame) != 39 or frame[0] != EVENT_OBSERVATION or struct.unpack(">H", frame[1:3])[0] != 32:
        raise ValueError("invalid profile event length")
    if not crc_ok(frame):
        raise ValueError("profile CRC failure")
    payload = frame[3:-4]
    if payload[0:2] != bytes((OBS_VERSION, 1)):
        raise ValueError("unsupported profile payload")
    return {"request_id": struct.unpack(">I", payload[2:6])[0], "status": payload[6],
            "boot_id": struct.unpack(">Q", payload[7:15])[0], "profile_id": payload[15],
            "config": payload[16:32].hex()}


class Port:
    def __init__(self, name: str, baud: int, transcript: list[dict]):
        self.name = name
        self.transcript = transcript
        self.serial = serial.Serial(
            port=None, baudrate=baud, timeout=READ_TIMEOUT, write_timeout=2,
        )
        self.serial.port = name
        self.serial.dtr = False
        self.serial.rts = False
        self.serial.open()
        time.sleep(0.15)
        self.serial.dtr = True
        time.sleep(0.05)
        self.serial.reset_input_buffer()
        self.buffer = bytearray()

    def record(self, direction: str, data: bytes) -> None:
        self.transcript.append({"utc": utc_now(), "perf": time.perf_counter(), "port": self.name,
                                "direction": direction, "hex": data.hex()})

    def write(self, data: bytes) -> None:
        if self.serial.write(data) != len(data):
            raise IOError("partial host command write")
        self.record("tx", data)

    def read(self, limit: int = 1024) -> bytes:
        data = self.serial.read(limit)
        if data:
            self.record("rx", data)
        return data

    def close(self) -> None:
        if self.serial.is_open:
            self.serial.dtr = False
            time.sleep(0.15)
            self.serial.close()


def collect_events(port: Port, seconds: float = 2.0, until=None) -> list[dict]:
    deadline = time.monotonic() + seconds
    buffer = port.buffer
    events: list[dict] = []
    while time.monotonic() < deadline:
        buffer.extend(port.read())
        while True:
            try:
                start = next(i for i, b in enumerate(buffer) if b in (EVENT_RX, EVENT_TX, EVENT_CONFIG, EVENT_OBSERVATION))
            except StopIteration:
                buffer.clear()
                break
            if start:
                del buffer[:start]
            marker = buffer[0]
            need = 7 if marker == EVENT_OBSERVATION else 2
            if len(buffer) < need:
                break
            if marker == EVENT_OBSERVATION:
                length = struct.unpack(">H", buffer[1:3])[0]
                if not 15 <= length <= MAX_PAYLOAD:
                    del buffer[:3]
                    continue
                total = 7 + length
                if len(buffer) < total:
                    break
                frame = bytes(buffer[:total])
                del buffer[:total]
                try:
                    value = parse_profile(frame) if frame[4] == 1 else parse_observation(frame)
                    event = {"kind": "profile" if frame[4] == 1 else "observation", "value": value}
                    events.append(event)
                    if until is not None and until(event):
                        return events
                except ValueError as error:
                    events.append({"kind": "observation_error", "error": str(error), "hex": frame.hex()})
            elif marker == EVENT_CONFIG:
                event = {"kind": "config", "status": buffer[1]}
                events.append(event)
                del buffer[:2]
                if until is not None and until(event):
                    return events
            elif marker == EVENT_TX:
                if len(buffer) < 4:
                    break
                event = {"kind": "tx", "status": buffer[1], "frame_len": int.from_bytes(buffer[2:4], "little")}
                events.append(event)
                del buffer[:4]
                if until is not None and until(event):
                    return events
            else:
                if len(buffer) < 7:
                    break
                length = int.from_bytes(buffer[1:3], "little")
                total = 7 + length
                if len(buffer) < total:
                    break
                event = {"kind": "rx", "length": length, "hex": bytes(buffer[7:total]).hex()}
                events.append(event)
                del buffer[:total]
                if until is not None and until(event):
                    return events
    return events


def tx(port: Port, sequence: int, payload: bytes) -> dict:
    if len(payload) > 255:
        raise ValueError("synthetic payload exceeds direct-PHY limit")
    port.write(b"\x00\x01" + struct.pack("<H", len(payload)) + payload)
    for event in collect_events(port, 2.0, lambda event: event["kind"] == "tx"):
        if event["kind"] == "tx":
            if event["status"] != 0 or event["frame_len"] != len(payload):
                raise RuntimeError(f"TX refused on {port.name}: {event}")
            time.sleep(0.25)
            return {"sequence": sequence, "length": len(payload), "status": event["status"]}
    raise TimeoutError(f"TX acknowledgement timeout on {port.name}")


def request_cursor(port: Port, request_id: int, boot: int = 0, after: int = 0) -> dict:
    port.write(observation_request(request_id, boot, after))
    for event in collect_events(port, 2.0, lambda event: event.get("kind") == "observation" and event["value"]["request_id"] == request_id):
        if event["kind"] == "observation":
            value = event["value"]
            if value["request_id"] == request_id:
                return value
    raise TimeoutError(f"observation request {request_id} timed out on {port.name}")


def request_profile(port: Port, request_id: int, boot: int, profile_id: int) -> dict:
    port.write(observation_request(request_id, boot, profile_id, 1))
    for event in collect_events(port, 2.0, lambda event: event.get("kind") == "profile" and event["value"]["request_id"] == request_id):
        if event["kind"] != "profile":
            continue
        if event["value"]["request_id"] == request_id:
            value = event["value"]
            if value["status"] != 0 or value["boot_id"] != boot or value["profile_id"] != profile_id:
                raise RuntimeError(f"invalid profile reply: {value}")
            return value
    raise TimeoutError(f"profile request {request_id} timed out on {port.name}")


def drain(port: Port, boot: int, request_id: int, start_after: int = 0, limit: int = 64) -> tuple[list[dict], int]:
    records = []
    after = start_after
    for _ in range(limit):
        reply = request_cursor(port, request_id, boot, after)
        request_id += 1
        if reply["status"] != 0 or reply["boot_id"] != boot:
            raise RuntimeError(f"observation status {reply['status']}: {reply}")
        if not reply["record_len"]:
            return records, request_id
        if reply["sequence"] != after + 1 or reply["next"] <= after:
            raise ValueError("cursor failed to advance contiguously")
        records.append(reply)
        after = reply["next"]
    raise RuntimeError("observation drain page limit exceeded")


def discovery(port: Port, request_id: int) -> tuple[dict, int]:
    reply = request_cursor(port, request_id)
    if reply["status"] != 0 or not reply["boot_id"]:
        raise RuntimeError(f"invalid discovery reply: {reply}")
    return reply, request_id + 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--t114", default="COM10")
    parser.add_argument("--v4", default="COM6")
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("--output", type=Path, default=Path("o3_usb_bench.json"))
    args = parser.parse_args()
    transcript: list[dict] = []
    report = {"started_utc": utc_now(), "limits": {"scenario_seconds": SCENARIO_LIMIT_S, "pressure_requests": 256},
              "ports": {"t114": args.t114, "v4": args.v4}, "scenarios": [], "parsed_records": [],
              "transcript": transcript}
    t114 = v4 = None
    try:
        started = time.monotonic()
        def check_limit() -> None:
            if time.monotonic() - started > SCENARIO_LIMIT_S:
                raise TimeoutError("bench total runtime limit exceeded")

        t114 = Port(args.t114, args.baud, transcript)
        v4 = Port(args.v4, args.baud, transcript)
        discovery_reply, request_id = discovery(t114, 1)
        initial_boot = discovery_reply["boot_id"]
        baseline_after = discovery_reply["newest"]
        profiles = []
        for profile_id in range(1, discovery_reply["profile_count"] + 1):
            profiles.append(request_profile(t114, request_id, discovery_reply["boot_id"], profile_id))
            request_id += 1
        report["scenarios"].append({"name": "initial_discovery", "boot_id": discovery_reply["boot_id"],
                                    "profile_count": discovery_reply["profile_count"], "profiles": profiles})
        if len(profiles) != 1:
            raise RuntimeError("bench requires one unambiguous T114 PHY profile")
        config = bytes.fromhex(profiles[0]["config"])
        if len(config) != 16 or config[0] != 2:
            raise ValueError("invalid PHY configuration command")
        v4.write(b"\x00" + config)
        events = collect_events(v4, 2.0, lambda event: event["kind"] == "config")
        if not any(event == {"kind": "config", "status": 0} for event in events):
            raise RuntimeError("V4 did not accept the exact T114 runtime PHY")
        report["scenarios"].append({"name": "v4_runtime_phy", "config": config.hex()})
        # Send six baseline packets while the T114 handle is closed, then reconnect and drain.
        t114.close(); t114 = None
        baseline = [tx(v4, i, f"o3-baseline-{i}".encode()) for i in range(6)]
        t114 = Port(args.t114, args.baud, transcript)
        discovery_reply, request_id = discovery(t114, request_id)
        if discovery_reply["boot_id"] != initial_boot:
            raise RuntimeError("board rebooted during baseline")
        baseline_records, request_id = drain(t114, discovery_reply["boot_id"], request_id, baseline_after)
        baseline_captures = sum(r.get("event_tag") == 2 for r in baseline_records)
        if baseline_captures != 6:
            raise RuntimeError(f"baseline expected 6 captures, observed {baseline_captures}; verify exact runtime PHY")
        cursor_after = baseline_records[-1]["next"] if baseline_records else 0
        report["parsed_records"].extend(baseline_records)
        report["scenarios"].append({"name": "baseline_closed_collector", "tx": baseline, "records": len(baseline_records), "captures": baseline_captures})
        normal = []
        for i in range(6):
            check_limit()
            normal.append(tx(v4, 10 + i, f"o3-normal-{i}".encode()))
            time.sleep(0.25)
            records, request_id = drain(t114, discovery_reply["boot_id"], request_id, cursor_after)
            if sum(r.get("event_tag") == 2 for r in records) != 1:
                raise RuntimeError("normal collection did not retain exactly one capture")
            if records:
                cursor_after = records[-1]["next"]
            report["parsed_records"].extend(records)
            report["scenarios"].append({"name": "normal_packet", "index": i, "tx": normal[-1], "records": len(records)})
        # Pressure is deliberately bounded and records bytes without interpreting the result as a backpressure proof.
        sent_pressure = 0
        pressure_deadline = time.monotonic() + 5.0
        for i in range(256):
            if time.monotonic() >= pressure_deadline:
                break
            try:
                t114.write(observation_request(request_id + i))
                sent_pressure += 1
            except serial.SerialTimeoutException:
                break
        time.sleep(0.10)
        pressure_bytes = bytearray()
        deadline = time.monotonic() + 5.0
        while time.monotonic() < deadline:
            pressure_bytes.extend(t114.read())
        report["scenarios"].append({"name": "no_reader_pressure", "requests": sent_pressure, "drained_bytes": len(pressure_bytes)})
        t114.close(); t114 = None
        time.sleep(0.15)
        t114 = Port(args.t114, args.baud, transcript)
        discovery_reply, request_id = discovery(t114, request_id + 256)
        if discovery_reply["boot_id"] != initial_boot:
            raise RuntimeError("board rebooted during reader pressure")
        report["scenarios"].append({"name": "dtr_reconnect_discovery", "boot_id": discovery_reply["boot_id"]})
        t114.close(); t114 = None
        for i in range(40):
            check_limit()
            tx(v4, 100 + i, f"o3-gap-{i}".encode())
            time.sleep(0.25)
        t114 = Port(args.t114, args.baud, transcript)
        discovery_reply, request_id = discovery(t114, request_id)
        gap_records, request_id = drain(t114, discovery_reply["boot_id"], request_id)
        if not any(r.get("gap_count", 0) > 0 for r in gap_records):
            raise RuntimeError("overflow experiment produced no explicit gap")
        report["parsed_records"].extend(gap_records)
        report["scenarios"].append({"name": "overwrite_gap", "tx": 40, "records": len(gap_records),
                                    "gap_or_capture_records": sum(bool(r["raw_record"]) for r in gap_records)})
        final_tx = tx(t114, 200, b"o3-t114-tx")
        time.sleep(0.25)
        final_records, _ = drain(t114, discovery_reply["boot_id"], request_id, 0)
        if [r.get("event_tag") for r in final_records[-4:]] != [1, 4, 5, 0]:
            raise RuntimeError("missing matched listening/TX/return lifecycle")
        report["parsed_records"].extend(final_records)
        report["scenarios"].append({"name": "t114_tx_lifecycle", "tx": final_tx, "records": len(final_records)})
        report["finished_utc"] = utc_now()
        return 0
    except Exception as error:
        report["error"] = {"type": type(error).__name__, "message": str(error)}
        return 1
    finally:
        if t114 is not None: t114.close()
        if v4 is not None: v4.close()
        args.output.write_text(json.dumps(report, indent=2), encoding="utf-8")


if __name__ == "__main__":
    sys.exit(main())
