"""Resource proof recovery gate: a lost proof is asked for again with a cache request.

Stock RNS sends a Resource to Retinue's Endpoint, as in interop_library_resource_recv.py,
but the relay in front of Retinue drops every resource proof Retinue sends (the one sent at
completion and the copies queued behind it) until RNS sends a cache request. An RNS
sender that has sent every part and heard no proof sends a CACHE_REQUEST naming the full
hash of the proof it expects (`Resource.py` watchdog, AWAITING_PROOF); Retinue keeps the
proof it sent per link and answers the request with it.

Three things must hold:
  * Retinue's library returns exactly the bytes RNS sent;
  * the relay did drop a proof, and RNS did send a cache request; and
  * the RNS sender's Resource still reaches COMPLETE.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_resource_proof_cache.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import hashlib
import sys
import threading
import time

from library_gate import RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "RESOURCE PROOF CACHE INTEROP"
LENGTH = 40_000
SEED = 0xCAC4E
PROOF_GRACE = 45.0  # RNS waits about three round trips plus 10 s before each cache request
PAYLOAD = payload(LENGTH, SEED)
CACHE_REQUEST = 0x08


def main() -> int:
    retinue = Retinue("resource-recv-drop-proof", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS

        start_rns(port)
        concluded = threading.Event()
        state: dict[str, object] = {}

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
                state["link"] = link

                def established(established_link):
                    def done(resource):
                        state["callback_status"] = resource.status
                        state["concluded_at"] = time.monotonic()
                        print(f"  RNS sender concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}",
                              flush=True)
                        concluded.set()

                    print(f"  RNS: link up, sending {LENGTH}-byte Resource", flush=True)
                    state["resource"] = RNS.Resource(PAYLOAD, established_link, callback=done)

                link.set_link_established_callback(established)

        # Count the cache requests RNS puts on the wire.
        sent_cache_requests = []
        original_cache_request = RNS.Transport.cache_request

        def counting_cache_request(packet_hash, destination):
            sent_cache_requests.append(packet_hash)
            print(f"  RNS: cache request for {packet_hash.hex()}", flush=True)
            return original_cache_request(packet_hash, destination)

        RNS.Transport.cache_request = staticmethod(counting_cache_request)

        RNS.Transport.register_announce_handler(Linker())
        print("waiting for Retinue's announce, link and transfer...", flush=True)
        got = retinue.wait_for(r"RESOURCE_OK|RESOURCE_MISMATCH|UNEXPECTED_DATA \d+|RECEIVE_ERR .*|MODE_ERR .*", 90)
        received_at = time.monotonic()
        if got is not None:
            concluded.wait(timeout=PROOF_GRACE)
        resource = state.get("resource")
        sender_status = getattr(resource, "status", None)
        waited = state.get("concluded_at", time.monotonic()) - received_at
        link = state.get("link")
        if link is not None:
            link.teardown()
        process_code = retinue.wait_exit(20)

        digest = retinue.find(r"RESOURCE (\d+) ([0-9a-f]{64})")
        bytes_ok = (
            retinue.find(r"RESOURCE_OK") is not None and digest is not None
            and int(digest.group(1)) == LENGTH
            and digest.group(2) == hashlib.sha256(PAYLOAD).hexdigest()
        )
        status_name = RESOURCE_STATUS.get(sender_status, str(sender_status))
        dropped = retinue.tap("to_rns", "Dropped", RNS.Packet.RESOURCE_PRF)
        proofs = retinue.tap("to_rns", "Proof", RNS.Packet.RESOURCE_PRF)
        requests_seen = retinue.tap("to_retinue", "Data", CACHE_REQUEST)
        print("\n" + "=" * 72)
        print(f"Proofs dropped {dropped}, delivered {proofs}; cache requests RNS sent "
              f"{len(sent_cache_requests)}, Retinue heard {requests_seen}")
        ok = verdict("Retinue library received the exact bytes", bytes_ok, f"{LENGTH} bytes")
        ok &= verdict("the relay dropped the proofs sent at completion", dropped >= 1, str(dropped))
        ok &= verdict("RNS asked for the proof with a cache request",
                      len(sent_cache_requests) >= 1 and requests_seen >= 1,
                      f"{len(sent_cache_requests)} sent, {requests_seen} heard")
        ok &= verdict("Retinue answered with the proof", proofs >= 1, str(proofs))
        ok &= verdict("RNS sender's Resource reached COMPLETE", sender_status == RNS.Resource.COMPLETE,
                      f"status {status_name} {waited:.1f}s after Retinue had the bytes")
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
