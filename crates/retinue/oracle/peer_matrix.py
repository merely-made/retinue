"""Peer-lane H8: capture the three black-box Reticulum announce pairings.

This is deliberately a local gate, like the existing ``interop_*.py`` drivers.
It does not import Prns into Retinue.  Instead it starts an untouched, pinned
``prnsd`` executable with a disposable configuration, speaks to it through a
normal shared-instance connection using stock RNS, and records the TCP bytes
between the two implementations.

The three cases are:

* stock RNS 1.4.2 <-> Retinue (the control pairing),
* pinned Prns <-> Retinue, and
* pinned Prns <-> stock RNS 1.4.2.

Each case proves an announce in both directions.  A localhost relay writes one
raw stream per direction and the result manifest records its SHA-256, the exact
commands, ports, source revisions, and each assertion.  Results live in
``validation/results/`` because they are execution evidence, not deterministic
fixtures for CI.

Run from ``crates/retinue/oracle`` after building the pinned Prns daemon:

    cargo build --manifest-path ..\\..\\..\\..\\Prns\\prnsd\\Cargo.toml -p prnsd \\
        --no-default-features --features tokio-host,observability
    .\\.venv\\Scripts\\python.exe -u peer_matrix.py

``--prns-root`` selects a clean Prns worktree and ``--prnsd`` selects its
daemon executable explicitly.  Otherwise the driver uses ``repos/Prns`` and
looks for its normal debug artifact locations.  It rejects a Prns tree whose
commit is not H8's pinned revision or whose working tree is dirty, preserving
the independent-process claim.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import selectors
import shutil
import socket
import subprocess
import sys
import threading
import time
from dataclasses import dataclass, field
from datetime import UTC, datetime
from pathlib import Path
from typing import Callable

import RNS


HERE = Path(__file__).resolve().parent
RETINUE_CRATE = HERE.parent
RETINUE_REPO = RETINUE_CRATE.parents[1]
PRNS_ROOT = RETINUE_REPO.parent / "Prns"
PINNED_PRNS_REVISION = "72b6b30d27cac910ce20d370e1dc711fe9b95955"

RETINUE_ASPECT = "retinue.interop"
RETINUE_APP_DATA = b"hello-from-retinue"
RNS_APP_DATA = b"hello-from-rns"
PRNS_APP_DATA = b"hello-from-prns"
RNS_SEED = bytes.fromhex(
    "f0ecbba49e783dee14ffc6c9f1e1251efa7d7629e0fa32413c5c59ec2e0f6d6c" * 2
)
PRNS_APP_SEED = bytes([0x22]) * 64


class GateError(RuntimeError):
    """A failed receipt assertion, with a message suitable for the JSON record."""


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def run_checked(command: list[str], cwd: Path) -> str:
    completed = subprocess.run(
        command,
        cwd=cwd,
        check=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    return completed.stdout.strip()


def git_state(path: Path) -> dict[str, object]:
    return {
        "revision": run_checked(["git", "rev-parse", "HEAD"], path),
        "status": run_checked(["git", "status", "--porcelain=v1"], path).splitlines(),
    }


def free_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def write_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")


@dataclass
class LineProcess:
    """A process whose combined output is retained both on disk and in memory."""

    command: list[str]
    cwd: Path
    log_path: Path
    environment: dict[str, str] | None = None
    process: subprocess.Popen[str] = field(init=False)
    lines: list[str] = field(default_factory=list, init=False)
    _pump: threading.Thread = field(init=False)

    def __post_init__(self) -> None:
        self.process = subprocess.Popen(
            self.command,
            cwd=self.cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
            env=self.environment,
        )

        def pump() -> None:
            assert self.process.stdout is not None
            with self.log_path.open("w", encoding="utf-8") as log:
                for raw_line in self.process.stdout:
                    line = raw_line.rstrip("\r\n")
                    self.lines.append(line)
                    log.write(raw_line)
                    log.flush()

        self._pump = threading.Thread(target=pump, daemon=True)
        self._pump.start()

    def wait_for(self, predicate: Callable[[], bool], timeout: float, description: str) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            if self.process.poll() is not None:
                raise GateError(
                    f"process exited before {description} (exit {self.process.returncode}): "
                    + "\n".join(self.lines[-20:])
                )
            time.sleep(0.05)
        raise GateError(f"timed out waiting for {description}: " + "\n".join(self.lines[-20:]))

    def stop(self) -> None:
        if self.process.poll() is None:
            self.process.terminate()
            try:
                self.process.wait(timeout=8)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=8)
        self._pump.join(timeout=2)


class RecordingRelay:
    """One TCP hop that persists raw client->server and server->client streams."""

    def __init__(
        self,
        target_port: int,
        capture_dir: Path,
        client_to_server_name: str,
        server_to_client_name: str,
    ) -> None:
        self.target_port = target_port
        self.capture_dir = capture_dir
        self.client_to_server_path = capture_dir / f"{client_to_server_name}.bin"
        self.server_to_client_path = capture_dir / f"{server_to_client_name}.bin"
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.listener.settimeout(0.2)
        self.port = self.listener.getsockname()[1]
        self.accepted = threading.Event()
        self.finished = threading.Event()
        self.error: str | None = None
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)
        self._thread.start()

    def _connect_target(self) -> socket.socket:
        deadline = time.monotonic() + 15
        last_error: OSError | None = None
        while time.monotonic() < deadline and not self._stop.is_set():
            try:
                return socket.create_connection(("127.0.0.1", self.target_port), timeout=0.5)
            except OSError as error:
                last_error = error
                time.sleep(0.05)
        raise GateError(f"relay could not connect to 127.0.0.1:{self.target_port}: {last_error}")

    def _run(self) -> None:
        client: socket.socket | None = None
        server: socket.socket | None = None
        try:
            while not self._stop.is_set():
                try:
                    client, _address = self.listener.accept()
                    break
                except TimeoutError:
                    continue
            if client is None:
                return
            self.accepted.set()
            server = self._connect_target()
            client.setblocking(False)
            server.setblocking(False)
            with self.client_to_server_path.open("wb") as client_to_server, self.server_to_client_path.open(
                "wb"
            ) as server_to_client:
                selector = selectors.DefaultSelector()
                selector.register(client, selectors.EVENT_READ, (server, client_to_server))
                selector.register(server, selectors.EVENT_READ, (client, server_to_client))
                while not self._stop.is_set():
                    ready = selector.select(timeout=0.2)
                    if not ready:
                        continue
                    for key, _mask in ready:
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
        except Exception as error:  # surfaced synchronously by close()
            self.error = repr(error)
        finally:
            for endpoint in (client, server, self.listener):
                if endpoint is not None:
                    try:
                        endpoint.close()
                    except OSError:
                        pass
            self.finished.set()

    def close(self) -> dict[str, object]:
        self._stop.set()
        try:
            self.listener.close()
        except OSError:
            pass
        self._thread.join(timeout=5)
        if self.error:
            raise GateError(f"relay failed: {self.error}")
        captures: dict[str, object] = {"relay_port": self.port}
        for path in (self.client_to_server_path, self.server_to_client_path):
            if not path.exists():
                path.write_bytes(b"")
            captures[path.name] = {"bytes": path.stat().st_size, "sha256": sha256_file(path)}
        return captures


class AnnounceHandler:
    def __init__(self, aspect: str | None, expected_data: bytes | None) -> None:
        self.aspect_filter = aspect
        self.expected_data = expected_data
        self.event = threading.Event()
        self.received: dict[str, object] = {}

    def received_announce(self, destination_hash, announced_identity, app_data) -> None:
        data = bytes(app_data) if app_data else b""
        self.received = {
            "destination": destination_hash.hex(),
            "identity": announced_identity.hash.hex(),
            "app_data_hex": data.hex(),
        }
        if self.expected_data is None or data == self.expected_data:
            self.event.set()


def rns_config(*, target_port: int | None, shared: bool, instance_port: int | None = None, control_port: int | None = None) -> str:
    reticulum = [
        "[reticulum]",
        "  enable_transport = No",
        f"  share_instance = {'Yes' if shared else 'No'}",
        "  panic_on_interface_error = No",
    ]
    if instance_port is not None:
        reticulum.append(f"  shared_instance_port = {instance_port}")
    if control_port is not None:
        reticulum.append(f"  instance_control_port = {control_port}")
    text = "\n".join(reticulum) + "\n\n[logging]\n  loglevel = 3\n"
    if target_port is not None:
        text += (
            "\n[interfaces]\n  [[peer]]\n    type = TCPClientInterface\n"
            "    enabled = Yes\n    target_host = 127.0.0.1\n"
            f"    target_port = {target_port}\n"
        )
    return text


def rns_transport_config(listen_port: int) -> str:
    return (
        "[reticulum]\n  enable_transport = Yes\n  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "\n[logging]\n  loglevel = 3\n"
        "\n[interfaces]\n  [[transport]]\n    type = TCPServerInterface\n"
        "    enabled = Yes\n    listen_ip = 127.0.0.1\n"
        f"    listen_port = {listen_port}\n"
    )


def prns_config(
    *,
    shared_port: int,
    control_port: int,
    target_port: int | None,
    listen_port: int | None,
    transport: bool = False,
) -> str:
    text = (
        "[reticulum]\n"
        f"  enable_transport = {'Yes' if transport else 'No'}\n"
        "  share_instance = Yes\n"
        "  panic_on_interface_error = No\n"
        f"  shared_instance_port = {shared_port}\n"
        f"  instance_control_port = {control_port}\n"
        "\n[logging]\n  loglevel = 4\n  logtimestamps = No\n"
        "\n[interfaces]\n  [[peer]]\n"
    )
    if target_port is not None:
        return (
            text
            + "    type = TCPClientInterface\n    enabled = Yes\n"
            + "    target_host = 127.0.0.1\n"
            + f"    target_port = {target_port}\n"
        )
    assert listen_port is not None
    return (
        text
        + "    type = TCPServerInterface\n    enabled = Yes\n"
        + "    listen_ip = 127.0.0.1\n"
        + f"    listen_port = {listen_port}\n"
    )


def retinue_example_command(example: str) -> list[str]:
    """Use an already-built receipt executable when Cargo's shared target is busy."""

    extension = ".exe" if os.name == "nt" else ""
    target_dir = Path(os.environ["CARGO_TARGET_DIR"]) if "CARGO_TARGET_DIR" in os.environ else None
    if target_dir is not None:
        executable = target_dir / "debug" / "examples" / f"{example}{extension}"
        if executable.is_file():
            return [str(executable)]
    return ["cargo", "run", "--quiet", "--example", example]


