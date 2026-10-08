"""Ratchet rotation live gate: stock RNS encrypts to a Retinue ratchet that has rotated.

A LOCAL gate, not a CI test: it needs the Python oracle and runs the
`ratchet_rotation_oracle` example over TCP. It uses only public RNS APIs.

What it proves:

  1. A Retinue announce rotates its ratchet once the interval has passed, and stock RNS
     adopts the new ratchet from that announce.
  2. A packet stock RNS encrypted to the previous ratchet, sent after the rotation,
     still decrypts against Retinue's retained epoch and reports that epoch's id.
  3. A packet encrypted to the new ratchet decrypts against the current epoch.
  4. Retinue persisted the new ratchet before it advertised it.

Run from the repository root:

    python -u crates/retinue/oracle/interop_ratchet_rotation.py
"""

from __future__ import annotations

import atexit
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import RNS

HERE = Path(__file__).resolve().parent
REPO = HERE.parent

APP = "retinue"
ASPECTS = ("ratchet", "rotation")
OLDER = b"OLDER-EPOCH"
CURRENT = b"CURRENT-EPOCH"
# The example rotates when its current epoch is older than 2 s, on whole host seconds.
ROTATION_WAIT = 3.5


def wait_for(predicate, timeout: float) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        time.sleep(0.1)
    return predicate()


def main() -> int:
    print(f"RNS {RNS.__version__}")
    executable = os.environ.get("RETINUE_RATCHET_ROTATION")
    command = (
        [executable]
        if executable
        else [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "retinue",
            "--example",
            "ratchet_rotation_oracle",
        ]
    )
    process = subprocess.Popen(
        command,
        cwd=REPO,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert process.stdout is not None
        for raw in process.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [retinue] {line}")

    threading.Thread(target=pump, daemon=True).start()

    def field(prefix: str) -> str | None:
        return next((line[len(prefix):] for line in list(lines) if line.startswith(prefix)), None)

    if not wait_for(lambda: field("DESTINATION ") is not None or process.poll() is not None, 300):
        process.kill()
        return 1
    port, destination = field("LISTENING "), field("DESTINATION ")
    if port is None or destination is None:
        process.kill()
        return 1
    destination_hash = bytes.fromhex(destination)

    def command_retinue(command: str) -> None:
        assert process.stdin is not None
        process.stdin.write(command + "\n")
        process.stdin.flush()

    config = Path(tempfile.mkdtemp(prefix="retinue-ratchet-rotation-"))
    (config / "config").write_text(
        "[reticulum]\n"
        "  enable_transport=No\n"
        "  share_instance=No\n"
        "  panic_on_interface_error=No\n"
        "\n[logging]\n"
        "  loglevel=2\n"
        "\n[interfaces]\n"
        "  [[retinue]]\n"
        "    type=TCPClientInterface\n"
        "    enabled=yes\n"
        f"    target_host=127.0.0.1\n"
        f"    target_port={int(port)}\n",
        encoding="utf-8",
    )

    exit_code = 1
    RNS.Reticulum(configdir=str(config))
    try:
        def learn(previous: bytes | None) -> bytes | None:
            # Ask Retinue to announce until stock RNS holds a ratchet other than `previous`.
            for _ in range(5):
                command_retinue("ANNOUNCE")
                if wait_for(
                    lambda: RNS.Identity.current_ratchet_id(destination_hash) not in (None, previous),
                    2,
                ):
                    return RNS.Identity.current_ratchet_id(destination_hash)
            return None

        first = learn(None)
        identity = RNS.Identity.recall(destination_hash)
        if first is None or identity is None:
            print("stock learned the first ratchet: FAIL")
            return 1
        print(f"stock learned ratchet {first.hex()}")
        outgoing = RNS.Destination(identity, RNS.Destination.OUT, RNS.Destination.SINGLE, APP, *ASPECTS)
        # Packing encrypts now, to the first ratchet; sending happens after the rotation.
        held = RNS.Packet(outgoing, OLDER, create_receipt=False)
        held.pack()

        time.sleep(ROTATION_WAIT)
        second = learn(first)
        if second is None:
            print("stock learned the rotated ratchet: FAIL")
            return 1
        print(f"stock learned rotated ratchet {second.hex()}")

        held.send()
        RNS.Packet(outgoing, CURRENT, create_receipt=False).send()
        older_line = f"RECEIVED {OLDER.decode()} {first.hex()}"
        current_line = f"RECEIVED {CURRENT.decode()} {second.hex()}"
        wait_for(lambda: older_line in lines and current_line in lines, 10)
        command_retinue("QUIT")
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        persisted = f"PERSISTED {second.hex()}"
        announced = f"ANNOUNCED {second.hex()}"
        ordered = (
            persisted in lines
            and announced in lines
            and lines.index(persisted) < lines.index(announced)
        )
        checks = {
            "announce rotated the ratchet and stock adopted it": first != second,
            "older epoch decrypted after rotation": older_line in lines,
            "current epoch decrypted": current_line in lines,
            "rotated ratchet persisted before advertised": ordered,
        }
        for label, ok in checks.items():
            print(f"{label}: {'PASS' if ok else 'FAIL'}")
        ok = all(checks.values())
        print(f"RATCHET_ROTATION_INTEROP: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        if process.poll() is None:
            process.kill()
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
