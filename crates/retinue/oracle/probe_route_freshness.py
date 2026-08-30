"""Black-box probe for stock RNS announce route/freshness admission (P8).

The probe never reads RNS implementation source.  It constructs signed Type-1
announces through public RNS APIs, passes them through stock RNS transport
nodes, records the resulting Type-2 frames, and compares a persistent stock
RNS receiver's ``destination_table`` before and after the candidate set.

Ordinary candidates are injected through a transport that has not seen their
destinations.  Path-response candidates are first cached by another transport
while the receiver is disconnected, then requested with the public
``RNS.Transport.request_path`` API.  This keeps the path-response context real
instead of synthesising context byte 0x0b.

Run from ``oracle/``::

    ./.venv/Scripts/python.exe -u probe_route_freshness.py --profile smoke
    ./.venv/Scripts/python.exe -u probe_route_freshness.py --profile full
    ./.venv/Scripts/python.exe -u probe_route_freshness.py --profile same-blob-diagnostic

The ``expired`` arm is deliberately labelled ``loaded-expired-state``.  It
changes only the observed MessagePack expiry f64 to a past value, proves the
re-encoder is byte-identical before that change, restarts stock RNS, and
records what stock RNS does.  It is not evidence for natural elapsed expiry.

The same-blob diagnostic preserves the observed packet hash list as an artifact,
moves it out of the receiver storage path, and reloads the untouched destination
table before replaying exact signed blobs.  This isolates announce admission from
packet-loop suppression without inspecting RNS implementation source.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import selectors
import shutil
import socket
import struct
import subprocess
import sys
import threading
import time
from dataclasses import asdict, dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Iterable

import RNS

from peer_matrix import RecordingRelay, free_port
from probe_announce_timebase import (
    SOURCE_SEED,
    decode_destination_table,
    destination_entry,
    frame,
    hexify,
    make_announce,
)


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
ORDINARY_CONTEXT = 0x00
PATH_RESPONSE_CONTEXT = 0x0B
HEADER_TYPE_2_MASK = 0x40
PACKET_TYPE_MASK = 0x03
ANNOUNCE_PACKET_TYPE = 0x01
TYPE_1_HEADER_SIZE = 19
TYPE_2_HEADER_SIZE = 35
ANNOUNCE_BLOB_OFFSET = 74
ROLE_TIMEOUT = 45.0


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path) -> str:
    return sha256_bytes(path.read_bytes())


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def write_json_atomic(path: Path, value: object) -> None:
    temporary = path.with_suffix(path.suffix + ".tmp")
    write_json(temporary, value)
    os.replace(temporary, path)


def wait_for_path(path: Path, process: subprocess.Popen[str], description: str, timeout: float = ROLE_TIMEOUT) -> None:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if path.exists():
            return
        if process.poll() is not None:
            raise RuntimeError(f"{description} exited early with {process.returncode}")
        time.sleep(0.05)
    raise RuntimeError(f"timed out waiting for {description}: {path}")


def stop_process(process: subprocess.Popen[str], stop_path: Path, description: str) -> None:
    stop_path.write_text("stop\n", encoding="utf-8")
    try:
        process.wait(timeout=ROLE_TIMEOUT)
    except subprocess.TimeoutExpired as error:
        process.terminate()
        try:
            process.wait(timeout=8)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=8)
        raise RuntimeError(f"{description} did not stop cleanly") from error
    if process.returncode != 0:
        raise RuntimeError(f"{description} failed with {process.returncode}")


def cleanup_process(process: subprocess.Popen[str], stop_path: Path) -> None:
    """Best-effort child cleanup for a probe that is already failing closed."""

    if process.poll() is None:
        stop_path.write_text("stop\n", encoding="utf-8")
        try:
            process.wait(timeout=12.0)
        except subprocess.TimeoutExpired:
            process.terminate()
            try:
                process.wait(timeout=5.0)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5.0)
    close_role_log(process)


def start_role(command: list[str], log_path: Path) -> subprocess.Popen[str]:
    log_path.parent.mkdir(parents=True, exist_ok=True)
    log = log_path.open("w", encoding="utf-8")
    process = subprocess.Popen(
        command,
        cwd=HERE,
        stdout=log,
        stderr=subprocess.STDOUT,
        text=True,
    )
    process._retinue_log = log  # type: ignore[attr-defined]
    return process


def close_role_log(process: subprocess.Popen[str]) -> None:
    log = getattr(process, "_retinue_log", None)
    if log is not None:
        log.close()


def write_transport_config(config_dir: Path, port: int, upstream_port: int | None) -> None:
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "config").write_text(
        "[reticulum]\n"
        "  enable_transport = Yes\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "\n[logging]\n  loglevel = 3\n"
        "\n[interfaces]\n"
        "  [[P8Transport]]\n"
        "    type = TCPServerInterface\n"
        "    enabled = Yes\n"
        "    listen_ip = 127.0.0.1\n"
        f"    listen_port = {port}\n"
        + (
            "  [[P8Upstream]]\n"
            "    type = TCPClientInterface\n"
            "    enabled = Yes\n"
            "    target_host = 127.0.0.1\n"
            f"    target_port = {upstream_port}\n"
            if upstream_port is not None
            else ""
        ),
        encoding="utf-8",
    )


def write_receiver_config(config_dir: Path, interfaces: list[dict[str, object]]) -> None:
    config_dir.mkdir(parents=True, exist_ok=True)
    text = (
        "[reticulum]\n"
        "  enable_transport = Yes\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "\n[logging]\n  loglevel = 3\n"
        "\n[interfaces]\n"
    )
    for interface in interfaces:
        text += (
            f"  [[{interface['name']}]]\n"
            "    type = TCPClientInterface\n"
            "    enabled = Yes\n"
            "    target_host = 127.0.0.1\n"
            f"    target_port = {interface['port']}\n"
        )
    (config_dir / "config").write_text(text, encoding="utf-8")


class PersistentRecordingRelay:
    """Recording TCP relay that accepts sequential receiver restarts on one port."""

    def __init__(self, target_port: int, capture_dir: Path) -> None:
        self.target_port = target_port
        self.client_to_server_path = capture_dir / "receiver-to-transport.bin"
        self.server_to_client_path = capture_dir / "transport-to-receiver.bin"
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(4)
        self.listener.settimeout(0.2)
        self.port = self.listener.getsockname()[1]
        self._stop = threading.Event()
        self._connection_condition = threading.Condition()
        self.connection_count = 0
        self.error: str | None = None
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _connect_target(self) -> socket.socket:
        deadline = time.monotonic() + 15.0
        last_error: OSError | None = None
        while time.monotonic() < deadline and not self._stop.is_set():
            try:
                return socket.create_connection(("127.0.0.1", self.target_port), timeout=0.5)
            except OSError as error:
                last_error = error
                time.sleep(0.05)
        raise RuntimeError(f"persistent relay could not connect to transport: {last_error}")

    def _run_connection(self, client: socket.socket, server: socket.socket, client_capture: Any, server_capture: Any) -> None:
        client.setblocking(False)
        server.setblocking(False)
        selector = selectors.DefaultSelector()
        selector.register(client, selectors.EVENT_READ, (server, client_capture))
        selector.register(server, selectors.EVENT_READ, (client, server_capture))
        try:
            while not self._stop.is_set():
                for key, _mask in selector.select(timeout=0.2):
                    source: socket.socket = key.fileobj
                    destination, capture = key.data
                    try:
                        data = source.recv(64 * 1024)
                    except BlockingIOError:
                        continue
                    if not data:
                        return
                    capture.write(data)
                    capture.flush()
                    destination.sendall(data)
        finally:
            selector.close()

    def _run(self) -> None:
        try:
            with self.client_to_server_path.open("wb") as client_capture, self.server_to_client_path.open(
                "wb"
            ) as server_capture:
                while not self._stop.is_set():
                    try:
                        client, _address = self.listener.accept()
                    except TimeoutError:
                        continue
                    server: socket.socket | None = None
                    try:
                        server = self._connect_target()
                        with self._connection_condition:
                            self.connection_count += 1
                            self._connection_condition.notify_all()
                        self._run_connection(client, server, client_capture, server_capture)
                    finally:
                        client.close()
                        if server is not None:
                            server.close()
        except Exception as error:
            if not self._stop.is_set():
                self.error = repr(error)
        finally:
            self.listener.close()

    def wait_for_connections(self, minimum: int, timeout: float) -> bool:
        deadline = time.monotonic() + timeout
        with self._connection_condition:
            while self.connection_count < minimum and self.error is None:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    break
                self._connection_condition.wait(remaining)
            return self.connection_count >= minimum

    def close(self) -> dict[str, object]:
        self._stop.set()
        try:
            self.listener.close()
        except OSError:
            pass
        self._thread.join(timeout=5.0)
        if self.error:
            raise RuntimeError(f"persistent relay failed: {self.error}")
        return {
            "relay_port": self.port,
            "connection_count": self.connection_count,
            self.client_to_server_path.name: {
                "bytes": self.client_to_server_path.stat().st_size,
                "sha256": sha256_file(self.client_to_server_path),
            },
            self.server_to_client_path.name: {
                "bytes": self.server_to_client_path.stat().st_size,
                "sha256": sha256_file(self.server_to_client_path),
            },
        }


class ReceiverHandler:
    aspect_filter = None

    def __init__(self, events_path: Path | None = None) -> None:
        self.events: list[dict[str, object]] = []
        self.events_path = events_path
        self.lock = threading.Lock()

    def received_announce(self, destination_hash: bytes, announced_identity: Any, app_data: Any) -> None:
        with self.lock:
            self.events.append(
                {
                    "destination_hash": bytes(destination_hash).hex(),
                    "identity_hash": bytes(announced_identity.hash).hex(),
                    "app_data_hex": bytes(app_data).hex() if app_data else "",
                    "observed_monotonic": time.monotonic(),
                }
            )
            self._write_events_locked()

    def write_events(self) -> None:
        with self.lock:
            self._write_events_locked()

    def _write_events_locked(self) -> None:
        if self.events_path is not None:
            write_json_atomic(
                self.events_path,
                {"events": self.events, "rns_version": RNS.__version__},
            )


def bounded_rns_exit() -> None:
    shutdown = threading.Thread(target=RNS.exit, daemon=True)
    shutdown.start()
    shutdown.join(5.0)


def transport_child(args: argparse.Namespace) -> int:
    assert args.config_dir is not None and args.ready is not None and args.stop is not None
    write_transport_config(args.config_dir, args.port, args.upstream_port)
    RNS.Reticulum(configdir=str(args.config_dir))
    time.sleep(0.75)
    args.ready.write_text("ready\n", encoding="utf-8")
    deadline = time.monotonic() + 600.0
    while not args.stop.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    bounded_rns_exit()
    os._exit(0 if args.stop.exists() else 1)


def receiver_child(args: argparse.Namespace) -> int:
    assert args.config_dir is not None and args.ready is not None and args.stop is not None
    interfaces = json.loads(args.interfaces_json or "[]")
    write_receiver_config(args.config_dir, interfaces)
    handler = ReceiverHandler(args.receiver_events)
    RNS.Reticulum(configdir=str(args.config_dir))
    RNS.Transport.register_announce_handler(handler)
    time.sleep(0.75)
    args.ready.write_text("ready\n", encoding="utf-8")

    if args.request_destinations is not None:
        assert args.request_ready is not None and args.request_go is not None and args.requests_done is not None
        destinations = json.loads(args.request_destinations.read_text(encoding="utf-8"))
        args.request_ready.write_text("ready\n", encoding="utf-8")
        deadline = time.monotonic() + ROLE_TIMEOUT
        while not args.request_go.exists() and time.monotonic() < deadline:
            if args.stop.exists():
                break
            time.sleep(0.05)
        if not args.request_go.exists():
            os._exit(2)
        for request in destinations:
            if isinstance(request, str):
                destination = request
                on_interface = None
            else:
                destination = request["destination_hash"]
                interface_name = request["interface_name"]
                matching_interfaces = [
                    interface
                    for interface in RNS.Transport.interfaces
                    if getattr(interface, "name", None) == interface_name
                    or interface_name in str(interface)
                ]
                if len(matching_interfaces) != 1:
                    raise RuntimeError(
                        f"could not resolve request interface {interface_name}: "
                        f"{[str(interface) for interface in RNS.Transport.interfaces]}"
                    )
                on_interface = matching_interfaces[0]
            RNS.Transport.request_path(
                bytes.fromhex(destination),
                on_interface=on_interface,
                recursive=bool(request.get("recursive", False)) if isinstance(request, dict) else False,
            )
            time.sleep(args.request_interval)
        args.requests_done.write_text("done\n", encoding="utf-8")

    deadline = time.monotonic() + 600.0
    while not args.stop.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    handler.write_events()
    bounded_rns_exit()
    os._exit(0 if args.stop.exists() else 1)


def patched_announce(raw: bytes, source_hops: int, context: int = ORDINARY_CONTEXT) -> bytes:
    if not 0 <= source_hops <= 255:
        raise ValueError("source hops must fit in one byte")
    changed = bytearray(raw)
    changed[1] = source_hops
    changed[TYPE_1_HEADER_SIZE - 1] = context
    packet = RNS.Packet(None, None)
    packet.raw = bytes(changed)
    packet.unpack()
    if RNS.Identity.validate_announce(packet) is not True:
        raise RuntimeError("patched announce did not validate under stock RNS")
    return bytes(changed)


@dataclass(frozen=True)
class CellSpec:
    cell_id: str
    aspect: str
    time_relation: str
    nonce_relation: str
    hop_relation: str
    context: str
    route_state: str
    incumbent_timebase: int = 100
    incumbent_nonce_hex: str = b"P8-A0".hex()

    @property
    def candidate_timebase(self) -> int:
        return {"older": 99, "equal": 100, "newer": 101}[self.time_relation]

    @property
    def candidate_nonce_hex(self) -> str:
        return self.incumbent_nonce_hex if self.nonce_relation == "same" else b"P8-B0".hex()

    @property
    def candidate_source_hops(self) -> int:
        return 0


def all_specs(profile: str) -> list[CellSpec]:
    rows: list[CellSpec] = []
    for route_state in ("live", "loaded-expired-state"):
        for context in ("ordinary", "path-response"):
            for time_relation in ("older", "equal", "newer"):
                for nonce_relation in ("same", "new"):
                    for hop_relation in ("better", "equal", "worse"):
                        cell_id = "-".join(
                            (route_state, context, time_relation, nonce_relation, hop_relation)
                        )
                        rows.append(
                            CellSpec(
                                cell_id=cell_id,
                                aspect=f"route-freshness-{len(rows):02d}",
                                time_relation=time_relation,
                                nonce_relation=nonce_relation,
                                hop_relation=hop_relation,
                                context=context,
                                route_state=route_state,
                            )
                        )
    if profile == "full":
        return rows
    if profile == "same-blob-diagnostic":
        return [
            row
            for row in rows
            if row.context == "ordinary"
            and row.time_relation == "equal"
            and row.nonce_relation == "same"
        ]
    wanted = {
        "live-ordinary-older-new-equal",
        "live-ordinary-newer-new-worse",
        "live-path-response-equal-same-better",
        "loaded-expired-state-ordinary-older-new-equal",
        "loaded-expired-state-path-response-newer-same-worse",
    }
    return [row for row in rows if row.cell_id in wanted]


def sender_child(args: argparse.Namespace) -> int:
    assert args.packet_plan is not None and args.output is not None
    plan = json.loads(args.packet_plan.read_text(encoding="utf-8"))
    config_dir = args.output.parent / "sender-config"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport = No\n  share_instance = No\n\n[interfaces]\n",
        encoding="utf-8",
    )
    RNS.Reticulum(configdir=str(config_dir))
    identity = RNS.Identity.from_bytes(SOURCE_SEED)
    packets: dict[str, object] = {"rns_version": RNS.__version__, "cells": {}, "calibration": {}}

    def destination(aspect: str) -> Any:
        return RNS.Destination(
            identity, RNS.Destination.IN, RNS.Destination.SINGLE, "retinue", aspect
        )

    def build(destination: Any, timebase: int, nonce_hex: str, source_hops: int) -> dict[str, object]:
        raw, blob, destination_hash = make_announce(
            identity, destination, timebase, bytes.fromhex(nonce_hex)
        )
        raw = patched_announce(raw, source_hops)
        return {
            "destination_hash": destination_hash.hex(),
            "blob_hex": blob.hex(),
            "source_hops": source_hops,
            "context": ORDINARY_CONTEXT,
            "packet_hex": raw.hex(),
            "packet_sha256": sha256_bytes(raw),
        }

    for calibration in plan["calibration"]:
        target = destination(calibration["aspect"])
        packets["calibration"][calibration["label"]] = build(
            target, 10 + calibration["source_hops"], b"P8-C0".hex(), calibration["source_hops"]
        )
    for cell in plan["cells"]:
        target = destination(cell["aspect"])
        packets["cells"][cell["cell_id"]] = {
            "incumbent": build(
                target, cell["incumbent_timebase"], cell["incumbent_nonce_hex"], 0
            ),
            "candidate": build(
                target, cell["candidate_timebase"], cell["candidate_nonce_hex"], cell["candidate_source_hops"]
            ),
        }
    write_json(args.output, packets)
    os._exit(0)


def decode_hdlc_stream(data: bytes) -> list[bytes]:
    frames: list[bytes] = []
    current = bytearray()
    escaped = False
    for byte in data:
        if byte == 0x7E:
            if current:
                frames.append(bytes(current))
                current.clear()
            escaped = False
        elif byte == 0x7D:
            escaped = True
        elif escaped:
            current.append(byte ^ 0x20)
            escaped = False
        else:
            current.append(byte)
    if current:
        frames.append(bytes(current))
    return frames


def parse_forwarded_announce(raw: bytes) -> dict[str, object] | None:
    if len(raw) < TYPE_2_HEADER_SIZE or raw[0] & PACKET_TYPE_MASK != ANNOUNCE_PACKET_TYPE:
        return None
    header_type_2 = bool(raw[0] & HEADER_TYPE_2_MASK)
    header_size = TYPE_2_HEADER_SIZE if header_type_2 else TYPE_1_HEADER_SIZE
    if len(raw) < header_size + ANNOUNCE_BLOB_OFFSET + 10:
        return None
    packet = RNS.Packet(None, None)
    packet.raw = raw
    try:
        packet.unpack()
        signature_valid = RNS.Identity.validate_announce(packet) is True
    except Exception:
        signature_valid = False
    destination_start = 18 if header_type_2 else 2
    context_offset = 34 if header_type_2 else 18
    return {
        "raw_hex": raw.hex(),
        "sha256": sha256_bytes(raw),
        "header_type": 2 if header_type_2 else 1,
        "signature_valid": signature_valid,
        "hops": raw[1],
        "transport_id_hex": raw[2:18].hex() if header_type_2 else None,
        "destination_hash": raw[destination_start : destination_start + 16].hex(),
        "context": raw[context_offset],
        "blob_hex": raw[header_size + ANNOUNCE_BLOB_OFFSET : header_size + ANNOUNCE_BLOB_OFFSET + 10].hex(),
    }


def parsed_capture(path: Path) -> list[dict[str, object]]:
    return [
        parsed
        for raw in decode_hdlc_stream(path.read_bytes() if path.exists() else b"")
        if (parsed := parse_forwarded_announce(raw)) is not None
    ]


def matching_frames(
    frames: Iterable[dict[str, object]], destination_hash: str, blob_hex: str
) -> list[dict[str, object]]:
    return [
        frame_record
        for frame_record in frames
        if frame_record["destination_hash"] == destination_hash and frame_record["blob_hex"] == blob_hex
    ]


def send_packets(port: int, packets: list[bytes], capture_path: Path, interval: float = 0.15) -> None:
    capture_path.parent.mkdir(parents=True, exist_ok=True)
    encoded = b"".join(frame(packet) for packet in packets)
    capture_path.write_bytes(encoded)
    with socket.create_connection(("127.0.0.1", port), timeout=5.0) as connection:
        for packet in packets:
            connection.sendall(frame(packet))
            time.sleep(interval)
        time.sleep(0.75)


def drain_until_quiet(
    port: int,
    capture_path: Path,
    quiet_seconds: float,
    timeout: float = 120.0,
) -> dict[str, object]:
    """Attach real downstream egress and retain everything emitted until quiet."""

    if quiet_seconds <= 0:
        raise ValueError("drain quiet window must be positive")
    capture_path.parent.mkdir(parents=True, exist_ok=True)
    chunks: list[bytes] = []
    started = time.monotonic()
    last_data = started
    deadline = started + timeout
    failure: Exception | None = None
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=5.0) as connection:
            connection.settimeout(min(0.25, quiet_seconds))
            while True:
                now = time.monotonic()
                if now - last_data >= quiet_seconds:
                    break
                if now >= deadline:
                    raise RuntimeError(
                        f"downstream drain on port {port} did not become quiet within {timeout:.1f}s"
                    )
                try:
                    data = connection.recv(64 * 1024)
                except TimeoutError:
                    continue
                except ConnectionResetError as error:
                    raise RuntimeError(
                        f"downstream drain on port {port} reset before the quiet window"
                    ) from error
                if not data:
                    raise RuntimeError(
                        f"downstream drain on port {port} closed before the quiet window"
                    )
                chunks.append(data)
                last_data = time.monotonic()
    except Exception as error:
        failure = error
    finally:
        raw = b"".join(chunks)
        capture_path.write_bytes(raw)
    if failure is not None:
        raise RuntimeError(
            f"{failure}; retained {len(raw)} bytes at {capture_path}"
        ) from failure

    parsed_announces = [
        parsed
        for frame_bytes in decode_hdlc_stream(raw)
        if (parsed := parse_forwarded_announce(frame_bytes)) is not None
    ]
    return {
        "path": str(capture_path),
        "bytes": len(raw),
        "sha256": sha256_bytes(raw),
        "hdlc_frame_count": len(decode_hdlc_stream(raw)),
        "forwarded_announce_count": len(parsed_announces),
        "quiet_seconds": quiet_seconds,
        "elapsed_seconds": time.monotonic() - started,
    }


def wait_for_cached_blobs(chain: TransportChain, blob_hexes: list[str], timeout: float = 90.0) -> None:
    if not blob_hexes:
        return
    cache_dir = chain.terminal_dir / "rns-config" / "storage" / "cache" / "announces"
    expected = [bytes.fromhex(blob) for blob in blob_hexes]
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        cache_bytes = b"".join(path.read_bytes() for path in cache_dir.glob("*") if path.is_file())
        if all(blob in cache_bytes for blob in expected):
            return
        time.sleep(0.1)
    raise RuntimeError(f"{chain.label} did not cache every path-response candidate")


def wait_for_forwarded_announces(
    capture_path: Path,
    expected: list[tuple[str, str]],
    timeout: float = 90.0,
) -> None:
    """Wait until the terminal relay has recorded every expected announce."""

    deadline = time.monotonic() + timeout
    missing = expected
    while time.monotonic() < deadline:
        frames = parsed_capture(capture_path)
        missing = [
            (destination_hash, blob_hex)
            for destination_hash, blob_hex in expected
            if not matching_frames(frames, destination_hash, blob_hex)
        ]
        if not missing:
            return
        time.sleep(0.1)
    raise RuntimeError(
        f"terminal relay did not record {len(missing)} of {len(expected)} expected announces"
    )


def wait_for_receiver_events(
    events_path: Path,
    expected_destination_hashes: list[str],
    process: subprocess.Popen[str],
    timeout: float = 90.0,
) -> None:
    """Wait until the stock receiver has accepted every expected announce."""

    deadline = time.monotonic() + timeout
    missing = set(expected_destination_hashes)
    while time.monotonic() < deadline:
        if events_path.is_file():
            try:
                observed = {
                    event["destination_hash"]
                    for event in json.loads(events_path.read_text(encoding="utf-8"))["events"]
                }
            except (json.JSONDecodeError, KeyError, TypeError):
                observed = set()
            missing = set(expected_destination_hashes) - observed
            if not missing:
                return
        if process.poll() is not None:
            raise RuntimeError(f"receiver exited before accepting {len(missing)} incumbents")
        time.sleep(0.1)
    raise RuntimeError(f"receiver did not accept {len(missing)} incumbents within {timeout:.1f}s")


def pack_msgpack(value: Any) -> bytes:
    if value is None:
        return b"\xc0"
    if value is False:
        return b"\xc2"
    if value is True:
        return b"\xc3"
    if isinstance(value, int):
        if 0 <= value <= 0x7F:
            return bytes([value])
        if -32 <= value < 0:
            return bytes([value & 0xFF])
        if 0 <= value <= 0xFF:
            return b"\xcc" + value.to_bytes(1, "big")
        if 0 <= value <= 0xFFFF:
            return b"\xcd" + value.to_bytes(2, "big")
        if 0 <= value <= 0xFFFFFFFF:
            return b"\xce" + value.to_bytes(4, "big")
        if 0 <= value <= 0xFFFFFFFFFFFFFFFF:
            return b"\xcf" + value.to_bytes(8, "big")
        for marker, width, minimum, maximum in (
            (b"\xd0", 1, -(1 << 7), (1 << 7) - 1),
            (b"\xd1", 2, -(1 << 15), (1 << 15) - 1),
            (b"\xd2", 4, -(1 << 31), (1 << 31) - 1),
            (b"\xd3", 8, -(1 << 63), (1 << 63) - 1),
        ):
            if minimum <= value <= maximum:
                return marker + value.to_bytes(width, "big", signed=True)
        raise ValueError("integer outside MessagePack range")
    if isinstance(value, float):
        return b"\xcb" + struct.pack(">d", value)
    if isinstance(value, bytes):
        length = len(value)
        if length <= 0xFF:
            return b"\xc4" + bytes([length]) + value
        if length <= 0xFFFF:
            return b"\xc5" + length.to_bytes(2, "big") + value
        return b"\xc6" + length.to_bytes(4, "big") + value
    if isinstance(value, str):
        encoded = value.encode("utf-8")
        length = len(encoded)
        if length <= 31:
            return bytes([0xA0 | length]) + encoded
        raise ValueError("probe encoder only supports short strings")
    if isinstance(value, (list, tuple)):
        length = len(value)
        if length <= 15:
            prefix = bytes([0x90 | length])
        elif length <= 0xFFFF:
            prefix = b"\xdc" + length.to_bytes(2, "big")
        else:
            prefix = b"\xdd" + length.to_bytes(4, "big")
        return prefix + b"".join(pack_msgpack(item) for item in value)
    raise TypeError(f"unsupported MessagePack value: {type(value).__name__}")


def patch_expiry(
    table_path: Path, expired_destinations: set[str], evidence_dir: Path
) -> dict[str, object]:
    original = table_path.read_bytes()
    decoded = decode_destination_table(table_path)
    if pack_msgpack(decoded) != original:
        raise RuntimeError("independent MessagePack re-encoding did not reproduce RNS destination_table")
    changed: list[str] = []
    past = time.time() - 60.0
    for row in decoded:
        if isinstance(row, list) and len(row) >= 5 and isinstance(row[0], bytes):
            destination = row[0].hex()
            if destination in expired_destinations:
                row[4] = past
                changed.append(destination)
    if changed != sorted(expired_destinations):
        if set(changed) != expired_destinations:
            raise RuntimeError("could not find every loaded-expiry destination in the stage-one table")
    modified = pack_msgpack(decoded)
    differing_offsets = [index for index, pair in enumerate(zip(original, modified)) if pair[0] != pair[1]]
    if len(original) != len(modified):
        raise RuntimeError("expiry perturbation changed destination_table length")
    evidence_dir.mkdir(parents=True, exist_ok=True)
    (evidence_dir / "destination_table.original").write_bytes(original)
    (evidence_dir / "destination_table.loaded-expired").write_bytes(modified)
    table_path.write_bytes(modified)
    return {
        "classification": "loaded-expired-state perturbation; not natural elapsed expiry",
        "past_unix_seconds": past,
        "destinations": sorted(changed),
        "original_sha256": sha256_bytes(original),
        "modified_sha256": sha256_bytes(modified),
        "differing_byte_offsets": differing_offsets,
    }


def table_snapshot(table_path: Path, output_dir: Path, name: str) -> list[Any]:
    raw = table_path.read_bytes() if table_path.exists() else b""
    (output_dir / f"{name}.bin").write_bytes(raw)
    decoded = decode_destination_table(table_path) if raw else []
    write_json(output_dir / f"{name}.json", hexify(decoded))
    return decoded


def start_transport(
    base: Path, label: str, upstream_port: int | None = None
) -> tuple[subprocess.Popen[str], int, Path, Path]:
    role_dir = base / label
    role_dir.mkdir(parents=True, exist_ok=True)
    port = free_port()
    ready = role_dir / "ready"
    stop = role_dir / "stop"
    process = start_role(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "--transport",
            "--config-dir",
            str(role_dir / "rns-config"),
            "--port",
            str(port),
            "--ready",
            str(ready),
            "--stop",
            str(stop),
        ]
        + (["--upstream-port", str(upstream_port)] if upstream_port is not None else []),
        role_dir / "transport.log",
    )
    wait_for_path(ready, process, label)
    return process, port, stop, role_dir


@dataclass
class TransportChain:
    label: str
    nodes: list[tuple[subprocess.Popen[str], int, Path, Path]]

    @property
    def root_port(self) -> int:
        return self.nodes[0][1]

    @property
    def terminal_port(self) -> int:
        return self.nodes[-1][1]

    @property
    def terminal_dir(self) -> Path:
        return self.nodes[-1][3]

    def stop(self) -> None:
        for index, (process, _port, stop_path, _role_dir) in reversed(list(enumerate(self.nodes))):
            stop_process(process, stop_path, f"{self.label} node {index + 1}")
            close_role_log(process)

    def cleanup(self) -> None:
        for process, _port, stop_path, _role_dir in reversed(self.nodes):
            cleanup_process(process, stop_path)


def start_chain(base: Path, label: str, depth: int) -> TransportChain:
    if depth < 1:
        raise ValueError("transport chain depth must be positive")
    nodes: list[tuple[subprocess.Popen[str], int, Path, Path]] = []
    upstream_port: int | None = None
    for index in range(depth):
        node = start_transport(base, f"{label}-node-{index + 1}", upstream_port)
        nodes.append(node)
        upstream_port = node[1]
    time.sleep(0.75)
    return TransportChain(label=label, nodes=nodes)


def start_receiver(
    base: Path,
    label: str,
    config_dir: Path,
    interfaces: list[dict[str, object]],
    requests: list[str] | None = None,
    request_interval: float = 0.4,
) -> tuple[subprocess.Popen[str], dict[str, Path]]:
    role_dir = base / label
    role_dir.mkdir(parents=True, exist_ok=True)
    paths = {
        "ready": role_dir / "ready",
        "stop": role_dir / "stop",
        "events": role_dir / "events.json",
        "request_ready": role_dir / "request-ready",
        "request_go": role_dir / "request-go",
        "requests_done": role_dir / "requests-done",
        "requests": role_dir / "requests.json",
    }
    command = [
        sys.executable,
        str(Path(__file__).resolve()),
        "--receiver",
        "--config-dir",
        str(config_dir),
        "--interfaces-json",
        json.dumps(interfaces),
        "--ready",
        str(paths["ready"]),
        "--stop",
        str(paths["stop"]),
        "--receiver-events",
        str(paths["events"]),
    ]
    if requests is not None:
        write_json(paths["requests"], requests)
        command.extend(
            [
                "--request-destinations",
                str(paths["requests"]),
                "--request-ready",
                str(paths["request_ready"]),
                "--request-go",
                str(paths["request_go"]),
                "--requests-done",
                str(paths["requests_done"]),
                "--request-interval",
                str(request_interval),
            ]
        )
    process = start_role(command, role_dir / "receiver.log")
    try:
        wait_for_path(paths["ready"], process, label)
    except Exception:
        cleanup_process(process, paths["stop"])
        close_role_log(process)
        raise
    return process, paths


def run_probe(args: argparse.Namespace) -> int:
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    base = (args.output or REPO / "validation" / "results" / f"route-freshness-{args.profile}-{stamp}").resolve()
    if base.exists() and any(base.iterdir()):
        raise SystemExit(f"refusing non-empty output directory: {base}")
    base.mkdir(parents=True, exist_ok=True)
    specs = all_specs(args.profile)
    calibration = [
        {
            "label": f"transport-depth-{depth}",
            "aspect": f"route-hop-cal-{depth}",
            "source_hops": 0,
            "depth": depth,
            "relation": relation,
        }
        for depth, relation in ((2, "better"), (3, "equal"), (4, "worse"))
    ]
    plan = {
        "profile": args.profile,
        "calibration": calibration,
        "cells": [
            {
                **asdict(spec),
                "candidate_timebase": spec.candidate_timebase,
                "candidate_nonce_hex": spec.candidate_nonce_hex,
                "candidate_source_hops": spec.candidate_source_hops,
            }
            for spec in specs
        ],
    }
    write_json(base / "plan.json", plan)
    sender_result = base / "packets.json"
    sender = subprocess.run(
        [
            sys.executable,
            str(Path(__file__).resolve()),
            "--sender",
            "--packet-plan",
            str(base / "plan.json"),
            "--output",
            str(sender_result),
        ],
        cwd=HERE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=ROLE_TIMEOUT,
        check=False,
    )
    (base / "sender.log").write_text(sender.stdout, encoding="utf-8")
    if sender.returncode != 0:
        raise RuntimeError(f"sender helper failed with {sender.returncode}")
    packets = json.loads(sender_result.read_text(encoding="utf-8"))
    receiver_config = base / "receiver-config"
    table_path = receiver_config / "storage" / "destination_table"

    # Stage one: a natural three-transport path durably seeds every incumbent.
    incumbent_chain = start_chain(base, "incumbent-chain", 3)
    incumbent_dir = incumbent_chain.terminal_dir
    incumbent_relay = PersistentRecordingRelay(incumbent_chain.terminal_port, incumbent_dir)
    receiver_one, receiver_one_paths = start_receiver(
        base,
        "receiver-stage-one",
        receiver_config,
        [{"name": "P8Incumbent", "port": incumbent_relay.port}],
    )
    if not incumbent_relay.wait_for_connections(1, 8.0):
        raise RuntimeError("stage-one receiver did not connect through the recording relay")
    stage_one_packets = [
        bytes.fromhex(packets["cells"][spec.cell_id]["incumbent"]["packet_hex"])
        for spec in specs
    ]
    send_packets(
        incumbent_chain.root_port,
        stage_one_packets,
        base / "incumbent-chain-node-1" / "source-to-transport.bin",
        args.send_interval,
    )
    wait_for_forwarded_announces(
        incumbent_relay.server_to_client_path,
        [
            (
                packets["cells"][spec.cell_id]["incumbent"]["destination_hash"],
                packets["cells"][spec.cell_id]["incumbent"]["blob_hex"],
            )
            for spec in specs
        ],
    )
    wait_for_receiver_events(
        receiver_one_paths["events"],
        [
            packets["cells"][spec.cell_id]["incumbent"]["destination_hash"]
            for spec in specs
        ],
        receiver_one,
    )
    stop_process(receiver_one, receiver_one_paths["stop"], "stage-one receiver")
    close_role_log(receiver_one)
    stage_one_capture_path = incumbent_dir / "transport-to-receiver.stage-one.bin"
    shutil.copyfile(incumbent_dir / "transport-to-receiver.bin", stage_one_capture_path)
    stage_one_table = table_snapshot(table_path, base, "stage-one-destination-table")
    stage_one_frames = parsed_capture(stage_one_capture_path)

    missing_incumbents: list[str] = []
    for spec in specs:
        packet = packets["cells"][spec.cell_id]["incumbent"]
        entry = destination_entry(stage_one_table, bytes.fromhex(packet["destination_hash"]))
        matches = matching_frames(stage_one_frames, packet["destination_hash"], packet["blob_hex"])
        valid_matches = [
            item
            for item in matches
            if item["header_type"] == 2
            and item["context"] == ORDINARY_CONTEXT
            and item["signature_valid"] is True
        ]
        conflicting_matches = [
            item
            for item in matches
            if item["signature_valid"] is True and item not in valid_matches
        ]
        if (
            entry is None
            or packet["blob_hex"] not in entry["random_blobs"]
            or not valid_matches
            or conflicting_matches
        ):
            missing_incumbents.append(spec.cell_id)
    if missing_incumbents:
        raise RuntimeError(f"stage-one incumbent precondition failed: {missing_incumbents}")

    original_packet_hashes: set[bytes] | None = None
    packet_hash_perturbation: dict[str, object] | None = None
    if args.profile == "same-blob-diagnostic":
        packet_hash_path = receiver_config / "storage" / "packet_hashlist.raw"
        if not packet_hash_path.is_file():
            raise RuntimeError("stock RNS did not persist packet_hashlist.raw for diagnostic")
        perturbation_dir = base / "packet-hash-perturbation"
        perturbation_dir.mkdir(parents=True, exist_ok=True)
        preserved_path = perturbation_dir / "packet_hashlist.before.raw"
        original_hash_bytes = packet_hash_path.read_bytes()
        if len(original_hash_bytes) % 32:
            raise RuntimeError("observed packet hash list was not a sequence of 32-byte hashes")
        original_packet_hashes = {
            original_hash_bytes[offset : offset + 32]
            for offset in range(0, len(original_hash_bytes), 32)
        }
        original_bytes = len(original_hash_bytes)
        original_sha256 = sha256_file(packet_hash_path)
        packet_hash_path.replace(preserved_path)
        packet_hash_perturbation = {
            "classification": "loaded packet-hash-list pruning; destination table preserved",
            "original_bytes": original_bytes,
            "original_hash_count": len(original_packet_hashes),
            "original_sha256": original_sha256,
            "preserved_path": str(preserved_path.relative_to(base)),
            "destination_table_sha256": sha256_file(table_path),
        }

    expired_destinations = {
        packets["cells"][spec.cell_id]["candidate"]["destination_hash"]
        for spec in specs
        if spec.route_state == "loaded-expired-state"
    }
    expiry_result = patch_expiry(table_path, expired_destinations, base / "expiry-perturbation")

    # Restart against the same still-live incumbent interface so stock RNS can
    # load the perturbed state without treating every saved interface as gone.
    settle_receiver, settle_paths = start_receiver(
        base,
        "receiver-loaded-expiry-settle",
        receiver_config,
        [{"name": "P8Incumbent", "port": incumbent_relay.port}],
    )
    if not incumbent_relay.wait_for_connections(2, 8.0):
        raise RuntimeError("loaded-expiry receiver did not reconnect to the incumbent interface")
    time.sleep(1.0)
    stop_process(settle_receiver, settle_paths["stop"], "loaded-expiry settle receiver")
    close_role_log(settle_receiver)
    settled_table = table_snapshot(table_path, base, "pre-candidate-destination-table")
    if packet_hash_perturbation is not None:
        reloaded_hash_path = receiver_config / "storage" / "packet_hashlist.raw"
        reloaded_hashes = reloaded_hash_path.read_bytes() if reloaded_hash_path.exists() else b""
        reloaded_evidence = base / "packet-hash-perturbation" / "packet_hashlist.pre-candidate.raw"
        reloaded_evidence.write_bytes(reloaded_hashes)
        packet_hash_perturbation["pre_candidate_bytes"] = len(reloaded_hashes)
        packet_hash_perturbation["pre_candidate_sha256"] = sha256_bytes(reloaded_hashes)
        packet_hash_perturbation["pre_candidate_path"] = str(reloaded_evidence.relative_to(base))
        if len(reloaded_hashes) % 32:
            raise RuntimeError("observed packet hash list was not a sequence of 32-byte hashes")
        reloaded_hash_set = {
            reloaded_hashes[offset : offset + 32]
            for offset in range(0, len(reloaded_hashes), 32)
        }
        destination_set = {
            bytes.fromhex(packets["cells"][spec.cell_id]["incumbent"]["destination_hash"])
            for spec in specs
        }
        incumbent_packet_hashes = {
            row[7]
            for row in settled_table
            if isinstance(row, list)
            and len(row) >= 8
            and row[0] in destination_set
            and isinstance(row[7], bytes)
        }
        original_incumbent_hashes = (original_packet_hashes or set()) & incumbent_packet_hashes
        surviving_incumbent_hashes = reloaded_hash_set & incumbent_packet_hashes
        packet_hash_perturbation["pre_candidate_hash_count"] = len(reloaded_hash_set)
        packet_hash_perturbation["incumbent_packet_hash_count"] = len(incumbent_packet_hashes)
        packet_hash_perturbation["original_incumbent_packet_hashes"] = sorted(
            value.hex() for value in original_incumbent_hashes
        )
        packet_hash_perturbation["surviving_incumbent_packet_hashes"] = sorted(
            value.hex() for value in surviving_incumbent_hashes
        )
        if len(incumbent_packet_hashes) != len(specs):
            raise RuntimeError("could not identify every incumbent route packet hash")
        if original_incumbent_hashes != incumbent_packet_hashes:
            raise RuntimeError("pre-pruning packet hash list did not contain every incumbent hash")
        if surviving_incumbent_hashes:
            raise RuntimeError("same-blob incumbent packet hashes survived the pruning diagnostic")

    # Stage two uses natural two-, three-, and four-transport paths.  The
    # terminal router in every path-response arm has therefore learned a real
    # forwarded Type-2 route rather than only a direct Type-1 cache artifact.
    # Each
    # relation has one fresh chain and disjoint destinations, so its router
    # state cannot pre-filter another relation's candidate.
    depth_by_relation = {"better": 2, "equal": 3, "worse": 4}
    candidate_chains = {
        relation: start_chain(base, f"candidate-{relation}-chain", depth)
        for relation, depth in depth_by_relation.items()
    }
    path_specs = [spec for spec in specs if spec.context == "path-response"]
    for relation, chain in candidate_chains.items():
        seed_packets = [
            bytes.fromhex(packets["cells"][spec.cell_id]["candidate"]["packet_hex"])
            for spec in path_specs
            if spec.hop_relation == relation
        ]
        send_packets(
            chain.root_port,
            seed_packets,
            base / f"candidate-{relation}-chain-node-1" / "path-seed-source.bin",
            args.send_interval,
        )
    for relation, chain in candidate_chains.items():
        wait_for_cached_blobs(
            chain,
            [
                packets["cells"][spec.cell_id]["candidate"]["blob_hex"]
                for spec in path_specs
                if spec.hop_relation == relation
            ],
        )
    cache_artifacts: list[dict[str, object]] = []
    for relation, chain in candidate_chains.items():
        cache_dir = chain.terminal_dir / "rns-config" / "storage" / "cache" / "announces"
        cache_artifacts.extend(
            {
                "relation": relation,
                "path": str(path.relative_to(base)),
                "bytes": path.stat().st_size,
                "sha256": sha256_file(path),
            }
            for path in sorted(cache_dir.glob("*"))
            if path.is_file()
        )

    # A disconnected wait cannot establish that a transport's egress queue is
    # empty: the terminal has no downstream interface to drain through.  Give
    # every terminal a real passive TCP peer, retain the emitted bytes, and
    # require an observed quiet window before the persistent receiver connects.
    candidate_relays: dict[str, RecordingRelay] = {}
    try:
        drain_artifacts = {
            relation: drain_until_quiet(
                chain.terminal_port,
                chain.terminal_dir / "pre-receiver-drain.bin",
                args.settle_seconds,
            )
            for relation, chain in candidate_chains.items()
        }

        for relation, chain in candidate_chains.items():
            candidate_relays[relation] = RecordingRelay(
                chain.terminal_port,
                chain.terminal_dir,
                "receiver-to-transport",
                "transport-to-receiver",
            )
        request_destinations = [
            {
                "destination_hash": packets["cells"][spec.cell_id]["candidate"]["destination_hash"],
                "interface_name": f"P8Candidate{spec.hop_relation.title()}",
                "recursive": True,
            }
            for spec in path_specs
        ]
        receiver_two, receiver_two_paths = start_receiver(
            base,
            "receiver-stage-two",
            receiver_config,
            [{"name": "P8Incumbent", "port": incumbent_relay.port}]
            + [
                {
                    "name": f"P8Candidate{relation.title()}",
                    "port": candidate_relays[relation].port,
                }
                for relation in ("better", "equal", "worse")
            ],
            requests=request_destinations,
            request_interval=args.request_interval,
        )
    except Exception:
        for relay in candidate_relays.values():
            relay.close()
        for chain in candidate_chains.values():
            chain.cleanup()
        incumbent_chain.cleanup()
        incumbent_relay.close()
        raise
    try:
        if not all(relay.accepted.wait(8.0) for relay in candidate_relays.values()):
            raise RuntimeError("stage-two receiver did not connect through every recording relay")
        if not incumbent_relay.wait_for_connections(3, 8.0):
            raise RuntimeError("stage-two receiver did not retain the incumbent interface")
        wait_for_path(receiver_two_paths["request_ready"], receiver_two, "receiver request-ready")
        time.sleep(1.0)
        contamination = []
        for relation, chain in candidate_chains.items():
            path_pre_request = chain.terminal_dir / "transport-to-receiver.bin"
            pre_request_bytes = path_pre_request.read_bytes() if path_pre_request.exists() else b""
            (chain.terminal_dir / "transport-to-receiver.pre-request.bin").write_bytes(pre_request_bytes)
            pre_request_frames = [
                parsed
                for raw in decode_hdlc_stream(pre_request_bytes)
                if (parsed := parse_forwarded_announce(raw)) is not None
            ]
            for spec in path_specs:
                packet = packets["cells"][spec.cell_id]["candidate"]
                if matching_frames(pre_request_frames, packet["destination_hash"], packet["blob_hex"]):
                    contamination.append(f"{relation}:{spec.cell_id}")
        if contamination:
            raise RuntimeError(f"path-response candidates arrived before request: {contamination}")
        receiver_two_paths["request_go"].write_text("go\n", encoding="utf-8")
        wait_for_path(receiver_two_paths["requests_done"], receiver_two, "path requests", timeout=120.0)
        time.sleep(max(10.0, args.settle_seconds))
        ordinary_specs = [spec for spec in specs if spec.context == "ordinary"]
        for relation, chain in candidate_chains.items():
            calibration_packet = packets["calibration"][f"transport-depth-{depth_by_relation[relation]}"]
            ordinary_candidate_packets = [bytes.fromhex(calibration_packet["packet_hex"])] + [
                bytes.fromhex(packets["cells"][spec.cell_id]["candidate"]["packet_hex"])
                for spec in ordinary_specs
                if spec.hop_relation == relation
            ]
            send_packets(
                chain.root_port,
                ordinary_candidate_packets,
                base / f"candidate-{relation}-chain-node-1" / "ordinary-source.bin",
                args.send_interval,
            )
        time.sleep(max(10.0, args.settle_seconds))
        stop_process(receiver_two, receiver_two_paths["stop"], "stage-two receiver")
        close_role_log(receiver_two)
        for chain in candidate_chains.values():
            chain.stop()
        incumbent_chain.stop()
    finally:
        cleanup_process(receiver_two, receiver_two_paths["stop"])
        for chain in candidate_chains.values():
            chain.cleanup()
        incumbent_chain.cleanup()
        candidate_captures = {
            relation: relay.close() for relation, relay in candidate_relays.items()
        }
        incumbent_capture = incumbent_relay.close()
    final_table = table_snapshot(table_path, base, "final-destination-table")
    candidate_frames = {
        relation: parsed_capture(chain.terminal_dir / "transport-to-receiver.bin")
        for relation, chain in candidate_chains.items()
    }

    calibration_result: dict[str, object] = {}
    observed_hops: list[int] = []
    for item in calibration:
        packet = packets["calibration"][item["label"]]
        frames = candidate_frames[item["relation"]]
        matches = matching_frames(frames, packet["destination_hash"], packet["blob_hex"])
        valid_matches = [
            match
            for match in matches
            if match["header_type"] == 2
            and match["context"] == ORDINARY_CONTEXT
            and match["signature_valid"] is True
        ]
        if len(valid_matches) != 1:
            raise RuntimeError(
                f"invalid transport-depth calibration for {item['label']}: {matches}"
            )
        observed = int(valid_matches[0]["hops"])
        observed_hops.append(observed)
        calibration_result[item["label"]] = {
            "transport_depth": item["depth"],
            "forwarded": valid_matches[0],
        }
    if not observed_hops[0] < observed_hops[1] < observed_hops[2]:
        raise RuntimeError(f"transport-depth calibration did not yield controlled relations: {observed_hops}")

    cells: list[dict[str, object]] = []
    invalid_cells: list[str] = []
    for spec in specs:
        pair = packets["cells"][spec.cell_id]
        incumbent = pair["incumbent"]
        candidate = pair["candidate"]
        before = destination_entry(stage_one_table, bytes.fromhex(candidate["destination_hash"]))
        pre_candidate = destination_entry(settled_table, bytes.fromhex(candidate["destination_hash"]))
        after = destination_entry(final_table, bytes.fromhex(candidate["destination_hash"]))
        source_frames = candidate_frames[spec.hop_relation]
        candidates = matching_frames(source_frames, candidate["destination_hash"], candidate["blob_hex"])
        expected_context = PATH_RESPONSE_CONTEXT if spec.context == "path-response" else ORDINARY_CONTEXT
        valid_frames = [
            item
            for item in candidates
            if item["header_type"] == 2
            and item["context"] == expected_context
            and item["signature_valid"] is True
        ]
        conflicting_forwarded_frames = [
            item
            for item in candidates
            if item["signature_valid"] is True and item not in valid_frames
        ]
        incumbent_frames = [
            item
            for item in matching_frames(
                stage_one_frames, incumbent["destination_hash"], incumbent["blob_hex"]
            )
            if item["header_type"] == 2
            and item["context"] == ORDINARY_CONTEXT
            and item["signature_valid"] is True
        ]
        observed_relation = None
        if incumbent_frames and valid_frames:
            incumbent_hops = int(incumbent_frames[0]["hops"])
            candidate_hops = int(valid_frames[0]["hops"])
            observed_relation = "better" if candidate_hops < incumbent_hops else "worse" if candidate_hops > incumbent_hops else "equal"
        blob_relation = "same" if candidate["blob_hex"] == incumbent["blob_hex"] else "new"
        new_blob_admitted = bool(
            blob_relation == "new"
            and after is not None
            and candidate["blob_hex"] in after["random_blobs"]
            and (pre_candidate is None or candidate["blob_hex"] not in pre_candidate["random_blobs"])
        )
        route_transition = bool(
            blob_relation == "same"
            and pre_candidate is not None
            and after is not None
            and pre_candidate["entry"][1:5] + pre_candidate["entry"][6:8]
            != after["entry"][1:5] + after["entry"][6:8]
        )
        observable_admission = new_blob_admitted or route_transition
        valid = bool(
            incumbent_frames
            and valid_frames
            and not conflicting_forwarded_frames
            and observed_relation == spec.hop_relation
        )
        if not valid:
            invalid_cells.append(spec.cell_id)
        cell = {
            "cell": asdict(spec),
            "destination_hash": candidate["destination_hash"],
            "incumbent_blob_hex": incumbent["blob_hex"],
            "candidate_blob_hex": candidate["blob_hex"],
            "blob_relation": blob_relation,
            "before_loaded_expiry_perturbation": before,
            "pre_candidate": pre_candidate,
            "after": after,
            "matching_forwarded_frames": valid_frames,
            "conflicting_forwarded_frames": conflicting_forwarded_frames,
            "observed_hop_relation": observed_relation,
            "new_blob_admitted": new_blob_admitted,
            "same_blob_route_transition": route_transition,
            "observable_admission": observable_admission,
            "outcome": "observable-admission" if observable_admission else "no-observable-admission",
            "valid_measurement": valid,
            "caveat": (
                "exact-same-blob ordinary replay may be suppressed by the persistent packet-hash window"
                if blob_relation == "same"
                and spec.context == "ordinary"
                and args.profile != "same-blob-diagnostic"
                else "persistent packet-hash list moved aside before receiver reload"
                if blob_relation == "same"
                and spec.context == "ordinary"
                and args.profile == "same-blob-diagnostic"
                else None
            ),
        }
        cell_dir = base / "cells" / spec.cell_id
        write_json(cell_dir / "manifest.json", cell)
        cells.append(cell)

    result = {
        "probe": "P8 stock-RNS forwarded announce route/freshness matrix",
        "profile": args.profile,
        "rns_version": packets["rns_version"],
        "rns_source_read": False,
        "topology": {
            "incumbent": "source Type-1 -> three stock RNS transports -> recorded Type-2 -> persistent stock RNS receiver",
            "ordinary_candidate": "source Type-1 -> measured two/three/four-transport chain -> recorded Type-2 -> persistent receiver",
            "path_response_candidate": "candidate traverses and is cached by its measured chain before receiver connects; public request_path -> real context-0x0b Type-2 -> persistent receiver",
        },
        "calibration": calibration_result,
        "expiry": expiry_result,
        "packet_hash_perturbation": packet_hash_perturbation,
        "captures": {
            "incumbent": incumbent_capture,
            "candidate_by_hop_relation": candidate_captures,
            "path_cache": cache_artifacts,
            "pre_receiver_drain": drain_artifacts,
        },
        "cells": cells,
        "cell_count": len(cells),
        "invalid_cells": invalid_cells,
        "all_measurements_valid": not invalid_cells,
        "scope": {
            "distinct_destination_per_cell": True,
            "shared_receiver_across_cells": True,
            "candidate_chain_per_hop_relation": True,
            "natural_elapsed_expiry_measured": False,
            "loaded_expired_state_measured": True,
            "same_blob_packet_hash_diagnostic_run": args.profile == "same-blob-diagnostic",
        },
    }
    write_json(base / "result.json", result)
    print(
        json.dumps(
            {
                "result": str(base / "result.json"),
                "cell_count": result["cell_count"],
                "invalid_cells": result["invalid_cells"],
                "all_measurements_valid": result["all_measurements_valid"],
            },
            indent=2,
            sort_keys=True,
        )
    )
    return 0 if result["all_measurements_valid"] else 1


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--profile",
        choices=("smoke", "full", "same-blob-diagnostic"),
        default="smoke",
    )
    parser.add_argument("--output", type=Path, help="fresh result directory under validation/results by default")
    parser.add_argument("--send-interval", type=float, default=1.1)
    parser.add_argument("--request-interval", type=float, default=1.1)
    parser.add_argument("--settle-seconds", type=float, default=3.0)
    parser.add_argument("--sender", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--transport", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--receiver", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--packet-plan", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--config-dir", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--port", type=int, default=0, help=argparse.SUPPRESS)
    parser.add_argument("--upstream-port", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--ready", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--stop", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--interfaces-json", help=argparse.SUPPRESS)
    parser.add_argument("--receiver-events", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--request-destinations", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--request-ready", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--request-go", type=Path, help=argparse.SUPPRESS)
    parser.add_argument("--requests-done", type=Path, help=argparse.SUPPRESS)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.sender:
        return sender_child(args)
    if args.transport:
        return transport_child(args)
    if args.receiver:
        return receiver_child(args)
    return run_probe(args)


if __name__ == "__main__":
    raise SystemExit(main())
