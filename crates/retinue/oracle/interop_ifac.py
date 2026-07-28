"""Live IFAC gate: Retinue and pinned RNS authenticate each other over TCP."""

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

import RNS


HERE = Path(__file__).resolve().parent
REPO = HERE.parent
ORACLE_SEED = bytes.fromhex(
    "f0ecbba49e783dee14ffc6c9f1e1251efa7d7629e0fa32413c5c59ec2e0f6d6c" * 2
)
RETINUE_ASPECT = "retinue.interop_ifac"
RETINUE_APP_DATA = b"hello-from-retinue-ifac"
NETWORK_NAME = "retinue-ifac-interop"
PASSPHRASE = "mixed-runtime"

got_retinue_announce: dict = {}
announce_seen = threading.Event()


class RetinueAnnounceHandler:
    aspect_filter = RETINUE_ASPECT

    def received_announce(self, destination_hash, announced_identity, app_data):
        got_retinue_announce.update(
            destination_hash=destination_hash.hex(),
            identity_hash=announced_identity.hash.hex(),
            app_data=bytes(app_data) if app_data else b"",
        )
        announce_seen.set()


def main() -> int:
    print(f"RNS {RNS.__version__}")
    proc = subprocess.Popen(
        ["cargo", "run", "--quiet", "--example", "interop_ifac"],
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            line = line.rstrip()
            lines.append(line)
            print(f"  [retinue] {line}")

    threading.Thread(target=pump, daemon=True).start()

    port = None
    deadline = time.time() + 120
    while time.time() < deadline:
        for line in list(lines):
            match = re.match(r"LISTENING (\d+)", line)
            if match:
                port = int(match.group(1))
                break
        if port:
            break
        if proc.poll() is not None:
            print("retinue exited before it listened", file=sys.stderr)
            return 1
        time.sleep(0.2)
    if port is None:
        proc.kill()
        print("timed out waiting for retinue to listen", file=sys.stderr)
        return 1

    config_dir = Path(tempfile.mkdtemp(prefix="retinue-ifac-interop-"))
    atexit.register(shutil.rmtree, config_dir, ignore_errors=True)
    (config_dir / "config").write_text(
        "[reticulum]\n"
        "  enable_transport = No\n"
        "  share_instance = No\n"
        "  panic_on_interface_error = No\n"
        "\n[logging]\n"
        "  loglevel = 3\n"
        "\n[interfaces]\n"
        "  [[retinue-ifac]]\n"
        "    type = TCPClientInterface\n"
        "    enabled = yes\n"
        "    target_host = 127.0.0.1\n"
        f"    target_port = {port}\n"
        f"    network_name = {NETWORK_NAME}\n"
        f"    passphrase = {PASSPHRASE}\n"
        "    ifac_size = 64\n",
        encoding="utf-8",
    )

    RNS.Reticulum(configdir=str(config_dir))
    exit_code = 1
    try:
        RNS.Transport.register_announce_handler(RetinueAnnounceHandler())
        identity = RNS.Identity.from_bytes(ORACLE_SEED)
        destination = RNS.Destination(
            identity,
            RNS.Destination.IN,
            RNS.Destination.SINGLE,
            "retinue",
            "ifac_oracle",
        )

        time.sleep(2.5)
        destination.announce(app_data=b"hello-from-rns-ifac")
        announce_seen.wait(timeout=15)
        time.sleep(1)

        output = "\n".join(lines)
        rns_accepted = (
            bool(got_retinue_announce)
            and got_retinue_announce["app_data"] == RETINUE_APP_DATA
        )
        retinue_accepted = "VALIDATED_RNS_IFAC_ANNOUNCE" in output
        print(f"retinue -> RNS IFAC: {'PASS' if rns_accepted else 'FAIL'}")
        print(f"RNS -> retinue IFAC: {'PASS' if retinue_accepted else 'FAIL'}")
        ok = rns_accepted and retinue_accepted
        print(f"IFAC INTEROP: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        try:
            proc.wait(timeout=8)
        except subprocess.TimeoutExpired:
            proc.kill()
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