def start_retinue(case_dir: Path) -> tuple[LineProcess, int]:
    process = LineProcess(
        retinue_example_command("interop_tcp"),
        RETINUE_CRATE,
        case_dir / "retinue.log",
    )
    port: int | None = None

    def found_port() -> bool:
        nonlocal port
        for line in process.lines:
            if line.startswith("LISTENING "):
                port = int(line.split()[1])
                return True
        return False

    process.wait_for(found_port, 120, "Retinue TCP listener")
    assert port is not None
    return process, port


def start_announce_logger(case_dir: Path, label: str, port: int) -> LineProcess:
    environment = os.environ.copy()
    environment["RETINUE_LABEL"] = label
    environment["RETINUE_ADDR"] = f"127.0.0.1:{port}"
    return LineProcess(
        retinue_example_command("announce_logger"),
        RETINUE_CRATE,
        case_dir / f"retinue-{label}.log",
        environment,
    )


def start_prnsd(prnsd: Path, config_dir: Path, case_dir: Path, shared_port: int) -> LineProcess:
    process = LineProcess(
        [str(prnsd), "run", "--service", "--config", str(config_dir), "--log-format", "json"],
        PRNS_ROOT,
        case_dir / "prnsd.log",
        prns_environment(case_dir),
    )
    def shared_port_open() -> bool:
        try:
            with socket.create_connection(("127.0.0.1", shared_port), timeout=0.2):
                return True
        except OSError:
            return False

    process.wait_for(shared_port_open, 30, "Prns shared instance port")
    return process


