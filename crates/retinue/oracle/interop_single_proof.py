"""Single-packet proof gate: delivery receipts both ways, and identity-key encryption.

  1. RNS -> retinue. retinue registers `retinue.single` proving every packet. RNS sends it
     one single packet with a delivery callback; the receipt must reach DELIVERED.
  2. retinue -> RNS. RNS registers `retinue.rnsrecv` with PROVE_ALL and no ratchets.
     retinue sends it one single packet (encrypted to the identity key, since no ratchet
     is advertised); RNS must receive it and retinue's receipt must reach DELIVERED.

Run from the oracle/ directory:  ./.venv/Scripts/python.exe -u interop_single_proof.py
"""
from __future__ import annotations

import atexit
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
RNS_SEED = bytes([0x6B] * 64)

state = {"rns_received": [], "receipt": None}
finished = threading.Event()


def main() -> int:
    print(f"RNS {RNS.__version__}\n")
    proc = subprocess.Popen(
        ["cargo", "run", "--quiet", "--example", "single_proof_interop"],
        cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1,
    )
    lines: list[str] = []

    def pump():
        for line in proc.stdout:
            line = line.rstrip()
            lines.append(line)
            print(f"  [retinue] {line}")

    threading.Thread(target=pump, daemon=True).start()

    port = None
    deadline = time.time() + 180
    while time.time() < deadline and port is None:
        for line in list(lines):
            m = re.match(r"LISTENING (\d+)", line)
            if m:
                port = int(m.group(1))
                break
        if proc.poll() is not None:
            return 1
        time.sleep(0.2)
    if port is None:
        proc.kill()
        return 1

    cfg = Path(tempfile.mkdtemp(prefix="retinue-sp-"))
    (cfg / "config").write_text(
        "[reticulum]\n  enable_transport = No\n  share_instance = No\n"
        "  panic_on_interface_error = No\n\n[logging]\n  loglevel = 3\n\n[interfaces]\n"
        "  [[retinue]]\n    type = TCPClientInterface\n    enabled = yes\n"
        f"    target_host = 127.0.0.1\n    target_port = {port}\n",
        encoding="utf-8",
    )
    RNS.Reticulum(configdir=str(cfg))
    exit_code = 1
    try:
        identity = RNS.Identity.from_bytes(RNS_SEED)
        inbound = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                                  "retinue", "rnsrecv")
        inbound.set_proof_strategy(RNS.Destination.PROVE_ALL)

        def on_packet(data, packet):
            print(f"  RNS: received {bytes(data)!r}")
            state["rns_received"].append(bytes(data))

        inbound.set_packet_callback(on_packet)
        print(f"RNS destination {inbound.hash.hex()} (no ratchets, PROVE_ALL)")

        class SendToRetinue:
            aspect_filter = "retinue.single"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if state.get("sent"):
                    return
                state["sent"] = True
                out = RNS.Destination(announced_identity, RNS.Destination.OUT,
                                      RNS.Destination.SINGLE, "retinue", "single")
                receipt = RNS.Packet(out, b"from-rns").send()
                state["receipt"] = receipt
                print(f"  RNS: sent to retinue {destination_hash.hex()}")

                def delivered(r):
                    print(f"  RNS: receipt DELIVERED in {r.get_rtt():.3f}s")
                    finished.set()

                def timed_out(r):
                    print("  RNS: receipt TIMED OUT")
                    finished.set()

                receipt.set_delivery_callback(delivered)
                receipt.set_timeout_callback(timed_out)

        RNS.Transport.register_announce_handler(SendToRetinue())

        # Announce until retinue has heard us and sent its packet.
        deadline = time.time() + 40
        while time.time() < deadline and "DONE" not in "\n".join(lines):
            if not any(line.startswith("RNS_ANNOUNCE") for line in lines):
                inbound.announce()
            time.sleep(1.0)
        finished.wait(timeout=5)
        time.sleep(0.5)

        joined = "\n".join(lines)
        receipt = state["receipt"]
        rns_delivered = receipt is not None and receipt.status == RNS.PacketReceipt.DELIVERED
        retinue_got = "RECEIVED from-rns" in joined
        rns_got = b"from-retinue" in state["rns_received"]
        identity_key = "SENT ratchet=none" in joined
        retinue_delivered = "RECEIPT DELIVERED" in joined

        print("\n" + "=" * 68)
        print(f"RNS -> retinue packet received:           {'PASS' if retinue_got else 'FAIL'}")
        print(f"RNS receipt DELIVERED by retinue proof:   {'PASS' if rns_delivered else 'FAIL'}")
        print(f"retinue -> RNS via identity key received: "
              f"{'PASS' if (identity_key and rns_got) else 'FAIL'}")
        print(f"retinue receipt DELIVERED by RNS proof:   {'PASS' if retinue_delivered else 'FAIL'}")
        print("=" * 68)
        ok = retinue_got and rns_delivered and identity_key and rns_got and retinue_delivered
        print(f"SINGLE PACKET PROOF INTEROP: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        try:
            proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            proc.kill()
        atexit.register(shutil.rmtree, cfg, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
