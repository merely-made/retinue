"""IFAC full-size frame gate: stock RNS TCPServerInterface with IFAC <-> Retinue Endpoint.

A 431-byte link packet is 499 bytes logical, so 515 on the wire at ifac_size 128 and 563
at 512; a Resource part (450 logical, Resource.py 344) is 514 at 512. Both are past the
bare 500-byte MTU, so a deframer capped there drops them.
Run once per size; each run must hold on stock's own state:
  * the RNS sender's 64 KiB Resource reaches COMPLETE (Retinue proved every part);
  * RNS's 431-byte link packet receipt reaches DELIVERED (Retinue proved it);
  * RNS's resource-concluded callback gets Retinue's 64 KiB Resource intact.

Run: python -u interop_ifac_resource.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

from library_gate import RESOURCE_STATUS, payload, verdict

HERE = Path(__file__).resolve().parent
REPO = HERE.parent
EXAMPLE = "ifac_resource"
CONFIG_ENV = "RETINUE_IFAC_GATE_CONFIG_DIR"
EXPECTED_RNS = "1.5.7"
NETWORK_NAME = "retinue-ifac-carrier"  # examples/ifac_resource.rs
PASSPHRASE = "full-size-frames"
RESOURCE_LEN = 64 * 1024
LINK_MDU = 431
SEED = 0x1FAC
TITLE = "IFAC RESOURCE INTEROP"
SIZES_BITS = (128, 512)


def binary() -> Path:
    target = Path(os.environ.get("CARGO_TARGET_DIR", REPO.parent.parent / "target"))
    return target / "debug" / "examples" / EXAMPLE


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


class Example:
    """The running Retinue example and its captured output."""

    def __init__(self, args: list[str]) -> None:
        print(f"Retinue: {EXAMPLE} {' '.join(args)}", flush=True)
        self.proc = subprocess.Popen([str(binary()), *args], cwd=REPO, stdout=subprocess.PIPE,
                                     stderr=subprocess.STDOUT, text=True, bufsize=1)
        self.lines: list[str] = []
        self.lock = threading.Lock()
        threading.Thread(target=self._pump, daemon=True).start()

    def _pump(self) -> None:
        assert self.proc.stdout is not None
        for raw in self.proc.stdout:
            with self.lock:
                self.lines.append(raw.rstrip())
            print(f"  [retinue] {raw.rstrip()}", flush=True)

    def find(self, pattern: str) -> re.Match[str] | None:
        with self.lock:
            lines = list(self.lines)
        return next((m for m in (re.fullmatch(pattern, l) for l in lines) if m), None)

    def wait_for(self, pattern: str, timeout: float) -> re.Match[str] | None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if (match := self.find(pattern)) or self.proc.poll() is not None:
                return match or self.find(pattern)
            time.sleep(0.05)
        return None

    def finish(self, timeout: float) -> int | None:
        try:
            return self.proc.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            return None


def start_stock_server(port: int, ifac_bits: int | None):
    """A fresh stock RNS whose only interface is an IFAC TCPServerInterface on `port`."""
    import RNS

    if RNS.__version__ != EXPECTED_RNS:
        raise RuntimeError(f"expected stock RNS {EXPECTED_RNS}, got {RNS.__version__}")
    config_dir = Path(os.environ[CONFIG_ENV])
    (config_dir / "storage" / "resources").mkdir(parents=True, exist_ok=True)
    size_line = f"    ifac_size = {ifac_bits}\n" if ifac_bits is not None else ""
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport = No\n  share_instance = No\n"
        "  panic_on_interface_error = No\n\n[logging]\n  loglevel = 3\n\n"
        "[interfaces]\n  [[ifac-server]]\n    type = TCPServerInterface\n    enabled = yes\n"
        f"    listen_ip = 127.0.0.1\n    listen_port = {port}\n"
        f"    network_name = {NETWORK_NAME}\n    passphrase = {PASSPHRASE}\n{size_line}",
        encoding="utf-8")
    reticulum = RNS.Reticulum(configdir=str(config_dir))
    server = next(i for i in RNS.Transport.interfaces if "ifac-server" in str(i))
    print(f"RNS {RNS.__version__}, stock ifac_size {server.ifac_size} bytes", flush=True)
    return reticulum, server


def peer(bits: int) -> int:
    port = free_port()
    import RNS

    _, server = start_stock_server(port, bits)
    retinue = Example(["resource", str(port), str(bits)])
    exit_code = 1
    try:
        state: dict[str, object] = {}
        sent_done, back_done = threading.Event(), threading.Event()

        def inbound_concluded(resource):
            data = resource.data.read() if hasattr(resource.data, "read") else bytes(resource.data or b"")
            state.update(back_status=resource.status, back_data=data)
            print(f"  RNS receiver concluded: {RESOURCE_STATUS.get(resource.status)}, {len(data)} bytes",
                  flush=True)
            back_done.set()

        def established(link):
            link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
            link.set_resource_concluded_callback(inbound_concluded)
            print(f"  RNS: link up, sending {RESOURCE_LEN}-byte Resource", flush=True)
            state["resource"] = RNS.Resource(payload(RESOURCE_LEN, SEED), link,
                                             callback=lambda r: sent_done.set())

        class Linker:
            aspect_filter = "retinue.ifac-resource"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if state.get("link"):
                    return
                remote = RNS.Destination(announced_identity, RNS.Destination.OUT,
                                         RNS.Destination.SINGLE, "retinue", "ifac-resource")
                state["link"] = link = RNS.Link(remote)
                link.set_link_established_callback(established)

        RNS.Transport.register_announce_handler(Linker())
        sent_done.wait(timeout=90)
        resource = state.get("resource")
        sent_status = getattr(resource, "status", None)

        receipt = None
        link = state.get("link")
        if sent_status == RNS.Resource.COMPLETE:
            packet = RNS.Packet(link, payload(LINK_MDU, SEED + 1))
            print(f"  RNS: sending a {LINK_MDU}-byte link packet", flush=True)
            receipt = packet.send()
            deadline = time.monotonic() + 15
            while receipt and receipt.status == RNS.PacketReceipt.SENT and time.monotonic() < deadline:
                time.sleep(0.1)
            back_done.wait(timeout=60)
        if link is not None:
            link.teardown()
        process_code = retinue.finish(30)

        print("\n" + "=" * 72)
        print(f"ifac_size {bits} bits; RNS server ifac_size {server.ifac_size} bytes")
        ok = verdict("stock interface uses the configured size", server.ifac_size == bits // 8)
        ok &= verdict("RNS sender's Resource reached COMPLETE", sent_status == RNS.Resource.COMPLETE,
                      RESOURCE_STATUS.get(sent_status, str(sent_status)))
        ok &= verdict("Retinue received the exact Resource", retinue.find("RESOURCE_OK") is not None)
        delivered = receipt is not None and receipt.status == RNS.PacketReceipt.DELIVERED
        ok &= verdict(f"RNS {LINK_MDU}-byte link packet receipt DELIVERED", delivered,
                      str(getattr(receipt, "status", None)))
        ok &= verdict("Retinue received the exact link packet", retinue.find(f"DATA {LINK_MDU} OK") is not None)
        back = state.get("back_data", b"")
        ok &= verdict("RNS received Retinue's Resource intact",
                      state.get("back_status") == RNS.Resource.COMPLETE
                      and back == payload(RESOURCE_LEN, SEED + 2), f"{len(back)} bytes")
        ok &= verdict("Retinue publish returned after RNS's proof",
                      retinue.find(f"PUBLISH_OK {RESOURCE_LEN}") is not None)
        ok &= verdict("Retinue process exited zero", process_code == 0, str(process_code))
        print("=" * 72, flush=True)
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        retinue.finish(1)
        RNS.exit(exit_code)


def supervise(script: str, title: str, runs: list[list[str]], timeout: float) -> int:
    """Run `script --peer ARGS` once per entry, each with a private RNS config directory."""
    # Build before any network timeout starts.
    subprocess.run(["cargo", "build", "--quiet", "-p", "retinue", "--example", EXAMPLE],
                   cwd=REPO, check=True)
    failed = []
    for args in runs:
        config_dir = Path(tempfile.mkdtemp(prefix="retinue-ifac-gate-"))
        env = dict(os.environ, **{CONFIG_ENV: str(config_dir)})
        child = subprocess.Popen([sys.executable, "-u", script, "--peer", *args], env=env)
        try:
            code = child.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait(timeout=5)
            code = None
        finally:
            shutil.rmtree(config_dir, ignore_errors=True)
        print(f"{title} [{' '.join(args)}]: {'PASS' if code == 0 else 'FAIL'}", flush=True)
        if code != 0:
            failed.append(" ".join(args))
    print(f"{title}: {'PASS' if not failed else 'FAIL (' + ', '.join(failed) + ')'}", flush=True)
    return 0 if not failed else 1


if __name__ == "__main__":
    if sys.argv[1:2] == ["--peer"]:
        raise SystemExit(peer(int(sys.argv[2])))
    raise SystemExit(supervise(__file__, TITLE, [[str(b)] for b in SIZES_BITS], 240))
