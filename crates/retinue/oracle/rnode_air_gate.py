"""Harness for `radio-hand`'s `rnode_air`: fake RNodes on ptys, and their byte log.

`Air(n, flags)` opens n ptys, starts the air on their masters and hands back the slave
paths a host opens as RNode ports. `frames()` reads the log as (time, device, direction,
frame) with the frame unescaped, command byte first.
"""

from __future__ import annotations

import json
import subprocess
import tempfile
import threading
import time
from pathlib import Path

from pty_bridge import REPO, open_pty

WORKSPACE = REPO.parent.parent
DATA, RADIO_STATE, DETECT, LEAVE = 0x00, 0x06, 0x08, 0x0A
ST_ALOCK, LT_ALOCK, READY, RESET, ERROR = 0x0B, 0x0C, 0x0F, 0x55, 0x90


class Air:
    def __init__(self, devices: int = 2, flags: tuple[str, ...] = ()):
        pairs = [open_pty() for _ in range(devices)]
        masters = [master for master, _ in pairs]
        self.ports = [port for _, port in pairs]
        self.log = Path(tempfile.mkstemp(prefix="rnode-air-", suffix=".jsonl")[1])
        fds = [arg for master in masters for arg in ("--fd", str(master))]
        self.proc = subprocess.Popen(
            ["cargo", "run", "--quiet", "-p", "radio-hand", "--example", "rnode_air", "--",
             *fds, "--log", str(self.log), *flags],
            cwd=WORKSPACE, pass_fds=masters, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        self.ready = threading.Event()
        threading.Thread(target=self._pump, daemon=True).start()
        if not self.ready.wait(300):
            raise RuntimeError("rnode_air did not start")

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            print(f"  [air] {line.rstrip()}", flush=True)
            if line.startswith("AIR_READY"):
                self.ready.set()

    def frames(self) -> list[tuple[float, int, str, bytes]]:
        out = []
        for line in self.log.read_text().splitlines():
            entry = json.loads(line)
            out.append((entry["t"], entry["dev"], entry["dir"], bytes.fromhex(entry["hex"])))
        return out

    def host_frames(self, dev: int, command: int | None = None) -> list[tuple[float, bytes]]:
        return [(t, f) for t, d, direction, f in self.frames()
                if d == dev and direction == "h2d" and (command is None or f[0] == command)]

    def device_frames(self, dev: int, command: int) -> list[tuple[float, bytes]]:
        return [(t, f) for t, d, direction, f in self.frames()
                if d == dev and direction == "d2h" and f[0] == command]

    def close(self) -> None:
        self.proc.kill()


def stock_rnode(port: str, sf: int = 7, flow_control: bool = False, alock: bool = False,
                beacon: tuple[str, int] | None = None, ifac_bits: int | None = None) -> str:
    """A stock RNodeInterface section matching `rnode_peer`'s defaults."""
    lines = ["  [[rnode]]", "    type = RNodeInterface", "    enabled = yes",
             f"    port = {port}", "    frequency = 867200000", "    bandwidth = 500000",
             "    txpower = 14", f"    spreadingfactor = {sf}", "    codingrate = 5",
             f"    flow_control = {'yes' if flow_control else 'no'}"]
    if alock:
        lines += ["    airtime_limit_short = 33.5", "    airtime_limit_long = 10"]
    if beacon:
        lines += [f"    id_callsign = {beacon[0]}", f"    id_interval = {beacon[1]}"]
    if ifac_bits:
        lines += ["    network_name = retinue-serial-gate", "    passphrase = serial-and-radio",
                  f"    ifac_size = {ifac_bits}"]
    return "\n".join(lines) + "\n"


def wait_until(predicate, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.2)
    return bool(predicate())
