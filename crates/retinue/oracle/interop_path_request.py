"""Path-request gate: stock RNS asks Retinue for a path to Retinue's own destination.

RNS connects to Retinue twice, as interfaces A and B, and has never heard the destination
announced. It calls `RNS.Transport.request_path`, which sends one request, with one tag, on
both interfaces. Retinue must answer once: RNS drops a repeated destination and tag, and
answers a local destination on the requesting interface only (`Transport.py` 1838-1856,
3452-3456). RNS must then hold the path. A second request with a new tag, on B alone, must
be answered on B alone.

Both Retinue cores run in turn: the `Endpoint`, and a `Node` behind a small TCP shell
(examples/path_request_oracle.rs). Before this gate, the Endpoint answered every request on
every interface (four responses to the first ask) and the Node never answered.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_path_request.py   (CARGO_TARGET_DIR as for the build)
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

from library_gate import CONFIG_ENV, EXPECTED_RNS, REPO, verdict

TITLE = "PATH REQUEST INTEROP"
RUN_SECONDS = 40


class Oracle:
    """The running path_request_oracle example and its captured output."""

    def __init__(self, mode: str) -> None:
        default_target = str(REPO.parent.parent / "target")
        target = Path(os.environ.get("CARGO_TARGET_DIR", default_target))
        name = "path_request_oracle.exe" if os.name == "nt" else "path_request_oracle"
        binary = target / "debug" / "examples" / name
        if not binary.is_file():
            raise FileNotFoundError(f"build the path_request_oracle example first: {binary}")
        self.proc = subprocess.Popen(
            [str(binary), mode, str(RUN_SECONDS)],
            cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        self.lines: list[str] = []
        self.lock = threading.Lock()
        threading.Thread(target=self._pump, args=(mode,), daemon=True).start()

    def _pump(self, mode: str) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            line = raw.rstrip()
            with self.lock:
                self.lines.append(line)
            print(f"  [retinue {mode}] {line}", flush=True)

    def wait_for(self, pattern: str, timeout: float) -> re.Match[str] | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.lock:
                for line in self.lines:
                    match = re.fullmatch(pattern, line)
                    if match:
                        return match
            time.sleep(0.05)
        return None

    def responses(self) -> list[str]:
        with self.lock:
            return [line.split()[1] for line in self.lines if line.startswith("PATH_RESPONSE ")]

    def kill(self) -> None:
        if self.proc.poll() is None:
            self.proc.kill()
        try:
            self.proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            pass


def start_rns(port_a: int, port_b: int):
    import RNS

    if RNS.__version__ != EXPECTED_RNS:
        raise RuntimeError(f"expected stock RNS {EXPECTED_RNS}, got {RNS.__version__}")
    print(f"RNS {RNS.__version__}", flush=True)
    config_dir = Path(os.environ[CONFIG_ENV])
    interfaces = "".join(
        f"  [[{name}]]\n    type=TCPClientInterface\n    enabled=yes\n"
        f"    target_host=127.0.0.1\n    target_port={port}\n"
        for name, port in (("A", port_a), ("B", port_b))
    )
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=3\n\n"
        f"[interfaces]\n{interfaces}",
        encoding="utf-8",
    )
    return RNS.Reticulum(configdir=str(config_dir))


def rns_interface(name: str):
    import RNS

    for interface in RNS.Transport.interfaces:
        if getattr(interface, "name", None) == name:
            return interface
    return None


def settle(oracle: Oracle, count: int, timeout: float) -> list[str]:
    """Wait until `count` responses have been seen, then a little longer for strays."""
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline and len(oracle.responses()) < count:
        time.sleep(0.05)
    time.sleep(2.0)
    return oracle.responses()


def peer(mode: str) -> int:
    oracle = Oracle(mode)
    exit_code = 1
    try:
        listening = oracle.wait_for(r"LISTENING (\d+) (\d+)", 60)
        dest_line = oracle.wait_for(r"DEST ([0-9a-f]{32})", 5)
        if listening is None or dest_line is None:
            print(f"{TITLE} ({mode}): FAIL (Retinue did not start)", flush=True)
            return 1
        import RNS

        start_rns(int(listening.group(1)), int(listening.group(2)))
        destination = bytes.fromhex(dest_line.group(1))
        connected = oracle.wait_for(r"CONNECTED B", 20) is not None
        deadline = time.monotonic() + 20
        while time.monotonic() < deadline and not all(
            getattr(rns_interface(name), "online", False) for name in ("A", "B")
        ):
            time.sleep(0.1)
        time.sleep(1.0)
        had_path = RNS.Transport.has_path(destination)

        print("RNS: request_path on every interface", flush=True)
        RNS.Transport.request_path(destination)
        first = settle(oracle, 1, 10)
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline and not RNS.Transport.has_path(destination):
            time.sleep(0.1)
        learned = RNS.Transport.has_path(destination)

        print("RNS: request_path on interface B only", flush=True)
        RNS.Transport.request_path(destination, on_interface=rns_interface("B"))
        second = settle(oracle, len(first) + 1, 10)[len(first):]

        print("\n" + "=" * 72)
        ok = verdict(f"[{mode}] both RNS interfaces connected", connected)
        ok &= verdict(f"[{mode}] RNS had no path before asking", not had_path)
        ok &= verdict(f"[{mode}] one tag on two interfaces is answered once",
                      len(first) == 1, f"responses {first}")
        ok &= verdict(f"[{mode}] RNS learned the path from the response", learned)
        ok &= verdict(f"[{mode}] a request on B is answered on B only",
                      second == ["B"], f"responses {second}")
        print("=" * 72)
        print(f"{TITLE} ({mode}): {'PASS' if ok else 'FAIL'}", flush=True)
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        oracle.kill()
        import RNS

        RNS.exit(exit_code)


def supervise(mode: str, timeout: float) -> int:
    """Run one mode's RNS peer in its own process, with a private config directory."""
    config_dir = Path(tempfile.mkdtemp(prefix="retinue-path-request-"))
    env = os.environ.copy()
    env[CONFIG_ENV] = str(config_dir)
    child = subprocess.Popen([sys.executable, "-u", __file__, "--peer", mode], env=env)
    try:
        return child.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        child.kill()
        child.wait(timeout=5)
        print(f"{TITLE} ({mode}): FAIL (peer timed out)", flush=True)
        return 1
    finally:
        shutil.rmtree(config_dir, ignore_errors=True)


def main() -> int:
    failed = [mode for mode in ("endpoint", "node") if supervise(mode, 120) != 0]
    print(f"\n{TITLE}: {'FAIL (' + ', '.join(failed) + ')' if failed else 'PASS'}", flush=True)
    return 1 if failed else 0


if __name__ == "__main__":
    if sys.argv[1:2] == ["--peer"]:
        raise SystemExit(peer(sys.argv[2]))
    raise SystemExit(main())