def launch_rns_announce(
    config_dir: Path,
    app_data: bytes,
    expected_from_retinue: bool,
    *,
    identity_seed: bytes = RNS_SEED,
    expected_aspect: str = RETINUE_ASPECT,
    expected_data: bytes = RETINUE_APP_DATA,
) -> dict[str, object]:
    """Run one stock RNS application on either a direct or shared interface."""

    RNS.Reticulum(configdir=str(config_dir), require_shared_instance=expected_from_retinue)
    instance = RNS.Reticulum.get_instance()
    if expected_from_retinue and not instance.is_connected_to_shared_instance:
        raise GateError("stock RNS did not attach to the Prns shared instance")

    handler = AnnounceHandler(expected_aspect, expected_data)
    RNS.Transport.register_announce_handler(handler)
    identity = RNS.Identity.from_bytes(identity_seed)
    destination = RNS.Destination(
        identity, RNS.Destination.IN, RNS.Destination.SINGLE, "peer", "matrix"
    )
    time.sleep(1.0)
    destination.announce(app_data=app_data)
    expected = handler.event.wait(timeout=15)
    return {
        "rns_shared_instance": bool(instance.is_connected_to_shared_instance),
        "rns_sent_app_data_hex": app_data.hex(),
        "received_retinue_announce": handler.received,
        "received_expected_data": expected,
    }


