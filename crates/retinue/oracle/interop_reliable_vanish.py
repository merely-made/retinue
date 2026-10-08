"""Vanished-peer gate (review #15): Retinue closes a reliable link whose RNS peer dies.

Stock RNS links to Retinue's reliable destination, identifies, and reads a long stream
that Retinue writes through its reliable Endpoint stream, using only public RNS APIs
(`RNS.Link`, `Link.get_channel`, `RNS.Buffer.create_reader`). Once RNS has read part of
the stream, this driver kills the RNS process outright, so it sends no link close and
proves nothing more.

Must hold: RNS read stream bytes before the kill; Retinue's reader then fails with
`TimedOut` (its channel gave each packet RNS's five tries and closed the link) rather
than waiting forever; the link is gone from Retinue's facts; Retinue exits zero.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_reliable_vanish.py   (CARGO_TARGET_DIR as for the build)
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
MIDSTREAM = 8 * 1024


def peer() -> int:
    """The stock RNS side: link in, read Retinue's stream until killed."""
    import RNS

    if RNS.__version__ != "1.5.7":
        raise RuntimeError(f"expected stock RNS 1.5.7, got {RNS.__version__}")
    print(f"RNS {RNS.__version__}", flush=True)
    config_dir = Path(os.environ["RETINUE_VANISH_CONFIG_DIR"])
    port = int(os.environ["RETINUE_VANISH_PORT"])
    (config_dir / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=3\n\n"
        "[interfaces]\n  [[retinue]]\n    type=TCPClientInterface\n"
        "    enabled=yes\n    target_host=127.0.0.1\n"
        f"    target_port={port}\n",
        encoding="utf-8",
    )
    RNS.Reticulum(configdir=str(config_dir))
    identity = RNS.Identity()
    started = threading.Event()
    links: list[object] = []

    class Linker:
        aspect_filter = "retinue.vanish-oracle"

        def received_announce(self, destination_hash, announced_identity, app_data):
            if started.is_set():
                return
            started.set()
            remote = RNS.Destination(
                announced_identity, RNS.Destination.OUT,
                RNS.Destination.SINGLE, "retinue", "vanish-oracle",
            )
            link = RNS.Link(remote)
            links.append(link)

            def established(established_link):
                established_link.identify(identity)
                channel = established_link.get_channel()
                reader = RNS.Buffer.create_reader(0, channel)

                def read() -> None:
                    received = 0
                    announced = False
                    while True:
                        chunk = reader.read(4096)
                        if not chunk:
                            time.sleep(0.02)
                            continue
                        received += len(chunk)
                        if received >= MIDSTREAM and not announced:
                            announced = True
                            print(f"RNS_MIDSTREAM {received}", flush=True)

                threading.Thread(target=read, daemon=True).start()

            link.set_link_established_callback(established)

    RNS.Transport.register_announce_handler(Linker())
    print("RNS_WAITING", flush=True)
    time.sleep(120)  # The driver kills this process long before.
    return 1


def pump(proc: subprocess.Popen, tag: str, lines: list[str], lock: threading.Lock) -> None:
    assert proc.stdout is not None
    for raw in proc.stdout:
        line = raw.rstrip()
        with lock:
            lines.append(line)
        print(f"  [{tag}] {line}", flush=True)


def wait_for(lines: list[str], lock: threading.Lock, pattern: str, seconds: float,
             alive: subprocess.Popen) -> re.Match | None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with lock:
            for line in lines:
                match = re.fullmatch(pattern, line)
                if match:
                    return match
        if alive.poll() is not None:
            return None
        time.sleep(0.05)
    return None


def main() -> int:
    default_target = r"C:\t\cargo-targets\retinue" if os.name == "nt" else str(REPO.parent.parent / "target")
    target = Path(os.environ.get("CARGO_TARGET_DIR", default_target))
    name = "reliable_vanish_oracle.exe" if os.name == "nt" else "reliable_vanish_oracle"
    binary = target / "debug" / "examples" / name
    if not binary.is_file():
        raise FileNotFoundError(f"build the reliable_vanish_oracle example first: {binary}")
    print(f"Retinue binary: {binary}", flush=True)

    lock = threading.Lock()
    retinue_lines: list[str] = []
    peer_lines: list[str] = []
    config_dir = Path(tempfile.mkdtemp(prefix="retinue-vanish-oracle-"))
    retinue = subprocess.Popen(
        [str(binary)], cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, bufsize=1,
    )
    rns: subprocess.Popen | None = None
    try:
        threading.Thread(target=pump, args=(retinue, "retinue", retinue_lines, lock),
                         daemon=True).start()
        listening = wait_for(retinue_lines, lock, r"LISTENING (\d+)", 60, retinue)
        if listening is None:
            print("RELIABLE VANISH INTEROP: FAIL (Retinue did not listen)")
            return 1
        env = os.environ.copy()
        env["RETINUE_VANISH_CONFIG_DIR"] = str(config_dir)
        env["RETINUE_VANISH_PORT"] = listening.group(1)
        rns = subprocess.Popen(
            [sys.executable, "-u", __file__, "--peer"], env=env, stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        threading.Thread(target=pump, args=(rns, "rns", peer_lines, lock), daemon=True).start()

        midstream = wait_for(peer_lines, lock, r"RNS_MIDSTREAM (\d+)", 60, rns)
        print(f"RNS read part of the stream: {'PASS' if midstream else 'FAIL'}")
        if midstream is None:
            print("RELIABLE VANISH INTEROP: FAIL")
            return 1
        rns.kill()  # No link close, no more proofs: the peer is simply gone.
        rns.wait(timeout=10)
        killed_at = time.monotonic()

        try:
            code = retinue.wait(timeout=120)
        except subprocess.TimeoutExpired:
            code = None
        elapsed = time.monotonic() - killed_at
        time.sleep(0.1)
        with lock:
            joined = "\n".join(retinue_lines)
        timed_out = re.search(r"^CLOSED TimedOut", joined, re.MULTILINE) is not None
        gone = "LINK_GONE" in joined
        print(f"Retinue stream failed with TimedOut: {'PASS' if timed_out else 'FAIL'} "
              f"({elapsed:.1f}s after the kill)")
        print(f"Retinue forgot the link: {'PASS' if gone else 'FAIL'}")
        print(f"Retinue process exited zero: {'PASS' if code == 0 else 'FAIL'} ({code})")
        success = timed_out and gone and "DONE" in joined and code == 0
        print(f"RELIABLE VANISH INTEROP: {'PASS' if success else 'FAIL'}")
        return 0 if success else 1
    finally:
        for proc in (rns, retinue):
            if proc is not None and proc.poll() is None:
                proc.kill()
                proc.wait(timeout=5)
        shutil.rmtree(config_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(peer() if sys.argv[1:] == ["--peer"] else main())
