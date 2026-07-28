"""Submit one message from stock LXMF to a Retinue propagation capture node."""

from __future__ import annotations

import atexit
import hashlib
import os
import re
import shutil
import subprocess
import tempfile
import threading
import time
from pathlib import Path

import LXMF
import RNS
import RNS.vendor.umsgpack as msgpack


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
TITLE = b"PROPAGATION TITLE"
CONTENT = b"PROPAGATION BODY"
TIMESTAMP = 1_753_603_204.5
SENDER_SEED = bytes([0x61] * 64)
RECEIVER_SEED = bytes([0x62] * 64)


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    capture_command = os.environ.get("OUTRIDER_PROPAGATION_CAPTURE")
    command = (
        [capture_command]
        if capture_command
        else [
            "cargo",
            "run",
            "--quiet",
            "-p",
            "outrider",
            "--example",
            "stock_propagation_submit_receive",
        ]
    )
    capture = subprocess.Popen(
        command,
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert capture.stdout is not None
        for raw in capture.stdout:
            line = raw.rstrip()
            lines.append(line)
            print(f"  [outrider] {line}")

    threading.Thread(target=pump, daemon=True).start()
    port = None
    node_destination = None
    deadline = time.time() + 180
    while time.time() < deadline and (port is None or node_destination is None):
        for line in list(lines):
            port_match = re.fullmatch(r"LISTENING (\d+)", line)
            if port_match:
                port = int(port_match.group(1))
            destination_match = re.fullmatch(
                r"PROPAGATION_DESTINATION ([0-9a-f]{32})", line
            )
            if destination_match:
                node_destination = bytes.fromhex(destination_match.group(1))
        if capture.poll() is not None:
            return 1
        time.sleep(0.1)
    if port is None or node_destination is None:
        capture.kill()
        return 1

    root = Path(tempfile.mkdtemp(prefix="outrider-propagation-submit-"))
    rns_config = root / "rns"
    store = root / "store"
    rns_config.mkdir()
    store.mkdir()
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

    exit_code = 1
    RNS.Reticulum(configdir=str(rns_config))
    try:
        sender_identity = RNS.Identity.from_bytes(SENDER_SEED)
        receiver_identity = RNS.Identity.from_bytes(RECEIVER_SEED)
        router = LXMF.LXMRouter(identity=sender_identity, storagepath=str(store))
        source = router.register_delivery_identity(
            sender_identity, display_name="Propagation Sender", stamp_cost=None
        )
        node_seen = threading.Event()

        class PropagationAnnounce:
            aspect_filter = "lxmf.propagation"

            def received_announce(
                self, destination_hash, announced_identity, app_data
            ) -> None:
                if destination_hash != node_destination or node_seen.is_set():
                    return
                print(f"  stock: learned capture node {destination_hash.hex()}")
                router.set_outbound_propagation_node(bytes(destination_hash))
                node_seen.set()

        RNS.Transport.register_announce_handler(PropagationAnnounce())

        if not node_seen.wait(timeout=45):
            print("stock learned capture node: FAIL")
            return 1
        destination = RNS.Destination(
            receiver_identity,
            RNS.Destination.OUT,
            RNS.Destination.SINGLE,
            "lxmf",
            "delivery",
        )
        message = LXMF.LXMessage(
            destination,
            source,
            CONTENT,
            title=TITLE,
            desired_method=LXMF.LXMessage.PROPAGATED,
        )
        message.timestamp = TIMESTAMP
        router.handle_outbound(message)
        print(f"  stock: queued {message.hash.hex()}")

        result_deadline = time.time() + 180
        while time.time() < result_deadline:
            if any(line.startswith("SUBMISSION ") for line in lines):
                break
            if capture.poll() is not None:
                break
            time.sleep(0.2)
        try:
            capture.wait(timeout=10)
        except subprocess.TimeoutExpired:
            capture.kill()

        captured = any(line.startswith("SUBMISSION ") for line in lines)
        data_packet = "SUBMISSION_MODE Data" in lines
        decoded = any(line.startswith("SUBMISSION_DECODED ") for line in lines)
        production_receive = "PRODUCTION_RECEIVE true" in lines
        decoded_message = (
            f"DECRYPTED_MESSAGE_ID {message.hash.hex()}" in lines
            and f"DECRYPTED_TITLE {TITLE.hex()}" in lines
            and f"DECRYPTED_CONTENT {CONTENT.hex()}" in lines
        )
        submission_hex = next(
            (line[11:] for line in lines if line.startswith("SUBMISSION ")), None
        )
        stock_decrypted = False
        if submission_hex is not None:
            packed_submission = bytes.fromhex(submission_hex)
            outer = msgpack.unpackb(packed_submission)
            opaque = bytes(outer[1][0])
            encrypted_data = opaque[16:-32]
            propagation_stamp = opaque[-32:]
            transient_id = bytes(message.transient_id)
            print(f"  stock: captured propagation_stamp={propagation_stamp.hex()}")
            print(
                "  stock: transient_match="
                f"{hashlib.sha256(opaque[:16] + encrypted_data).digest() == transient_id}"
            )
            print(f"  stock: recipient destination {destination.hash.hex()}")
            inbound_destination = RNS.Destination(
                receiver_identity,
                RNS.Destination.IN,
                RNS.Destination.SINGLE,
                "lxmf",
                "delivery",
            )
            for label, decryptor in (
                (
                    "identity_encrypted_data",
                    lambda: receiver_identity.decrypt(opaque[16:-32]),
                ),
                (
                    "destination_encrypted_data",
                    lambda: inbound_destination.decrypt(opaque[16:-32]),
                ),
            ):
                try:
                    plaintext = decryptor()
                    if plaintext is not None:
                        print(f"  stock: {label} {bytes(plaintext).hex()}")
                        stock_decrypted = True
                except Exception as error:
                    print(f"  stock: {label} failed {error}")
        print(f"stock learned capture node: {'PASS' if node_seen.is_set() else 'FAIL'}")
        print(f"Retinue received submission: {'PASS' if captured else 'FAIL'}")
        print(f"stock used one data packet: {'PASS' if data_packet else 'FAIL'}")
        print(f"outer container is MessagePack: {'PASS' if decoded else 'FAIL'}")
        print(f"production receiver accepted entry: {'PASS' if production_receive else 'FAIL'}")
        print(f"production receiver decoded message: {'PASS' if decoded_message else 'FAIL'}")
        print(f"stock primitive decrypted inner object: {'PASS' if stock_decrypted else 'FAIL'}")
        ok = (
            node_seen.is_set()
            and captured
            and data_packet
            and decoded
            and production_receive
            and decoded_message
            and stock_decrypted
        )
        print(f"PROPAGATION_SUBMIT_CAPTURE: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, root, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