def write_case_result(case_dir: Path, result: dict[str, object]) -> None:
    write_json(case_dir / "result.json", result)


def prns_environment(case_dir: Path) -> dict[str, str]:
    environment = os.environ.copy()
    environment["PRNSD_STATE_DIR"] = str(case_dir / "prnsd-state")
    return environment


def run_prns_command(
    prnsd: Path, case_dir: Path, log_name: str, arguments: list[str], *, timeout: float = 30
) -> dict[str, object]:
    command = [str(prnsd), *arguments]
    completed = subprocess.run(
        command,
        cwd=PRNS_ROOT,
        env=prns_environment(case_dir),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=timeout,
    )
    log_path = case_dir / log_name
    log_path.write_text(completed.stdout, encoding="utf-8")
    details = {"command": command, "exit_code": completed.returncode, "log": log_name}
    if completed.returncode:
        raise GateError(f"Prns command failed: {details}: {completed.stdout}")
    return details


def retinue_destination(lines: list[str]) -> str:
    for line in lines:
        if line.startswith("SENT_ANNOUNCE "):
            return line.removeprefix("SENT_ANNOUNCE ").strip()
    raise GateError("Retinue did not report its announce destination")


def prns_path_table(prnsd: Path, case_dir: Path, destination: str) -> dict[str, object]:
    details = run_prns_command(
        prnsd,
        case_dir,
        "prnsd-path.json",
        ["path", "--config", str(case_dir / "prns-config"), "--table", "--json"],
    )
    output = (case_dir / "prnsd-path.json").read_text(encoding="utf-8").lower()
    if destination.lower() not in output:
        raise GateError(f"Prns path table did not contain peer destination {destination}: {output}")
    return {**details, "required_destination": destination}


def seed_prns_pages(prnsd: Path, case_dir: Path, config_dir: Path) -> dict[str, object]:
    """Materialize Prns's shipped page destination before asking it to announce."""

    return run_prns_command(
        prnsd,
        case_dir,
        "prnsd-nnpages-seed.log",
        ["nnpages", "seed", "--config", str(config_dir)],
    )


def case_rns_retinue(case_dir: Path, _prnsd: Path) -> dict[str, object]:
    retinue, retinue_port = start_retinue(case_dir)
    relay: RecordingRelay | None = None
    try:
        relay = RecordingRelay(retinue_port, case_dir, "rns-to-retinue", "retinue-to-rns")
        config_dir = case_dir / "rns-config"
        config_dir.mkdir()
        (config_dir / "config").write_text(
            rns_config(target_port=relay.port, shared=False), encoding="utf-8"
        )
        rns_result = launch_rns_announce(config_dir, RNS_APP_DATA, expected_from_retinue=False)
        retinue.wait_for(
            lambda: "VALIDATED_RNS_ANNOUNCE" in retinue.lines,
            15,
            "Retinue validation of stock RNS announce",
        )
        if not rns_result["received_expected_data"]:
            raise GateError(f"stock RNS did not validate Retinue announce: {rns_result}")
        return {"rns": rns_result, "captures": relay.close()}
    finally:
        if relay is not None:
            try:
                relay.close()
            except GateError:
                pass
        retinue.stop()


