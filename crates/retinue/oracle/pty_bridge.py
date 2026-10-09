"""Pty plumbing shared by the serial gates: virtual serial lines with no hardware.

`open_pty()` gives a raw pty whose slave stays held open here, so a carrier closing and
reopening its side never makes the master see a hangup. `Bridge` copies bytes between two
masters, a null-modem cable. `FakeTnc` plays a KISS TNC on one master: it records every
frame and answers each DATA frame with READY after a delay.
"""

from __future__ import annotations

import os
import re
import select
import subprocess
import threading
import time
import tty
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
FEND, FESC, TFEND, TFESC = 0xC0, 0xDB, 0xDC, 0xDD

_held: list[int] = []


def open_pty() -> tuple[int, str]:
    """A raw pty: (master fd, slave path). The slave fd is held for the process's life."""
    master, slave = os.openpty()
    tty.setraw(slave)
    tty.setraw(master)
    _held.append(slave)
    return master, os.ttyname(slave)


class Bridge(threading.Thread):
    """Copy bytes both ways between two pty masters, recording each chunk."""

    def __init__(self, a: int, b: int):
        super().__init__(daemon=True)
        self.ends = {a: b, b: a}
        self.names = {a: "a2b", b: "b2a"}
        self.log: list[tuple[float, str, bytes]] = []
        self.start()

    def run(self) -> None:
        while True:
            ready, _, _ = select.select(list(self.ends), [], [])
            for fd in ready:
                try:
                    data = os.read(fd, 4096)
                except OSError:
                    continue
                self.log.append((time.time(), self.names[fd], data))
                os.write(self.ends[fd], data)


def kiss_frames(data: bytes) -> list[bytes]:
    """Unescaped KISS frames (command byte first) in a byte stream."""
    frames, buf, esc, inside = [], bytearray(), False, False
    for byte in data:
        if byte == FEND:
            if inside and buf:
                frames.append(bytes(buf))
            buf, esc, inside = bytearray(), False, True
        elif inside and esc:
            buf.append({TFEND: FEND, TFESC: FESC}.get(byte, byte))
            esc = False
        elif inside and byte == FESC:
            esc = True
        elif inside:
            buf.append(byte)
    return frames


class FakeTnc(threading.Thread):
    """A KISS TNC on a pty master: records host frames, answers DATA with READY."""

    def __init__(self, master: int, ready_delay: float = 0.3):
        super().__init__(daemon=True)
        self.master = master
        self.ready_delay = ready_delay
        self.raw = bytearray()
        # (time, "host" | "ready", frame) in order.
        self.events: list[tuple[float, str, bytes]] = []
        self.lock = threading.Lock()
        self.start()

    def run(self) -> None:
        pending = bytearray()
        while True:
            select.select([self.master], [], [])
            try:
                data = os.read(self.master, 4096)
            except OSError:
                continue
            with self.lock:
                self.raw.extend(data)
            pending.extend(data)
            last = pending.rfind(bytes([FEND]))
            if last <= 0:
                continue
            complete, pending = bytes(pending[: last + 1]), bytearray(pending[last:])
            for frame in kiss_frames(complete):
                with self.lock:
                    self.events.append((time.time(), "host", frame))
                if frame[0] & 0x0F == 0x00:
                    threading.Timer(self.ready_delay, self.ready).start()

    def ready(self) -> None:
        with self.lock:
            self.events.append((time.time(), "ready", b"\x0f\x01"))
        os.write(self.master, bytes([FEND, 0x0F, 0x01, FEND]))

    def snapshot(self) -> tuple[bytes, list[tuple[float, str, bytes]]]:
        with self.lock:
            return bytes(self.raw), list(self.events)


class Retinue:
    """A Retinue example as a child process, its stdout collected line by line."""

    def __init__(self, example: str, features: str, args: list[str], pass_fds=()):
        cmd = ["cargo", "run", "--quiet", "--features", features, "--example", example, "--", *args]
        self.proc = subprocess.Popen(
            cmd, cwd=REPO, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1, pass_fds=pass_fds,
        )
        self.lines: list[str] = []
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            line = line.rstrip()
            self.lines.append(line)
            print(f"  [retinue] {line}", flush=True)

    def wait_for(self, pattern: str, timeout: float) -> re.Match | None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            for line in list(self.lines):
                match = re.search(pattern, line)
                if match:
                    return match
            if self.proc.poll() is not None:
                return None
            time.sleep(0.1)
        return None

    def close(self, timeout: float = 20) -> None:
        """Close stdin, the example's cue to shut down, and wait for it."""
        try:
            assert self.proc.stdin is not None
            self.proc.stdin.close()
            self.proc.wait(timeout=timeout)
        except (subprocess.TimeoutExpired, BrokenPipeError):
            self.proc.kill()


