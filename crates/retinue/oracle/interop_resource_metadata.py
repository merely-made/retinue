"""Resource metadata gate, both directions: RNS's `Resource(data, metadata=...)`.

First stock RNS sends Retinue's Endpoint a Resource with metadata. RNS frames the packed
metadata in front of the data and sets the advertisement's metadata flag (0x20); Retinue
must hand back the data alone and the metadata separately, and prove the transfer so the
RNS sender concludes COMPLETE.

Then Retinue publishes a Resource with metadata to an RNS destination that accepts every
Resource. RNS must conclude COMPLETE with the exact data and the metadata Retinue sent as
`resource.metadata`, and Retinue's publish must return Ok on RNS's proof.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_resource_metadata.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import hashlib
import sys
import threading

from library_gate import RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "RESOURCE METADATA INTEROP"
LENGTH = 120_000
SEED = 0x3E7A
SINK_SEED = bytes([0x5A] * 64)  # RNS_SINK_SEED in examples/library_oracle/main.rs
PAYLOAD = payload(LENGTH, SEED)
RNS_METADATA = {"name": "rns.bin", "size": LENGTH, "tags": ["a", "b"]}
RETINUE_METADATA = {"name": "retinue.bin", "n": 7}  # RETINUE_METADATA in examples/library_oracle/resource.rs


def main() -> int:
    retinue = Retinue("resource-meta", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS
        from RNS.vendor import umsgpack

        start_rns(port)
        sent_concluded = threading.Event()
        received_concluded = threading.Event()
        state: dict[str, object] = {}

        # Phase 1: RNS sends to Retinue, with metadata.
        class Linker:
            aspect_filter = "retinue.library-resource"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if state.get("started"):
                    return
                state["started"] = True
                remote = RNS.Destination(
                    announced_identity, RNS.Destination.OUT,
                    RNS.Destination.SINGLE, "retinue", "library-resource",
                )
                link = RNS.Link(remote)
                state["send_link"] = link

                def established(established_link):
                    def done(resource):
                        state["sent_status"] = resource.status
                        print(f"  RNS sender concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}",
                              flush=True)
                        sent_concluded.set()

                    print(f"  RNS: link up, sending {LENGTH}-byte Resource with metadata", flush=True)
                    state["sent"] = RNS.Resource(PAYLOAD, established_link, metadata=RNS_METADATA,
                                                 callback=done)

                link.set_link_established_callback(established)

        RNS.Transport.register_announce_handler(Linker())
        print("phase 1: waiting for Retinue's announce, link and transfer...", flush=True)
        retinue.wait_for(r"METADATA .*|METADATA_NONE|RECEIVE_ERR .*|MODE_ERR .*", 90)
        sent_concluded.wait(timeout=20)
        link = state.get("send_link")
        if link is not None:
            link.teardown()

        # Phase 2: Retinue sends to RNS, with metadata.
        identity = RNS.Identity.from_bytes(SINK_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "library-sink")

        def resource_concluded(resource):
            state["received_status"] = resource.status
            try:
                data = resource.data.read() if hasattr(resource.data, "read") else bytes(resource.data or b"")
            except Exception as error:  # report, do not mask the verdict
                data = b""
                state["read_error"] = repr(error)
            state["received_data"] = data
            state["received_metadata"] = resource.metadata
            print(f"  RNS receiver concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}, "
                  f"{len(data)} bytes, metadata {resource.metadata!r}", flush=True)
            received_concluded.set()

        def link_established(link):
            state["receive_link"] = link
            link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
            link.set_resource_concluded_callback(resource_concluded)

        sink.set_link_established_callback(link_established)

        def announce_until_linked():
            while "receive_link" not in state and not received_concluded.is_set() and retinue.proc.poll() is None:
                sink.announce()
                received_concluded.wait(timeout=1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()
        print("phase 2: waiting for Retinue's link and Resource...", flush=True)
        received_concluded.wait(timeout=90)
        retinue.wait_for(r"PUBLISH_OK \d+|PUBLISH_ERR .*|MODE_ERR .*", 70)
        process_code = retinue.wait_exit(30)

        digest = retinue.find(r"RESOURCE (\d+) ([0-9a-f]{64})")
        bytes_ok = (
            retinue.find(r"RESOURCE_OK") is not None and digest is not None
            and int(digest.group(1)) == LENGTH
            and digest.group(2) == hashlib.sha256(PAYLOAD).hexdigest()
        )
        metadata_line = retinue.find(r"METADATA ([0-9a-f]+)")
        retinue_metadata = bytes.fromhex(metadata_line.group(1)) if metadata_line else None
        sent_status = state.get("sent_status")
        received_status = state.get("received_status")
        print("\n" + "=" * 72)
        if state.get("read_error"):
            print(f"RNS data read error: {state['read_error']}")
        ok = verdict("Retinue received RNS's data without the metadata", bytes_ok, f"{LENGTH} bytes")
        ok &= verdict("Retinue received RNS's metadata exactly",
                      retinue_metadata == umsgpack.packb(RNS_METADATA),
                      repr(umsgpack.unpackb(retinue_metadata)) if retinue_metadata else "none")
        ok &= verdict("RNS sender reached COMPLETE", sent_status == RNS.Resource.COMPLETE,
                      RESOURCE_STATUS.get(sent_status, str(sent_status)))
        ok &= verdict("RNS receiver reached COMPLETE", received_status == RNS.Resource.COMPLETE,
                      RESOURCE_STATUS.get(received_status, str(received_status)))
        ok &= verdict("RNS assembled Retinue's exact data", state.get("received_data") == PAYLOAD,
                      f"{len(state.get('received_data', b''))} of {LENGTH} bytes")
        ok &= verdict("RNS read Retinue's metadata", state.get("received_metadata") == RETINUE_METADATA,
                      repr(state.get("received_metadata")))
        ok &= verdict("Retinue publish returned after RNS's proof",
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
    raise SystemExit(supervise(__file__, TITLE, 240))