def case_prns_retinue(case_dir: Path, prnsd: Path) -> dict[str, object]:
    retinue, retinue_port = start_retinue(case_dir)
    relay: RecordingRelay | None = None
    prns: LineProcess | None = None
    try:
        relay = RecordingRelay(retinue_port, case_dir, "prns-to-retinue", "retinue-to-prns")
        shared_port, control_port = free_port(), free_port()
        config_dir = case_dir / "prns-config"
        config_dir.mkdir()
        (config_dir / "config").write_text(
            prns_config(
                shared_port=shared_port,
                control_port=control_port,
                target_port=relay.port,
                listen_port=None,
            ),
            encoding="utf-8",
        )
        prns = start_prnsd(prnsd, config_dir, case_dir, shared_port)
        prns.wait_for(relay.accepted.is_set, 15, "Prns TCP connection through recording relay")
        retinue.wait_for(
            lambda: any(line.startswith("SENT_ANNOUNCE ") for line in retinue.lines),
            15,
            "Retinue announce emission",
        )
        peer_path = prns_path_table(prnsd, case_dir, retinue_destination(retinue.lines))
        page_seed = seed_prns_pages(prnsd, case_dir, config_dir)
        announce = run_prns_command(
            prnsd,
            case_dir,
            "prnsd-nnpages-announce.log",
            ["nnpages", "announce", "--config", str(config_dir)],
        )
        retinue.wait_for(
            lambda: "VALIDATED_RNS_ANNOUNCE" in retinue.lines,
            15,
            "Retinue validation of Prns daemon announce",
        )
        return {
            "prns_observed_retinue": peer_path,
            "prns_page_seed": page_seed,
            "prns_announcement": announce,
            "captures": relay.close(),
        }
    finally:
        if relay is not None:
            try:
                relay.close()
            except GateError:
                pass
        if prns is not None:
            prns.stop()
        retinue.stop()


def case_prns_rns(case_dir: Path, prnsd: Path) -> dict[str, object]:
    """Exercise the untouched Prns daemon as stock RNS's direct TCP peer."""

    shared_port, control_port, prns_listen_port = free_port(), free_port(), free_port()
    config_dir = case_dir / "prns-config"
    config_dir.mkdir()
    (config_dir / "config").write_text(
        prns_config(
            shared_port=shared_port,
            control_port=control_port,
            target_port=None,
            listen_port=prns_listen_port,
        ),
        encoding="utf-8",
    )
    relay: RecordingRelay | None = None
    prns: LineProcess | None = None
    try:
        prns = start_prnsd(prnsd, config_dir, case_dir, shared_port)
        relay = RecordingRelay(prns_listen_port, case_dir, "rns-to-prns", "prns-to-rns")
        direct_config = case_dir / "rns-config"
        direct_config.mkdir()
        (direct_config / "config").write_text(
            rns_config(target_port=relay.port, shared=False), encoding="utf-8"
        )
        RNS.Reticulum(configdir=str(direct_config))
        direct_handler = AnnounceHandler("nomadnetwork.node", None)
        RNS.Transport.register_announce_handler(direct_handler)
        identity = RNS.Identity.from_bytes(RNS_SEED)
        destination = RNS.Destination(
            identity, RNS.Destination.IN, RNS.Destination.SINGLE, "peer", "matrix"
        )
        time.sleep(1.0)
        page_seed = seed_prns_pages(prnsd, case_dir, config_dir)
        announce = run_prns_command(
            prnsd,
            case_dir,
            "prnsd-nnpages-announce.log",
            ["nnpages", "announce", "--config", str(config_dir)],
        )
        if not direct_handler.event.wait(timeout=15):
            raise GateError(
                "stock RNS did not validate the announce sent through pinned Prns: "
                + repr(direct_handler.received)
            )
        destination.announce(app_data=RNS_APP_DATA)
        peer_path = prns_path_table(prnsd, case_dir, destination.hash.hex())
        prns.wait_for(relay.accepted.is_set, 15, "stock RNS TCP connection through recording relay")
        return {
            "direct_stock_rns_received": direct_handler.received,
            "prns_page_seed": page_seed,
            "prns_announcement": announce,
            "prns_observed_stock_rns": peer_path,
            "captures": relay.close(),
        }
    finally:
        if relay is not None:
            try:
                relay.close()
            except GateError:
                pass
        if prns is not None:
            prns.stop()


