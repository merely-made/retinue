"""Library-path multi-segment Resource gate: 2.5 MiB each way with stock RNS.

RNS splits a Resource past MAX_EFFICIENT_SIZE (1 MiB - 1) into segments that share the
first segment's hash, advertising each once the previous one is proved (Resource.py
274-339, 793-835). Retinue's side is the public Endpoint API (`library_oracle
resource-segments`): first `ResourceSession::receive` takes a 2.5 MiB Resource RNS sends,
then `ResourceSession::publish` sends the same bytes to an RNS destination that accepts
every Resource.

Must hold, in each direction: the receiver has exactly the bytes sent, delivered once at
the last segment; the sender completes, which takes every segment's proof; every
segment's advertisement crossed (three each way). Retinue's process exits zero.

Before multi-segment support, Retinue refused RNS's split offer with a cancel and sent a
2.5 MiB payload as one oversized resource.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_library_resource_segments.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import hashlib
import sys
import threading
import time

from library_gate import RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "LIBRARY RESOURCE SEGMENTS INTEROP"
LENGTH = 5 * 1024 * 1024 // 2
SEED = 0x5E6
SEGMENTS = 3
SINK_SEED = bytes([0x5A] * 64)  # RNS_SINK_SEED in examples/library_oracle/main.rs
PROOF_GRACE = 30.0
PAYLOAD = payload(LENGTH, SEED)


def main() -> int:
    retinue = Retinue("resource-segments", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS

        start_rns(port)
        state: dict[str, object] = {}

        # RNS sends to Retinue.
        sent = threading.Event()

        class Linker:
            aspect_filter = "retinue.library-resource"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if state.get("started"):
                    return
                state["started"] = True
                remote = RNS.Destination(announced_identity, RNS.Destination.OUT,
                                         RNS.Destination.SINGLE, "retinue", "library-resource")
                link = RNS.Link(remote)
                state["out_link"] = link

                def established(established_link):
                    def done(resource):
                        state["sender_status"] = resource.status
                        print(f"  RNS sender concluded: "
                              f"{RESOURCE_STATUS.get(resource.status, resource.status)}", flush=True)
                        sent.set()

                    print(f"  RNS: link up, sending {LENGTH}-byte Resource", flush=True)
                    started = time.monotonic()
                    state["sent_at"] = started
                    RNS.Resource(PAYLOAD, established_link, callback=done)

                link.set_link_established_callback(established)

        RNS.Transport.register_announce_handler(Linker())
        print("waiting for Retinue's announce, link and transfer...", flush=True)
        got = retinue.wait_for(r"RESOURCE_OK|RESOURCE_MISMATCH|UNEXPECTED_DATA \d+|RECEIVE_ERR .*|MODE_ERR .*",
                               240)
        if got is not None:
            sent.wait(timeout=PROOF_GRACE)
        if "sent_at" in state:
            print(f"  RNS -> Retinue took {time.monotonic() - state['sent_at']:.1f}s", flush=True)
        link = state.get("out_link")
        if link is not None:
            link.teardown()

        # Retinue sends to RNS.
        received = threading.Event()
        identity = RNS.Identity.from_bytes(SINK_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "library-sink")

        def resource_concluded(resource):
            state["recv_status"] = resource.status
            state["recv_segments"] = (resource.segment_index, resource.total_segments)
            try:
                data = resource.data.read() if hasattr(resource.data, "read") else bytes(resource.data or b"")
            except Exception as error:  # report, do not mask the verdict
                data = b""
                state["read_error"] = repr(error)
            state["data"] = data
            print(f"  RNS receiver concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}, "
                  f"segment {resource.segment_index}/{resource.total_segments}, {len(data)} bytes",
                  flush=True)
            received.set()

        def link_established(in_link):
            state["in_link"] = in_link
            in_link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
            in_link.set_resource_concluded_callback(resource_concluded)
            print("  RNS: inbound link up, accepting all Resources", flush=True)

        sink.set_link_established_callback(link_established)

        def announce_until_linked():
            while "in_link" not in state and not received.is_set() and retinue.proc.poll() is None:
                sink.announce()
                received.wait(timeout=1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()
        received.wait(timeout=240)
        retinue.wait_for(r"PUBLISH_OK \d+|PUBLISH_ERR .*|MODE_ERR .*", 60)
        process_code = retinue.wait_exit(30)

        digest = retinue.find(r"RESOURCE (\d+) ([0-9a-f]{64})")
        recv_ok = (
            retinue.find(r"RESOURCE_OK") is not None and digest is not None
            and int(digest.group(1)) == LENGTH
            and digest.group(2) == hashlib.sha256(PAYLOAD).hexdigest()
        )
        adverts_in = retinue.tap("to_retinue", "Data", RNS.Packet.RESOURCE_ADV)
        adverts_out = retinue.tap("to_rns", "Data", RNS.Packet.RESOURCE_ADV)
        data = state.get("data", b"")
        print("\n" + "=" * 72)
        if state.get("read_error"):
            print(f"RNS data read error: {state['read_error']}")
        print(f"advertisements: to Retinue {adverts_in}, to RNS {adverts_out}")
        ok = verdict("Retinue received RNS's split Resource whole", recv_ok, f"{LENGTH} bytes")
        ok &= verdict("RNS sender completed every segment",
                      state.get("sender_status") == RNS.Resource.COMPLETE,
                      RESOURCE_STATUS.get(state.get("sender_status"), str(state.get("sender_status"))))
        ok &= verdict("RNS sent one advertisement per segment", adverts_in >= SEGMENTS, str(adverts_in))
        ok &= verdict("RNS receiver concluded COMPLETE at the last segment",
                      state.get("recv_status") == RNS.Resource.COMPLETE
                      and state.get("recv_segments") == (SEGMENTS, SEGMENTS),
                      str(state.get("recv_segments")))
        ok &= verdict("RNS assembled the exact bytes", data == PAYLOAD, f"{len(data)} of {LENGTH} bytes")
        ok &= verdict("Retinue sent one advertisement per segment", adverts_out >= SEGMENTS, str(adverts_out))
        ok &= verdict("Retinue publish returned after the last proof",
                      retinue.find(rf"PUBLISH_OK {LENGTH}") is not None)
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
    raise SystemExit(supervise(__file__, TITLE, 600))
