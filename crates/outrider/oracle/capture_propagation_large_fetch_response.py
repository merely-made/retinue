"""Prove Outrider fetches a large response from stock LXMF.

Outrider submits the fixed message through its already-proven Resource lane.
Pinned stock lxmd stores it and emits the Resource-backed fetch response.
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

import LXMF
import RNS


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
LXMD = Path(sys.executable).parent / "lxmd.exe"


def pump(process: subprocess.Popen[str], prefix: str, lines: list[str]) -> None:
    assert process.stdout is not None
    for raw in process.stdout:
        line = raw.rstrip()
        lines.append(line)
        print(f"  [{prefix}] {line}", flush=True)


def wait_for(lines: list[str], prefix: str, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if any(line.startswith(prefix) for line in lines):
            return True
        time.sleep(0.1)
    return False


def stop(process: subprocess.Popen[str] | None) -> None:
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}", flush=True)
    sender_executable = os.environ["OUTRIDER_PROPAGATION_SEND"]
    fetch_executable = os.environ["OUTRIDER_PROPAGATION_FETCH_RECEIVE"]
    sender = subprocess.Popen(
        [sender_executable],
        cwd=REPO,
        env={**os.environ, "OUTRIDER_LARGE": "1", "OUTRIDER_SUMMARY": "1"},
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    sender_lines: list[str] = []
    threading.Thread(
        target=pump, args=(sender, "sender", sender_lines), daemon=True
    ).start()

    fetcher = None
    daemon = None
    root = Path(tempfile.mkdtemp(prefix="outrider-large-fetch-capture-"))
    try:
        if not wait_for(sender_lines, "LISTENING ", 60):
            return 1
        port_line = next(line for line in sender_lines if line.startswith("LISTENING "))
        port = int(re.fullmatch(r"LISTENING (\d+)", port_line).group(1))

        node_rns = root / "node-rns"
        node_config = root / "node"
        node_rns.mkdir()
        node_config.mkdir()
        (node_rns / "config").write_text(
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
        (node_config / "config").write_text(
            "[propagation]\n"
            "enable_node=yes\n"
            "node_name=Stock Large Response Oracle\n"
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
        daemon = subprocess.Popen(
            [
                str(LXMD),
                "-p",
                "--config",
                str(node_config),
                "--rnsconfig",
                str(node_rns),
                "--verbose",
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        daemon_lines: list[str] = []
        threading.Thread(
            target=pump, args=(daemon, "lxmd", daemon_lines), daemon=True
        ).start()

        fetcher = subprocess.Popen(
            [fetch_executable],
            cwd=REPO,
            env={
                **os.environ,
                "RETINUE_ADDR": f"127.0.0.1:{port}",
                "OUTRIDER_SUMMARY": "1",
            },
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            bufsize=1,
        )
        fetch_lines: list[str] = []
        threading.Thread(
            target=pump, args=(fetcher, "fetcher", fetch_lines), daemon=True
        ).start()

        if not wait_for(fetch_lines, "FETCH_READY ", 60):
            return 1
        if not wait_for(sender_lines, "SUBMITTED ", 180):
            return 1
        time.sleep(5)
        assert fetcher.stdin is not None
        fetcher.stdin.write("fetch\n")
        fetcher.stdin.flush()

        deadline = time.time() + 90
        while time.time() < deadline:
            if "PRODUCTION_FETCH true" in fetch_lines:
                print("OUTRIDER_LARGE_FETCH_FROM_STOCK: PASS", flush=True)
                return 0
            if fetcher.poll() is not None:
                break
            time.sleep(0.1)
        print("OUTRIDER_LARGE_FETCH_FROM_STOCK: FAIL", flush=True)
        return 1
    finally:
        stop(fetcher)
        stop(daemon)
        stop(sender)
        shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