def case_prns_hops(case_dir: Path, prnsd: Path) -> dict[str, object]:
    """Cross-check O-10 on the wire through a pinned Prns transport node."""

    shared_port, control_port, prns_listen_port = free_port(), free_port(), free_port()
    config_dir = case_dir / "prns-config"
    config_dir.mkdir()
    (config_dir / "config").write_text(
        prns_config(
            shared_port=shared_port,
            control_port=control_port,
            target_port=None,
            listen_port=prns_listen_port,
            transport=True,
        ),
        encoding="utf-8",
    )
    prns: LineProcess | None = None
    relay_a: RecordingRelay | None = None
    relay_b: RecordingRelay | None = None
    logger_a: LineProcess | None = None
    logger_b: LineProcess | None = None
    try:
        prns = start_prnsd(prnsd, config_dir, case_dir, shared_port)
        relay_a = RecordingRelay(prns_listen_port, case_dir, "a-to-prns", "prns-to-a")
        relay_b = RecordingRelay(prns_listen_port, case_dir, "b-to-prns", "prns-to-b")
        logger_a = start_announce_logger(case_dir, "a", relay_a.port)
        logger_b = start_announce_logger(case_dir, "b", relay_b.port)
        logger_a.wait_for(
            lambda: any(line.startswith("SELF a ") for line in logger_a.lines),
            30,
            "Retinue A logger",
        )
        logger_b.wait_for(
            lambda: any(line.startswith("SELF b ") for line in logger_b.lines),
            30,
            "Retinue B logger",
        )
        a_destination = next(line.split()[2] for line in logger_a.lines if line.startswith("SELF a "))
        b_destination = next(line.split()[2] for line in logger_b.lines if line.startswith("SELF b "))

        def forwarded(lines: list[str], destination: str) -> bool:
            return any(
                line.startswith("RECV_ANNOUNCE ") and f"dest={destination}" in line and "hops=1" in line
                for line in lines
            )

        logger_a.wait_for(
            lambda: forwarded(logger_a.lines, b_destination),
            20,
            "Prns-forwarded B announce with hops=1",
        )
        logger_b.wait_for(
            lambda: forwarded(logger_b.lines, a_destination),
            20,
            "Prns-forwarded A announce with hops=1",
        )
        return {
            "expectation": "Prns increments wire hops from source 0 to forwarded 1 before rebroadcast.",
            "a_received": [line for line in logger_a.lines if line.startswith("RECV_ANNOUNCE ")],
            "b_received": [line for line in logger_b.lines if line.startswith("RECV_ANNOUNCE ")],
            "captures": {"a": relay_a.close(), "b": relay_b.close()},
        }
    finally:
        for logger in (logger_a, logger_b):
            if logger is not None:
                logger.stop()
        for relay in (relay_a, relay_b):
            if relay is not None:
                try:
                    relay.close()
                except GateError:
                    pass
        if prns is not None:
            prns.stop()


