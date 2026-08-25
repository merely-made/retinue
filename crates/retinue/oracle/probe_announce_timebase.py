"""Black-box probes for RNS announce timebase acceptance.

This driver runs the pinned RNS package as a receiver behind a local
``TCPClientInterface``. It sends hand-built, valid announce packets over the
observed HDLC framing, then reads RNS's persisted ``destination_table``. The
receiver config directory is deliberately retained so every result has a
durable state artifact and every probe uses the non-first-sighting arm.

The announce construction uses only RNS's public identity and destination
APIs plus the already-established wire layout. RNS source is never read.

Run from ``oracle/``::

    ./.venv/Scripts/python.exe -u probe_announce_timebase.py
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any

import RNS


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]

# Fixed sender identity. The receiver is a separate RNS process state: this
# identity is only used to produce signed announces for injection.
SOURCE_SEED = bytes.fromhex(
    "f0ecbba49e783dee14ffc6c9f1e1251efa7d7629e0fa32413c5c59ec2e0f6d6c" * 2
)

FLAG = 0x7E
ESC = 0x7D
ESC_MASK = 0x20
RAND_LEN = 10
TIME_LEN = 5
PAYLOAD_RAND_OFFSET = 74
PAYLOAD_SIGNATURE_OFFSET = 84


def frame(packet: bytes) -> bytes:
    """Encode one observed Reticulum TCP packet in HDLC framing."""

    out = bytearray([FLAG])
    for byte in packet:
        if byte in (FLAG, ESC):
            out.extend((ESC, byte ^ ESC_MASK))
        else:
            out.append(byte)
    out.append(FLAG)
    return bytes(out)


class MsgpackDecoder:
    """Small decoder for the msgpack values RNS writes to destination_table.

    The decoder is intentionally limited to the public observed artifact's
    arrays, binary fields, integers, and floating-point timestamps. It is not
    an RNS implementation and does not inspect any RNS source.
    """

    def __init__(self, data: bytes):
        self.data = data
        self.offset = 0

    def take(self, count: int) -> bytes:
        end = self.offset + count
        if end > len(self.data):
            raise ValueError("truncated msgpack value")
        value = self.data[self.offset : end]
        self.offset = end
        return value

    def integer(self, marker: int) -> int:
        widths = {0xCC: (1, "big"), 0xCD: (2, "big"), 0xCE: (4, "big"), 0xCF: (8, "big")}
        if marker in widths:
            width, order = widths[marker]
            return int.from_bytes(self.take(width), order, signed=False)
        if marker in (0xD0, 0xD1, 0xD2, 0xD3):
            width = {0xD0: 1, 0xD1: 2, 0xD2: 4, 0xD3: 8}[marker]
            return int.from_bytes(self.take(width), "big", signed=True)
        raise ValueError(f"unsupported msgpack integer marker 0x{marker:02x}")

    def value(self) -> Any:
        marker = self.take(1)[0]
        if marker <= 0x7F:
            return marker
        if marker >= 0xE0:
            return marker - 0x100
        if 0x90 <= marker <= 0x9F:
            return [self.value() for _ in range(marker & 0x0F)]
        if 0xA0 <= marker <= 0xBF:
            return self.take(marker & 0x1F).decode("utf-8")
        if marker in (0xC0, 0xC2, 0xC3):
            return {0xC0: None, 0xC2: False, 0xC3: True}[marker]
        if marker in (0xCA, 0xCB):
            import struct

            return struct.unpack(">f" if marker == 0xCA else ">d", self.take(4 if marker == 0xCA else 8))[0]
        if marker in (0xC4, 0xC5, 0xC6):
            width = {0xC4: 1, 0xC5: 2, 0xC6: 4}[marker]
            return self.take(int.from_bytes(self.take(width), "big"))
        if marker in (0xDC, 0xDD):
            width = 2 if marker == 0xDC else 4
            count = int.from_bytes(self.take(width), "big")
            return [self.value() for _ in range(count)]
        if marker in (0xCC, 0xCD, 0xCE, 0xCF, 0xD0, 0xD1, 0xD2, 0xD3):
            return self.integer(marker)
        raise ValueError(f"unsupported msgpack marker 0x{marker:02x}")


def decode_destination_table(path: Path) -> list[Any]:
    if not path.exists():
        return []
    raw = path.read_bytes()
    if not raw:
        return []
    decoder = MsgpackDecoder(raw)
    value = decoder.value()
    if decoder.offset != len(raw):
        raise ValueError(f"destination_table has trailing bytes: {len(raw) - decoder.offset}")
    return value if isinstance(value, list) else []


def hexify(value: Any) -> Any:
    if isinstance(value, bytes):
        return value.hex()
    if isinstance(value, list):
        return [hexify(item) for item in value]
    if isinstance(value, tuple):
        return [hexify(item) for item in value]
    if isinstance(value, dict):
        return {str(key): hexify(item) for key, item in value.items()}
    return value


def destination_entry(table: list[Any], destination: bytes) -> dict[str, Any] | None:
    for row in table:
        if not isinstance(row, list) or len(row) < 6 or row[0] != destination:
            continue
        blobs = row[5] if isinstance(row[5], list) else []
        return {
            "destination_hash": destination.hex(),
            "entry": hexify(row),
            "random_blobs": [blob.hex() for blob in blobs if isinstance(blob, bytes)],
            "random_blob_count": len(blobs),
        }
    return None


def wait_for_entry(path: Path, destination: bytes, minimum_blobs: int) -> dict[str, Any] | None:
    deadline = time.monotonic() + 4.0
    while time.monotonic() < deadline:
        entry = destination_entry(decode_destination_table(path), destination)
        if entry is not None and entry["random_blob_count"] >= minimum_blobs:
            return entry
        time.sleep(0.1)
    return destination_entry(decode_destination_table(path), destination)


def make_announce(
    identity: Any,
    destination: Any,
    timebase: int,
    nonce: bytes,
) -> tuple[bytes, bytes, bytes]:
    """Return a valid raw announce with an exact 40-bit timebase."""

    if len(nonce) != 5:
        raise ValueError("nonce must be exactly five bytes")
    if not 0 <= timebase < (1 << 40):
        raise ValueError("timebase must fit in 40 bits")

    template = destination.announce(send=False)
    if template is None:
        raise RuntimeError("RNS did not produce an announce template")
    if template.raw is None:
        template.pack()

    blob = nonce + timebase.to_bytes(TIME_LEN, "big")
    raw = bytearray(template.raw)
    payload_start = RNS.Reticulum.HEADER_MINSIZE
    rand_start = payload_start + PAYLOAD_RAND_OFFSET
    sig_start = payload_start + PAYLOAD_SIGNATURE_OFFSET
    raw[rand_start : rand_start + RAND_LEN] = blob

    app_data = b""
    signed = destination.hash + identity.get_public_key() + destination.name_hash + blob + app_data
    signature = identity.sign(signed)
    raw[sig_start : sig_start + len(signature)] = signature

    packet = RNS.Packet(None, None)
    packet.raw = bytes(raw)
    packet.unpack()
    if RNS.Identity.validate_announce(packet) is not True:
        raise RuntimeError("constructed announce did not validate under RNS")
    return bytes(raw), blob, destination.hash


class Receiver:
    def __init__(self, config_dir: Path, port: int):
        self.config_dir = config_dir
        self.port = port
        self.server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.server.bind(("127.0.0.1", port))
        self.server.listen(1)
        self.server.settimeout(0.2)
        self.connection: socket.socket | None = None
        self.connected = threading.Event()
        self.stop = threading.Event()
        self.reader_thread = threading.Thread(target=self.accept_and_read, daemon=True)

    def accept_and_read(self) -> None:
        while not self.stop.is_set() and self.connection is None:
            try:
                self.connection, _ = self.server.accept()
                self.connection.settimeout(0.2)
                self.connected.set()
            except TimeoutError:
                continue
        while not self.stop.is_set() and self.connection is not None:
            try:
                if not self.connection.recv(65536):
                    break
            except TimeoutError:
                continue
            except OSError:
                break

    def start(self) -> None:
        (self.config_dir / "config").write_text(
            "[reticulum]\n"
            "  enable_transport = Yes\n"
            "  share_instance = No\n"
            "  panic_on_interface_error = No\n"
            "\n[logging]\n  loglevel = 3\n"
            "\n[interfaces]\n"
            "  [[ProbeReceiver]]\n"
            "    type = TCPClientInterface\n"
            "    enabled = yes\n"
            "    target_host = 127.0.0.1\n"
            f"    target_port = {self.port}\n",
            encoding="utf-8",
        )
        self.reader_thread.start()
        RNS.Reticulum(configdir=str(self.config_dir))
        if not self.connected.wait(8.0):
            raise RuntimeError("RNS TCPClientInterface did not connect to probe receiver")
        time.sleep(0.5)

    def send(self, packet: bytes) -> None:
        if self.connection is None:
            raise RuntimeError("receiver connection is not open")
        self.connection.sendall(frame(packet))

    def close(self) -> None:
        self.stop.set()
        if self.connection is not None:
            try:
                self.connection.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.connection.close()
        self.server.close()


def run_probe(
    base: Path,
    config_dir: Path,
    port: int,
    app: str,
    aspect: str,
    cases: list[tuple[int, bytes]],
) -> dict[str, Any]:
    table_path = config_dir / "storage" / "destination_table"
    records: list[dict[str, Any]] = []
    for index, (timebase, nonce) in enumerate(cases):
        before = None
        sender_json = base / f"{aspect}-{index}-sender.json"
        receiver_json = base / f"{aspect}-{index}-receiver.json"
        sender = subprocess.run(
            [
                sys.executable,
                str(HERE / Path(__file__).name),
                "--sender",
                "--app",
                app,
                "--aspect",
                aspect,
                "--timebase",
                str(timebase),
                "--nonce",
                nonce.hex(),
                "--output",
                str(sender_json),
            ],
            check=False,
            timeout=30,
        )
        if sender.returncode != 0:
            raise RuntimeError(f"sender failed for {aspect} case {index}: {sender.returncode}")
        sent = json.loads(sender_json.read_text(encoding="utf-8"))
        packet = bytes.fromhex(sent["packet_hex"])
        blob = bytes.fromhex(sent["blob_hex"])
        destination_hash = bytes.fromhex(sent["destination_hash"])
        before = destination_entry(decode_destination_table(table_path), destination_hash)
        receiver = subprocess.run(
            [
                sys.executable,
                str(HERE / Path(__file__).name),
                "--receiver",
                "--config-dir",
                str(config_dir),
                "--port",
                str(port),
                "--packet-hex",
                packet.hex(),
                "--destination-hash",
                destination_hash.hex(),
                "--blob-hex",
                blob.hex(),
                "--output",
                str(receiver_json),
            ],
            check=False,
            timeout=30,
        )
        if receiver.returncode != 0:
            raise RuntimeError(f"receiver failed for {aspect} case {index}: {receiver.returncode}")
        expected = (before["random_blob_count"] if before else 0) + 1
        after = destination_entry(decode_destination_table(table_path), destination_hash)
        if after is None:
            after = wait_for_entry(table_path, destination_hash, expected)
        records.append(
            {
                "timebase_seconds": timebase,
                "nonce_hex": nonce.hex(),
                "blob_hex": blob.hex(),
                "packet_hex": packet.hex(),
                "packet_length": len(packet),
                "sender_result": sent,
                "receiver_result": json.loads(receiver_json.read_text(encoding="utf-8")),
                "before": before,
                "after": after,
                "accepted_by_blob_growth": bool(
                    after is not None
                    and after["random_blob_count"] > (before["random_blob_count"] if before else 0)
                ),
            }
        )
    return {"destination_hash": destination_hash.hex(), "records": records}


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, help="result JSON path; defaults under validation/results")
    parser.add_argument("--config-dir", type=Path, help="persistent RNS config directory")
    parser.add_argument("--port", type=int, default=42731)
    parser.add_argument("--sender", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--receiver", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--app", default="retinue", help=argparse.SUPPRESS)
    parser.add_argument("--aspect", help=argparse.SUPPRESS)
    parser.add_argument("--timebase", type=int, help=argparse.SUPPRESS)
    parser.add_argument("--nonce", help=argparse.SUPPRESS)
    parser.add_argument("--packet-hex", help=argparse.SUPPRESS)
    parser.add_argument("--destination-hash", help=argparse.SUPPRESS)
    parser.add_argument("--blob-hex", help=argparse.SUPPRESS)
    return parser.parse_args()


def sender_child(args: argparse.Namespace) -> int:
    if args.output is None or args.aspect is None or args.timebase is None or args.nonce is None:
        raise SystemExit("sender requires --output, --aspect, --timebase, and --nonce")
    output = args.output.resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    config_dir = output.parent / f"sender-config-{args.aspect}"
    config_dir.mkdir(parents=True, exist_ok=True)
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport = No\n  share_instance = No\n\n[interfaces]\n",
        encoding="utf-8",
    )
    RNS.Reticulum(configdir=str(config_dir))
    identity = RNS.Identity.from_bytes(SOURCE_SEED)
    destination = RNS.Destination(
        identity,
        RNS.Destination.IN,
        RNS.Destination.SINGLE,
        args.app,
        args.aspect,
    )
    packet, blob, destination_hash = make_announce(
        identity, destination, args.timebase, bytes.fromhex(args.nonce)
    )
    result = {
        "rns_version": RNS.__version__,
        "packet_hex": packet.hex(),
        "packet_length": len(packet),
        "blob_hex": blob.hex(),
        "destination_hash": destination_hash.hex(),
        "packet_valid": True,
        "rns_source_read": False,
    }
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    # RNS's background workers are not needed after this durable sender
    # artifact exists. The parent owns the receiver process and state.
    os._exit(0)


def receiver_child(args: argparse.Namespace) -> int:
    if (
        args.config_dir is None
        or args.packet_hex is None
        or args.destination_hash is None
        or args.blob_hex is None
        or args.output is None
    ):
        raise SystemExit("receiver requires --config-dir, --packet-hex, --destination-hash, --blob-hex, and --output")
    args.output = args.output.resolve()
    args.config_dir = args.config_dir.resolve()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.config_dir.mkdir(parents=True, exist_ok=True)
    receiver = Receiver(args.config_dir, args.port)
    packet = bytes.fromhex(args.packet_hex)
    destination_hash = bytes.fromhex(args.destination_hash)
    candidate_blob = bytes.fromhex(args.blob_hex)
    result: dict[str, Any] = {
        "rns_version": RNS.__version__,
        "packet_length": len(packet),
        "packet_valid": False,
        "destination_hash": destination_hash.hex(),
        "candidate_blob_hex": candidate_blob.hex(),
        "rns_source_read": False,
    }
    exit_code = 1
    try:
        receiver.start()
        parsed = RNS.Packet(None, None)
        parsed.raw = packet
        parsed.unpack()
        result["packet_valid"] = RNS.Identity.validate_announce(parsed) is True
        receiver.send(packet)
        table_path = args.config_dir / "storage" / "destination_table"
        deadline = time.monotonic() + 4.0
        candidate_observed = False
        while time.monotonic() < deadline:
            entry = destination_entry(decode_destination_table(table_path), destination_hash)
            if entry is not None and candidate_blob.hex() in entry["random_blobs"]:
                candidate_observed = True
                break
            time.sleep(0.1)
        result["in_process_snapshot"] = destination_entry(
            decode_destination_table(table_path), destination_hash
        )
        result["table_path"] = str(table_path)
        result["in_process_candidate_observed"] = candidate_observed
        result["processed_window_complete"] = True
        args.output.write_text(json.dumps(hexify(result), indent=2) + "\n", encoding="utf-8")
        exit_code = 0 if result["packet_valid"] else 1
    except Exception as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
        result["processed_window_complete"] = False
        args.output.write_text(json.dumps(hexify(result), indent=2) + "\n", encoding="utf-8")
    finally:
        receiver.close()
        # The stock store commits destination_table during normal RNS
        # shutdown, but an interface reconnect can keep that call alive.
        # Give it a bounded flush window, then end only this child.
        shutdown = threading.Thread(target=RNS.exit, daemon=True)
        shutdown.start()
        shutdown.join(3.0)
        os._exit(exit_code)


def main() -> int:
    args = parse_args()
    if args.sender:
        return sender_child(args)
    if args.receiver:
        return receiver_child(args)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    base = (args.output.parent if args.output else REPO / "validation" / "results" / f"announce-timebase-{stamp}").resolve()
    output = (args.output or base / "result.json").resolve()
    config_dir = (args.config_dir or base / "rns-config").resolve()
    base.mkdir(parents=True, exist_ok=True)
    if config_dir.exists() and any(config_dir.iterdir()):
        raise SystemExit(f"refusing non-empty persistent config directory: {config_dir}")
    config_dir.mkdir(parents=True, exist_ok=True)

    result: dict[str, Any] = {
        "rns_version": RNS.__version__,
        "method": "persistent RNS receiver; raw valid announces over observed HDLC TCP framing",
        "rns_source_read": False,
        "config_dir": str(config_dir),
        "destination_table": str(config_dir / "storage" / "destination_table"),
        "inputs": {
            "source_seed_hex": SOURCE_SEED.hex(),
            "wire_timebase": "blob[5:10], big-endian whole seconds",
            "nonce_length": 5,
        },
    }
    try:
        table_path = config_dir / "storage" / "destination_table"
        p1 = run_probe(
            base, config_dir, args.port, "retinue", "timebase-p1",
            [(100, b"P1-A0"), (100, b"P1-B0")],
        )
        p2 = run_probe(
            base, config_dir, args.port, "retinue", "timebase-p2",
            [(1 << 39, b"P2-HI"), (2, b"P2-LO")],
        )
        p3 = run_probe(
            base, config_dir, args.port, "retinue", "timebase-p3",
            [(1, b"P3-LO"), (1 << 39, b"P3-HI")],
        )
        result["probes"] = {
            "P1_equal_timestamp": {
                **p1,
                "answer": p1["records"][1]["accepted_by_blob_growth"],
            },
            "P2_high_then_correct": {
                **p2,
                "answer": not p2["records"][1]["accepted_by_blob_growth"],
            },
            "P3_low_then_2pow39": {
                **p3,
                "answer": p3["records"][1]["accepted_by_blob_growth"],
            },
        }
        result["final_table"] = hexify(decode_destination_table(table_path))
        # A probe answer is the observed growth boolean, so False is a valid
        # result (notably P1 may reject equal timestamps). Process success
        # means every case reached a persisted, parseable post-state.
        result["all_answers_present"] = all(
            probe["records"][0]["accepted_by_blob_growth"]
            and all(
                record["receiver_result"].get("packet_valid")
                and record["receiver_result"].get("processed_window_complete")
                for record in probe["records"]
            )
            for probe in result["probes"].values()
        )
        output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result, indent=2))
        return 0 if result["all_answers_present"] else 1
    except Exception as exc:
        result["error"] = f"{type(exc).__name__}: {exc}"
        output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result, indent=2))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
