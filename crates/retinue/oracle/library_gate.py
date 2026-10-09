"""Shared plumbing for the library-path live gates (examples/library_oracle/).

Each gate runs stock RNS in a child process (RNS owns process-global state and may
hard-exit during teardown); the parent owns the disposable RNS config directory and
removes it after the child exits. The Retinue side is the prebuilt `library_oracle`
example, launched directly so compiler and cache waits stay out of network timeouts.
Build it first with `cargo build -p retinue --examples --all-features --locked`.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
CONFIG_ENV = "RETINUE_LIBRARY_ORACLE_CONFIG_DIR"
EXPECTED_RNS = "1.5.7"

# RNS.Resource.status values, for readable verdicts.
RESOURCE_STATUS = {
    0x00: "NONE", 0x01: "QUEUED", 0x02: "ADVERTISED", 0x03: "TRANSFERRING",
    0x04: "AWAITING_PROOF", 0x05: "ASSEMBLING", 0x06: "COMPLETE", 0x07: "FAILED",
    0x08: "CORRUPT", 0x09: "REJECTED",
}


def payload(length: int, seed: int) -> bytes:
    """The xorshift32 stream examples/library_oracle/main.rs generates: one low byte per step."""
    x = seed or 1
    out = bytearray(length)
    for i in range(length):
        x ^= (x << 13) & 0xFFFFFFFF
        x ^= x >> 17
        x ^= (x << 5) & 0xFFFFFFFF
        out[i] = x & 0xFF
    return bytes(out)


class Retinue:
    """The running library_oracle example and its captured output."""

    def __init__(self, mode: str, length: int, seed: int) -> None:
        default_target = r"C:\t\cargo-targets\retinue" if os.name == "nt" else str(REPO.parent.parent / "target")
        target = Path(os.environ.get("CARGO_TARGET_DIR", default_target))
        name = "library_oracle.exe" if os.name == "nt" else "library_oracle"
        binary = target / "debug" / "examples" / name
        if not binary.is_file():
            raise FileNotFoundError(f"build the library_oracle example first: {binary}")
        print(f"Retinue binary: {binary} {mode} {length} {seed}", flush=True)
        self.proc = subprocess.Popen(
            [str(binary), mode, str(length), str(seed)],
            cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        self.lines: list[str] = []
        self.lock = threading.Lock()
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            line = raw.rstrip()
            with self.lock:
                self.lines.append(line)
            print(f"  [retinue] {line}", flush=True)

    def snapshot(self) -> list[str]:
        with self.lock:
            return list(self.lines)

    def find(self, pattern: str) -> re.Match[str] | None:
        for line in self.snapshot():
            match = re.fullmatch(pattern, line)
            if match:
                return match
        return None

    def wait_for(self, pattern: str, timeout: float) -> re.Match[str] | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            match = self.find(pattern)
            if match or self.proc.poll() is not None:
                return match or self.find(pattern)
            time.sleep(0.05)
        return None

    def wait_port(self) -> int:
        match = self.wait_for(r"LISTENING (\d+)", 60)
        if match is None:
            raise RuntimeError(f"Retinue did not listen (exit {self.proc.poll()})")
        return int(match.group(1))

    def wait_exit(self, timeout: float) -> int | None:
        try:
            code = self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            return None
        time.sleep(0.1)  # let the pump drain the last lines
        return code

    def tap(self, direction: str, packet_type: str, context: int) -> int:
        match = self.find(rf"TAP {direction} {packet_type} ctx=0x{context:02x} (\d+)")
        return int(match.group(1)) if match else 0

    def kill(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            pass


def start_rns(port: int):
    """Point a fresh stock RNS instance at Retinue's relay port."""
    import RNS

    if RNS.__version__ != EXPECTED_RNS:
        raise RuntimeError(f"expected stock RNS {EXPECTED_RNS}, got {RNS.__version__}")
    print(f"RNS {RNS.__version__}", flush=True)
    config_dir = Path(os.environ[CONFIG_ENV])
    (config_dir / "storage" / "resources").mkdir(parents=True, exist_ok=True)
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=3\n\n"
        "[interfaces]\n  [[retinue]]\n    type=TCPClientInterface\n"
        "    enabled=yes\n    target_host=127.0.0.1\n"
        f"    target_port={port}\n",
        encoding="utf-8",
    )
    return RNS.Reticulum(configdir=str(config_dir))


def verdict(label: str, ok: bool, detail: str = "") -> bool:
    suffix = f" ({detail})" if detail else ""
    print(f"{label}: {'PASS' if ok else 'FAIL'}{suffix}", flush=True)
    return ok


def supervise(script: str, title: str, timeout: float) -> int:
    """Run `script --peer` with a private RNS config directory and a hard deadline."""
    config_dir = Path(tempfile.mkdtemp(prefix="retinue-library-oracle-"))
    env = os.environ.copy()
    env[CONFIG_ENV] = str(config_dir)
    peer = subprocess.Popen([sys.executable, "-u", script, "--peer"], env=env)
    try:
        return peer.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        if os.name == "nt":
            subprocess.run(["taskkill", "/PID", str(peer.pid), "/T", "/F"], check=False)
        else:
            peer.kill()
        peer.wait(timeout=5)
        print(f"{title}: FAIL (peer timed out)", flush=True)
        return 1
    finally:
        shutil.rmtree(config_dir, ignore_errors=True)