def rns_config(directory: Path, interface: str, transport: bool = False) -> None:
    """Write a stock RNS config with one interface section (indented `key = value` lines)."""
    (directory / "config").write_text(
        "[reticulum]\n"
        f"  enable_transport = {'Yes' if transport else 'No'}\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "\n[logging]\n  loglevel = 4\n"
        "\n[interfaces]\n" + interface,
        encoding="utf-8",
    )


def payload(length: int, seed: int) -> bytes:
    """The xorshift32 stream the Retinue script checks against."""
    x, out = max(seed, 1), bytearray()
    for _ in range(length):
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        out.append(x & 0xFF)
    return bytes(out)


SINK_SEED = bytes([0x64]) * 64
REQUEST_SEED, RESOURCE_SEED, PUBLISH_SEED = 7, 0x5E210001, 0x5E210002


def _wait(predicate, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return bool(predicate())


def exercise_stock(retinue: Retinue, resource_len: int = 3000, publish_len: int | None = None,
                   request_len: int = 64, timeout: float = 120) -> dict[str, bool]:
    """Drive the shared script from the stock side and judge by stock's own state.

    Path, link, request and a Resource from stock to Retinue; with `publish_len`, also a
    Resource from Retinue into a stock sink. A `request_len` of 400 packs to the 431-byte
    link MDU, so request and response each fill one 499-byte packet.
    """
    import RNS

    results: dict[str, bool] = {}
    received: dict = {}
    if publish_len is not None:
        sink = RNS.Destination(RNS.Identity.from_bytes(SINK_SEED), RNS.Destination.IN,
                               RNS.Destination.SINGLE, "retinue", "serial-sink")

        def concluded(resource):
            received["status"] = resource.status
            received["data"] = resource.data.read() if resource.status == RNS.Resource.COMPLETE else b""

        def established(link):
            link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
            link.set_resource_concluded_callback(concluded)

        sink.set_link_established_callback(established)

        def announce_sink():
            while "status" not in received:
                sink.announce()
                time.sleep(5)

        threading.Thread(target=announce_sink, daemon=True).start()

    match = retinue.wait_for(r"DEST ([0-9a-f]{32})", 180)
    if match is None:
        return {"retinue printed its destination": False}
    dest = bytes.fromhex(match.group(1))
    results["stock has_path(retinue)"] = _wait(lambda: RNS.Transport.has_path(dest), timeout)
    identity = RNS.Identity.recall(dest)
    if identity is None:
        return results
    out = RNS.Destination(identity, RNS.Destination.OUT, RNS.Destination.SINGLE,
                          "retinue", "serial-peer")
    link = RNS.Link(out)
    results["stock link ACTIVE"] = _wait(lambda: link.status == RNS.Link.ACTIVE, 60)
    if not results["stock link ACTIVE"]:
        return results

    question = payload(request_len, REQUEST_SEED)
    receipt = link.request("/echo", question, timeout=60)
    done = (RNS.RequestReceipt.READY, RNS.RequestReceipt.FAILED)
    _wait(lambda: receipt.get_status() in done, 60)
    results["stock request answered"] = receipt.get_response() == question

    resource = RNS.Resource(payload(resource_len, RESOURCE_SEED), link)
    _wait(lambda: resource.status in (RNS.Resource.COMPLETE, RNS.Resource.FAILED), timeout)
    results[f"stock Resource COMPLETE ({resource.total_parts} parts)"] = (
        resource.status == RNS.Resource.COMPLETE and resource.total_parts > 1)

    if publish_len is not None:
        _wait(lambda: "status" in received, timeout)
        results["stock sink Resource COMPLETE"] = (
            received.get("status") == RNS.Resource.COMPLETE
            and received.get("data") == payload(publish_len, PUBLISH_SEED))
    link.teardown()
    time.sleep(1)
    return results


def report(title: str, results: dict[str, bool]) -> int:
    print("\n" + "=" * 68)
    for name, ok in results.items():
        print(f"{name:<52} {'PASS' if ok else 'FAIL'}")
    ok = bool(results) and all(results.values())
    print(f"{title}: {'PASS' if ok else 'FAIL'}", flush=True)
    return 0 if ok else 1
