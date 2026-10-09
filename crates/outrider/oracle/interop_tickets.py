"""Tickets between pinned stock LXMF and Outrider, judged on stock's side.

Stock (enforcing stamps at cost 8) sends with include_ticket=True. Outrider learns
the ticket, and its reply carries the ticket stamp instead of proof of work plus a
ticket of its own. The gate requires stock to accept that reply as a ticket stamp,
to hold Outrider's ticket as its outbound ticket, and to spend it on its next
message, which Outrider must accept at cost 8.

Stock's messages are sized to travel as Resources: Outrider does not yet prove a
direct Data packet, and stock would resend it on the link that carries the next.
Both must reach DELIVERED on stock's side.
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

import LXMF
import RNS
from LXMF import LXMessage


HERE = Path(__file__).resolve().parent
REPO = HERE.parents[2]
SEED = bytes([0x79] * 64)
BULK = "x" * 1000


def main() -> int:
    print(f"LXMF {LXMF.__version__} / RNS {RNS.__version__}")
    process = subprocess.Popen(
        ["cargo", "run", "--quiet", "-p", "outrider", "--example", "stock_tickets"],
        cwd=REPO,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    lines: list[str] = []

    def pump() -> None:
        assert process.stdout is not None
        for raw in process.stdout:
            lines.append(raw.rstrip())
            print(f"  [outrider] {raw.rstrip()}")

    def value(key: str) -> str | None:
        return next((line[len(key) + 1 :] for line in lines if line.startswith(key + " ")), None)

    threading.Thread(target=pump, daemon=True).start()
    deadline = time.time() + 180
    while time.time() < deadline and (value("LISTENING") is None or value("DESTINATION") is None):
        if process.poll() is not None:
            return 1
        time.sleep(0.1)
    if value("DESTINATION") is None:
        process.kill()
        return 1
    port = int(value("LISTENING"))
    outrider = bytes.fromhex(re.sub(r"[<>]", "", value("DESTINATION")))

    config = Path(tempfile.mkdtemp(prefix="outrider-tickets-"))
    (config / "config").write_text(
        "[reticulum]\n  enable_transport=No\n  share_instance=No\n"
        "  panic_on_interface_error=No\n\n[logging]\n  loglevel=5\n\n[interfaces]\n"
        "  [[outrider]]\n    type=TCPClientInterface\n    enabled=yes\n"
        f"    target_host=127.0.0.1\n    target_port={port}\n",
        encoding="utf-8",
    )

    exit_code = 1
    RNS.Reticulum(configdir=str(config))
    try:
        identity = RNS.Identity.from_bytes(SEED)
        router = LXMF.LXMRouter(identity=identity, storagepath=str(config), enforce_stamps=True)
        source = router.register_delivery_identity(identity, display_name="Stock Tickets", stamp_cost=8)
        sent: dict[str, LXMessage] = {}
        replies: list[LXMessage] = []
        replied = threading.Event()
        outbound: list[RNS.Destination] = []

        def on_delivery(message) -> None:
            replies.append(message)
            print(f"  stock: received {message.hash.hex()} stamp value {message.stamp_value}")
            replied.set()

        router.register_delivery_callback(on_delivery)

        class Announces:
            aspect_filter = "lxmf.delivery"

            def received_announce(self, destination_hash, announced_identity, app_data) -> None:
                if destination_hash != outrider or outbound:
                    return
                outbound.append(
                    RNS.Destination(announced_identity, RNS.Destination.OUT, RNS.Destination.SINGLE, "lxmf", "delivery")
                )

        RNS.Transport.register_announce_handler(Announces())
        source.announce()

        while time.time() < deadline and not outbound and process.poll() is None:
            time.sleep(0.1)
        if outbound:
            first = LXMessage(outbound[0], source, BULK, title="TICKET OFFER", include_ticket=True)
            router.handle_outbound(first)
            sent["first"] = first
            replied.wait(timeout=60)
        if replies:
            second = LXMessage(outbound[0], source, BULK, title="TICKET SPEND")
            router.handle_outbound(second)
            sent["second"] = second

        end = time.time() + 60
        while time.time() < end and process.poll() is None:
            if all(m.state == LXMessage.DELIVERED for m in sent.values()) and value("SECOND_STAMP"):
                break
            time.sleep(0.5)
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            process.kill()

        first, second = sent.get("first"), sent.get("second")
        for name, message in sent.items():
            print(f"  stock: {name} message state {message.state:#x}, stamp {(message.stamp or b'').hex()}")
        offered = first.fields.get(LXMF.FIELD_TICKET, [None, None])[1] if first else None
        learned_ok = (
            offered is not None
            and value("LEARNED") == offered.hex()
            and first.state == LXMessage.DELIVERED
        )
        reply = replies[0] if replies else None
        reply_ok = (
            reply is not None
            and reply.hash.hex() == value("REPLY_ID")
            and reply.stamp_valid
            and reply.stamp_value == LXMessage.COST_TICKET
            and len(reply.stamp) == LXMessage.TICKET_LENGTH
        )
        held = router.get_outbound_ticket(outrider)
        held_ok = held is not None and held.hex() == value("ISSUED")
        spent_ok = (
            second is not None
            and held is not None
            and second.stamp == RNS.Identity.truncated_hash(held + second.hash)
            and value("SECOND_ID") == second.hash.hex()
            and value("SECOND_STAMP") == "Ok(Ticket)"
            and second.state == LXMessage.DELIVERED
        )
        ok = learned_ok and reply_ok and held_ok and spent_ok
        print(f"stock's ticketed message delivered, Outrider learned the ticket: {'PASS' if learned_ok else 'FAIL'}")
        print(f"stock accepted the ticket-stamped reply at cost 8: {'PASS' if reply_ok else 'FAIL'}")
        print(f"stock holds Outrider's ticket: {'PASS' if held_ok else 'FAIL'}")
        print(f"stock spent it, Outrider accepted, stock saw DELIVERED: {'PASS' if spent_ok else 'FAIL'}")
        print(f"STOCK_OUTRIDER_TICKETS: {'PASS' if ok else 'FAIL'}")
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        atexit.register(shutil.rmtree, config, ignore_errors=True)
        RNS.exit(exit_code)


if __name__ == "__main__":
    raise SystemExit(main())
