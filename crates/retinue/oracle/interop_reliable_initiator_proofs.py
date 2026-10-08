"""Reliable-stream proof gate, Retinue as initiator: stock RNS sends, Retinue proves.

Retinue (`Endpoint::open_reliable`) opens a link to a stock RNS destination. The RNS
responder sends ten stream messages through the public `Channel.send`, waiting for
`Channel.is_ready_to_send` between them as a Buffer writer would, the last one carrying
EOF. Retinue proves every Channel packet it receives. RNS marks a packet's receipt
DELIVERED only when it validates that proof against the link initiator's key.

Must hold: RNS got all ten messages onto the wire; every one of their packet receipts
(`Envelope.packet.receipt`, the receipt of the packet's latest transmission) reaches
DELIVERED; Retinue read exactly the bytes and EOF; the Retinue process exits zero.

Without accepted proofs RNS's Channel window never reopens: RNS retransmits its first
messages, gives up after its retry limit and tears the link down, so fewer than ten
messages are sent and none is DELIVERED.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_reliable_initiator_proofs.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import threading
import time

from library_gate import Retinue, payload, start_rns, supervise, verdict

TITLE = "RELIABLE INITIATOR PROOFS INTEROP"
MESSAGES = 10
CHUNK = 400
LENGTH = MESSAGES * CHUNK
SEED = 0x7B3
STREAM_SEED = bytes([0x5B] * 64)  # RNS_STREAM_SEED in library_oracle.rs
PAYLOAD = payload(LENGTH, SEED)


def main() -> int:
    retinue = Retinue("stream-open", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS
        from RNS.Buffer import StreamDataMessage

        start_rns(port)
        finished = threading.Event()
        state: dict[str, object] = {"envelopes": []}
        keep: list[object] = []  # RNS objects must live until the verdict.
        identity = RNS.Identity.from_bytes(STREAM_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "reliable-sink")

        def receipt_status(envelope):
            receipt = getattr(envelope.packet, "receipt", None)
            return receipt.get_status() if receipt else None

        def link_established(link):
            if state.get("link"):
                return
            state["link"] = link
            channel = link.get_channel()
            # Registers the stream message type, so Retinue's EOF frame is understood.
            keep.append(RNS.Buffer.create_reader(0, channel))
            print("  RNS: inbound link up", flush=True)

            def send_all():
                envelopes = state["envelopes"]
                try:
                    deadline = time.monotonic() + 30
                    for index in range(MESSAGES):
                        while not channel.is_ready_to_send() and time.monotonic() < deadline:
                            time.sleep(0.02)
                        if not channel.is_ready_to_send():
                            print(f"  RNS: Channel never reopened after {index} messages", flush=True)
                            break
                        chunk = PAYLOAD[index * CHUNK:(index + 1) * CHUNK]
                        envelopes.append(channel.send(StreamDataMessage(
                            stream_id=0, data=chunk, eof=index == MESSAGES - 1,
                        )))
                    print(f"  RNS sent {len(envelopes)} of {MESSAGES} messages", flush=True)
                    deadline = time.monotonic() + 20
                    while time.monotonic() < deadline:
                        if all(receipt_status(e) == RNS.PacketReceipt.DELIVERED for e in envelopes):
                            break
                        time.sleep(0.05)
                    state["statuses"] = [receipt_status(e) for e in envelopes]
                except Exception as error:
                    state["error"] = repr(error)
                    state["statuses"] = [receipt_status(e) for e in envelopes]
                finally:
                    finished.set()

            threading.Thread(target=send_all, daemon=True).start()

        sink.set_link_established_callback(link_established)
        print(f"RNS stream sink {sink.hash.hex()}", flush=True)

        def announce_until_linked():
            while "link" not in state and retinue.proc.poll() is None:
                sink.announce()
                finished.wait(timeout=1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()
        print("waiting for Retinue's link...", flush=True)
        finished.wait(timeout=90)
        process_code = retinue.wait_exit(90)

        sent = len(state["envelopes"])
        statuses = state.get("statuses", [])
        names = {RNS.PacketReceipt.FAILED: "FAILED", RNS.PacketReceipt.SENT: "SENT",
                 RNS.PacketReceipt.DELIVERED: "DELIVERED", RNS.PacketReceipt.CULLED: "CULLED"}
        delivered = sum(1 for status in statuses if status == RNS.PacketReceipt.DELIVERED)
        print("\n" + "=" * 72)
        if state.get("error"):
            print(f"RNS send error: {state['error']}")
        print(f"RNS receipt statuses: {[names.get(s, s) for s in statuses]}")
        print(f"Link-data proofs Retinue sent: {retinue.tap('to_rns', 'Proof', RNS.Packet.NONE)}; "
              f"RNS Channel packets on the wire: {retinue.tap('to_retinue', 'Data', RNS.Packet.CHANNEL)}")
        ok = verdict(f"RNS sent all {MESSAGES} Channel messages", sent == MESSAGES, f"{sent} sent")
        ok &= verdict("every RNS packet receipt reached DELIVERED",
                      sent == MESSAGES and delivered == sent, f"{delivered} of {sent} DELIVERED")
        ok &= verdict("Retinue read the exact bytes and EOF",
                      retinue.find(rf"READ_EOF {LENGTH}") is not None and retinue.find("RECV_OK") is not None)
        ok &= verdict("Retinue process exited zero", process_code == 0, str(process_code))
        print("=" * 72)
        print(f"{TITLE}: {'PASS' if ok else 'FAIL'}", flush=True)
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        retinue.kill()
        import RNS

        RNS.exit(exit_code)


if __name__ == "__main__":
    if sys.argv[1:] == ["--peer"]:
        raise SystemExit(main())
    raise SystemExit(supervise(__file__, TITLE, 240))