def case_rns_hops(case_dir: Path, _prnsd: Path) -> dict[str, object]:
    """Capture stock RNS's matching local transport forward for O-10."""

    listen_port = free_port()
    config_dir = case_dir / "rns-config"
    config_dir.mkdir()
    (config_dir / "config").write_text(rns_transport_config(listen_port), encoding="utf-8")
    RNS.Reticulum(configdir=str(config_dir))
    relay_a: RecordingRelay | None = None
    relay_b: RecordingRelay | None = None
    logger_a: LineProcess | None = None
    logger_b: LineProcess | None = None
    try:
        relay_a = RecordingRelay(listen_port, case_dir, "a-to-rns", "rns-to-a")
        relay_b = RecordingRelay(listen_port, case_dir, "b-to-rns", "rns-to-b")
        logger_a = start_announce_logger(case_dir, "a", relay_a.port)
        logger_b = start_announce_logger(case_dir, "b", relay_b.port)
        logger_a.wait_for(
            lambda: any(line.startswith("SELF a ") for line in logger_a.lines),
            30,
            "Retinue A logger",
        )
        logger_b.wait_for(
            lambda: any(line.startswith("SELF b ") for line in logger_b.lines),
            30,
            "Retinue B logger",
        )
        a_destination = next(line.split()[2] for line in logger_a.lines if line.startswith("SELF a "))
        b_destination = next(line.split()[2] for line in logger_b.lines if line.startswith("SELF b "))

        def forwarded(lines: list[str], destination: str) -> bool:
            return any(
                line.startswith("RECV_ANNOUNCE ") and f"dest={destination}" in line and "hops=1" in line
                for line in lines
            )

        logger_a.wait_for(
            lambda: forwarded(logger_a.lines, b_destination),
            20,
            "stock-RNS-forwarded B announce with hops=1",
        )
        logger_b.wait_for(
            lambda: forwarded(logger_b.lines, a_destination),
            20,
            "stock-RNS-forwarded A announce with hops=1",
        )
        return {
            "expectation": "Stock RNS increments wire hops from source 0 to forwarded 1 before rebroadcast.",
            "a_received": [line for line in logger_a.lines if line.startswith("RECV_ANNOUNCE ")],
            "b_received": [line for line in logger_b.lines if line.startswith("RECV_ANNOUNCE ")],
            "captures": {"a": relay_a.close(), "b": relay_b.close()},
        }
    finally:
        for logger in (logger_a, logger_b):
            if logger is not None:
                logger.stop()
        for relay in (relay_a, relay_b):
            if relay is not None:
                try:
                    relay.close()
                except GateError:
                    pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--case", choices=("rns-retinue", "prns-retinue", "prns-rns", "prns-hops", "rns-hops")
    )
    parser.add_argument("--case-dir", type=Path, help="private directory for one isolated case")
    parser.add_argument(
        "--prns-root",
        type=Path,
        help="clean detached Prns worktree to use as the black-box peer",
    )
    parser.add_argument("--prnsd", type=Path, help="path to a prebuilt pinned prnsd executable")
    parser.add_argument(
        "--results-dir",
        type=Path,
        help="where to write raw captures and JSON records (default: validation/results/peer-<UTC timestamp>)",
    )
    arguments = parser.parse_args()

    global PRNS_ROOT
    if arguments.prns_root is not None:
        PRNS_ROOT = arguments.prns_root.resolve()

    prnsd_candidates = [
        arguments.prnsd,
        Path(os.environ["PRNSD_BIN"]) if "PRNSD_BIN" in os.environ else None,
        (
            Path(os.environ["CARGO_TARGET_DIR"])
            / "debug"
            / ("prnsd.exe" if os.name == "nt" else "prnsd")
            if "CARGO_TARGET_DIR" in os.environ
            else None
        ),
        PRNS_ROOT / "prnsd" / "target" / "debug" / ("prnsd.exe" if os.name == "nt" else "prnsd"),
        PRNS_ROOT / "target" / "debug" / ("prnsd.exe" if os.name == "nt" else "prnsd"),
    ]
    prnsd = next((path.resolve() for path in prnsd_candidates if path is not None and path.is_file()), None)
    if prnsd is None:
        print("peer matrix: no prnsd executable found", file=sys.stderr)
        print(
            "build it with: cargo build --manifest-path C:\\Users\\mark_\\Code\\repos\\Prns\\prnsd\\Cargo.toml "
            "-p prnsd --no-default-features --features tokio-host,observability",
            file=sys.stderr,
        )
        return 2

    prns_state = git_state(PRNS_ROOT)
    if prns_state["revision"] != PINNED_PRNS_REVISION:
        print(
            f"peer matrix: Prns is {prns_state['revision']}, expected {PINNED_PRNS_REVISION}",
            file=sys.stderr,
        )
        return 2
    if prns_state["status"]:
        print("peer matrix: refusing a dirty Prns checkout", file=sys.stderr)
        return 2

    runners: dict[str, Callable[[Path, Path], dict[str, object]]] = {
        "rns-retinue": case_rns_retinue,
        "prns-retinue": case_prns_retinue,
        "prns-rns": case_prns_rns,
        "prns-hops": case_prns_hops,
        "rns-hops": case_rns_hops,
    }
    if arguments.case is not None:
        if arguments.case_dir is None:
            parser.error("--case requires --case-dir")
        case_dir = arguments.case_dir.resolve()
        case_dir.mkdir(parents=True, exist_ok=True)
        started = datetime.now(UTC).isoformat()
        try:
            details = runners[arguments.case](case_dir, prnsd)
            outcome: dict[str, object] = {"status": "pass", "started_at_utc": started, "details": details}
            exit_code = 0
        except Exception as error:
            outcome = {"status": "fail", "started_at_utc": started, "error": repr(error)}
            exit_code = 1
        write_case_result(case_dir, outcome)
        print(f"{arguments.case}: {outcome['status']}", flush=True)
        if RNS.Reticulum.get_instance() is not None:
            RNS.exit(exit_code)
        return exit_code

    timestamp = datetime.now(UTC).strftime("%Y%m%dT%H%M%SZ")
    results_dir = arguments.results_dir or RETINUE_REPO / "validation" / "results" / f"peer-{timestamp}"
    results_dir = results_dir.resolve()
    if results_dir.exists():
        print(f"peer matrix: result directory already exists: {results_dir}", file=sys.stderr)
        return 2
    results_dir.mkdir(parents=True)

    record: dict[str, object] = {
        "schema": "retinue-peer-matrix-v1",
        "started_at_utc": datetime.now(UTC).isoformat(),
        "pinned_prns_revision": PINNED_PRNS_REVISION,
        "prns": {
            **prns_state,
            "root": str(PRNS_ROOT),
            "daemon": str(prnsd),
            "daemon_sha256": sha256_file(prnsd),
            "daemon_version": run_checked([str(prnsd), "--version"], PRNS_ROOT),
        },
        "retinue": {
            **git_state(RETINUE_REPO),
            "interop_example_sha256": sha256_file(RETINUE_CRATE / "examples" / "interop_tcp.rs"),
        },
        "stock_rns": {"version": RNS.__version__},
        "driver": {"path": str(Path(__file__).resolve()), "sha256": sha256_file(Path(__file__))},
        "cases": {},
    }
    write_json(results_dir / "matrix.json", record)

    cases = ["rns-retinue", "prns-retinue", "prns-rns", "rns-hops", "prns-hops"]
    exit_code = 0
    for name in cases:
        case_dir = results_dir / name
        case_dir.mkdir()
        command = [
            sys.executable,
            "-u",
            str(Path(__file__).resolve()),
            "--case",
            name,
            "--case-dir",
            str(case_dir),
            "--prns-root",
            str(PRNS_ROOT),
            "--prnsd",
            str(prnsd),
        ]
        with (case_dir / "driver.log").open("w", encoding="utf-8") as log:
            completed = subprocess.run(command, cwd=HERE, stdout=log, stderr=subprocess.STDOUT)
        result_path = case_dir / "result.json"
        if result_path.exists():
            outcome = json.loads(result_path.read_text(encoding="utf-8"))
        else:
            outcome = {
                "status": "fail",
                "error": f"case process exited {completed.returncode} without result.json",
            }
            write_case_result(case_dir, outcome)
        if completed.returncode or outcome["status"] != "pass":
            exit_code = 1
        record["cases"][name] = outcome
        write_json(results_dir / "matrix.json", record)
        print(f"{name}: {outcome['status']}")
        if exit_code:
            break

    record["finished_at_utc"] = datetime.now(UTC).isoformat()
    record["status"] = "pass" if exit_code == 0 else "fail"
    write_json(results_dir / "matrix.json", record)
    print(f"PEER MATRIX: {record['status'].upper()} ({results_dir})")
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
