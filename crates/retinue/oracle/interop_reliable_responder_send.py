"""Reliable-stream proof gate, Retinue as responder: Retinue sends, stock RNS reads.

A stock RNS initiator opens a link to Retinue's reliable destination and reads through
an RNS Buffer, as an ordinary RNS application would; it does not IDENTIFY. Retinue
(`Endpoint::accept_reliable`) writes 8 KiB of incompressible data and EOF. RNS proves
each Channel packet it receives under the link, signed with the initiator's link key.

Must hold: RNS reads exactly the 8 KiB and EOF; after RNS closes its own side, Retinue's
endpoint retires the link, which it does only once every packet it sent was proved by a
proof it accepted; the Retinue process exits zero.

Reported, not asserted: Retinue's retransmit count, from the example's wire tap. It is the
number of Channel packets Retinue put on the wire minus the distinct Channel messages RNS
delivered.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_reliable_responder_send.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import threading
import time

from library_gate import Retinue, payload, start_rns, supervise, verdict

TITLE = "RELIABLE RESPONDER SEND PROOFS INTEROP"
LENGTH = 8192
SEED = 0x3A0F
PAYLOAD = payload(LENGTH, SEED)


def main() -> int:
    retinue = Retinue("stream-respond", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS
        from RNS.Buffer import StreamDataMessage

        start_rns(port)
        read_done = threading.Event()
        state: dict[str, object] = {"messages": 0}
        keep: list[object] = []  # RNS Link and BufferedWriter must live until the verdict.

        class Linker:
            aspect_filter = "retinue.reliable-proofs"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if state.get("started"):
                    return
                state["started"] = True
                remote = RNS.Destination(
                    announced_identity, RNS.Destination.OUT,
                    RNS.Destination.SINGLE, "retinue", "reliable-proofs",
                )
                link = RNS.Link(remote)
                keep.append(link)

                def established(established_link):
                    channel = established_link.get_channel()

                    def count(message):
                        # Runs before the Buffer reader's handler; never consumes.
                        if isinstance(message, StreamDataMessage):
                            state["messages"] += 1
                        return False

                    channel.add_message_handler(count)
                    reader = RNS.Buffer.create_reader(0, channel)
                    writer = RNS.Buffer.create_writer(0, channel)
                    keep.append(writer)

                    def read_all():
                        received = bytearray()
                        eof = False
                        deadline = time.monotonic() + 45
                        try:
                            while time.monotonic() < deadline:
                                chunk = reader.read(4096)
                                if chunk is None:
                                    time.sleep(0.05)
                                    continue
                                if chunk == b"":
                                    eof = True
                                    break
                                received.extend(chunk)
                            state["received"] = bytes(received)
                            state["eof"] = eof
                            print(f"  RNS read {len(received)} bytes, EOF={eof}", flush=True)
                            # Our side ends too, so Retinue's stream can finish both halves.
                            writer.close()
                        except Exception as error:
                            state["error"] = repr(error)
                        finally:
                            read_done.set()

                    threading.Thread(target=read_all, daemon=True).start()

                link.set_link_established_callback(established)

        RNS.Transport.register_announce_handler(Linker())
        print("waiting for Retinue's announce, link and stream...", flush=True)
        read_done.wait(timeout=75)
        process_code = retinue.wait_exit(75)

        channel_packets = retinue.tap("to_rns", "Data", RNS.Packet.CHANNEL)
        delivered = int(state["messages"])
        rns_proofs = retinue.tap("to_retinue", "Proof", RNS.Packet.NONE)
        print("\n" + "=" * 72)
        if state.get("error"):
            print(f"RNS read error: {state['error']}")
        print(f"Retinue Channel packets on the wire: {channel_packets}; distinct messages RNS delivered: "
              f"{delivered}; Retinue retransmits: {max(channel_packets - delivered, 0)}")
        print(f"Link-data proofs RNS sent to Retinue: {rns_proofs}")
        ok = verdict("RNS read the exact 8 KiB and EOF",
                     state.get("received") == PAYLOAD and state.get("eof") is True,
                     f"{len(state.get('received', b''))} bytes, EOF={state.get('eof')}")
        ok &= verdict("Retinue read RNS's EOF", retinue.find(r"READ_EOF 0") is not None)
        ok &= verdict("Retinue accepted RNS's proofs and retired the link",
                      retinue.find(r"STREAM_IDLE") is not None)
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
