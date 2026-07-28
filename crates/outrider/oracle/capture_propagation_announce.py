"""Capture a pinned stock lxmd propagation-node announce through Retinue."""

from __future__ import annotations

import atexit
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LXMD = Path(sys.executable).parent / "lxmd.exe"


def main() -> int:
    version = subprocess.run(
        [str(LXMD), "--version"], check=True, capture_output=True, text=True
    ).stdout.strip()
    print(version)

    receiver = subprocess.Popen(
        [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "outrider",
            "--example",
            "stock_propagation_announce",
        ],
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert receiver.stdout is not None
        for raw in receiver.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [outrider] {line}")

    threading.Thread(target=pump, daemon=True).start()
    port = None
    deadline = time.time() + 180
    while time.time() < deadline and port is None:
        for line in list(lines):
            match = re.fullmatch(r"LISTENING (\d+)", line)
            if match:
                port = int(match.group(1))
        if receiver.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None:
        receiver.kill()
        return 1

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-announce-"))
    lxmd_config = root / "lxmd"
    rns_config = root / "rns"
    lxmd_config.mkdir()
    rns_config.mkdir()
    (lxmd_config / "config").write_text(
        "[propagation]\n"
        "enable_node=yes\n"
        "node_name=Stock Propagation Oracle\n"
        "announce_at_start=yes\n"
        "autopeer=no\n"
        "propagation_stamp_cost_target=8\n"
        "peering_cost=8\n"
        "\n[lxmf]\n"
        "display_name=Stock Delivery Oracle\n"
        "announce_at_start=no\n"
        "\n[logging]\n"
        "loglevel=5\n",
        encoding="utf-8",
    )
    (rns_config / "config").write_text(
        "[reticulum]\n"
        "enable_transport=No\n"
        "share_instance=No\n"
        "panic_on_interface_error=No\n"
        "\n[logging]\n"
        "loglevel=5\n"
        "\n[interfaces]\n"
        "[[outrider]]\n"
        "type=TCPClientInterface\n"
        "enabled=yes\n"
        "target_host=127.0.0.1\n"
        f"target_port={port}\n",
        encoding="utf-8",
    )

    daemon = subprocess.Popen(
        [
            str(LXMD),
            "-p",
            "--config",
            str(lxmd_config),
            "--rnsconfig",
            str(rns_config),
            "--verbose",
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    try:
        result_deadline = time.time() + 60
        while time.time() < result_deadline:
            if receiver.poll() is not None:
                break
            if any(line.startswith("PROPAGATION_APP_DATA ") for line in lines):
                break
            time.sleep(0.1)
        try:
            receiver.wait(timeout=10)
        except subprocess.TimeoutExpired:
            receiver.kill()
        captured = any(line.startswith("PROPAGATION_APP_DATA ") for line in lines)
        print(f"stock propagation announce captured: {'PASS' if captured else 'FAIL'}")
        print(f"PROPAGATION_ANNOUNCE: {'PASS' if captured else 'FAIL'}")
        return 0 if captured else 1
    finally:
        daemon.terminate()
        try:
            daemon.wait(timeout=5)
        except subprocess.TimeoutExpired:
            daemon.kill()
        atexit.register(shutil.rmtree, root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
