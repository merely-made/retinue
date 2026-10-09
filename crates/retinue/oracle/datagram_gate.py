"""Stock-side driver shared by the datagram gates (UDP and Auto), judged on stock's state.

Retinue runs `examples/datagram/script.rs` (through `udp_interop` or `auto_peer`): it
announces `retinue.datagram-peer`, takes a Resource, proves a link packet and publishes a
Resource back. `exercise` drives the stock half and returns named verdicts.
"""

from __future__ import annotations

import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

from pty_bridge import Retinue, payload, report

RESOURCE_LEN, PACKET_LEN, PUBLISH_LEN = 64 * 1024, 300, 4096
RESOURCE_SEED, PACKET_SEED, PUBLISH_SEED = 0xDA7A0001, 0xDA7A0002, 0xDA7A0003
NETWORK_NAME, PASSPHRASE = "retinue-datagram-gate", "udp-and-auto"
CONFIG_ENV = "RETINUE_DATAGRAM_GATE_CONFIG_DIR"
REPO = Path(__file__).resolve().parent.parent


def config_dir() -> Path:
    """The child's stock config directory, owned and removed by the parent."""
    return Path(os.environ[CONFIG_ENV])


def supervise(script: str, title: str, example: str, runs: list[list[str]]) -> int:
    """Build `example`, then run `script --child ARGS` once per entry in a fresh process,
    since stock RNS owns process-global state and hard-exits on teardown."""
    subprocess.run(["cargo", "build", "--quiet", "-p", "retinue", "--features", "auto",
                    "--example", example], cwd=REPO, check=True)
    failed = []
    for args in runs:
        directory = tempfile.mkdtemp(prefix="retinue-datagram-")
        env = dict(os.environ, **{CONFIG_ENV: directory})
        try:
            code = subprocess.run([sys.executable, "-u", script, "--child", *args], env=env,
                                  timeout=600).returncode
        except subprocess.TimeoutExpired:
            code = None
        finally:
            shutil.rmtree(directory, ignore_errors=True)
        if code != 0:
            failed.append(" ".join(args))
    print(f"{title}: {'PASS' if not failed else 'FAIL (' + ', '.join(failed) + ')'}", flush=True)
    return 0 if not failed else 1


def conclude(title: str, run) -> int:
    """Report `run()`'s verdicts and hard-exit stock RNS with the result."""
    import RNS

    code = 1
    try:
        code = report(title, run())
        return code
    finally:
        RNS.exit(code)


def free_udp_port() -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def ifac_lines(on: bool) -> str:
    """Stock IFAC keys without `ifac_size`, so stock applies the datagram default of 16."""
    return f"    network_name = {NETWORK_NAME}\n    passphrase = {PASSPHRASE}\n" if on else ""


def wait(predicate, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return bool(predicate())


def exercise(retinue: Retinue, timeout: float = 90) -> dict[str, bool]:
    """Path, link, a stock Resource, a proved stock packet and Retinue's Resource back."""
    import RNS

    results: dict[str, bool] = {}
    match = retinue.wait_for(r"DEST ([0-9a-f]{32})", 120)
    if match is None:
        return {"retinue printed its destination": False}
    dest = bytes.fromhex(match.group(1))
    results["stock has_path(retinue)"] = wait(lambda: RNS.Transport.has_path(dest), timeout)
    identity = RNS.Identity.recall(dest)
    if identity is None:
        return results
    back: dict = {}
    concluded = threading.Event()

    def on_concluded(resource):
        back["status"] = resource.status
        back["data"] = resource.data.read() if resource.status == RNS.Resource.COMPLETE else b""
        concluded.set()

    def established(link):
        link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
        link.set_resource_concluded_callback(on_concluded)

    out = RNS.Destination(identity, RNS.Destination.OUT, RNS.Destination.SINGLE,
                          "retinue", "datagram-peer")
    link = RNS.Link(out, established_callback=established)
    results["stock link ACTIVE"] = wait(lambda: link.status == RNS.Link.ACTIVE, 30)
    if not results["stock link ACTIVE"]:
        return results

    resource = RNS.Resource(payload(RESOURCE_LEN, RESOURCE_SEED), link)
    wait(lambda: resource.status in (RNS.Resource.COMPLETE, RNS.Resource.FAILED), timeout)
    results[f"stock Resource COMPLETE ({resource.total_parts} parts)"] = (
        resource.status == RNS.Resource.COMPLETE and resource.total_parts > 1)
    results["retinue got the exact Resource"] = retinue.wait_for(
        rf"RESOURCE {RESOURCE_LEN} OK", 10) is not None

    receipt = RNS.Packet(link, payload(PACKET_LEN, PACKET_SEED)).send()
    wait(lambda: receipt.status != RNS.PacketReceipt.SENT, 15)
    results["stock PacketReceipt DELIVERED"] = receipt.status == RNS.PacketReceipt.DELIVERED

    concluded.wait(timeout)
    results["stock received Retinue's Resource intact"] = (
        back.get("status") == RNS.Resource.COMPLETE
        and back.get("data") == payload(PUBLISH_LEN, PUBLISH_SEED))
    link.teardown()
    time.sleep(1)
    return results
