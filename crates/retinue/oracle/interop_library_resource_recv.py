"""Library-path Resource receive gate: stock RNS sends, Retinue's Endpoint receives.

Unlike interop_resource_recv.py, which drives the resource state machine from its own
example, the Retinue side here is `Endpoint::accept_resource` plus
`ResourceSession::receive`, the path applications use. RNS sends a ~300 kB
incompressible Resource (hundreds of parts: several RNS windows and several hashmap
segments, below the 1 MiB single-segment limit) with its default settings.

Two things must hold:
  * Retinue's library returns exactly the bytes RNS sent; and
  * the RNS sender's own Resource reaches COMPLETE, which it does only after it accepts
    the proof Retinue's library sent back.
The second is the half the older gate could not see. The sender gets a bounded wait
after Retinue has the bytes; a proof RNS cannot accept leaves it at AWAITING_PROOF.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_library_resource_recv.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import hashlib
import sys
import threading
import time

from library_gate import RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "LIBRARY RESOURCE RECEIVE INTEROP"
LENGTH = 300_000
SEED = 0x1D1C0DE
PROOF_GRACE = 20.0  # seconds the RNS sender gets to accept the proof after Retinue has the bytes
PAYLOAD = payload(LENGTH, SEED)


def main() -> int:
    retinue = Retinue("resource-recv", LENGTH, SEED)
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
                        print(f"  RNS sender concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}",
                              flush=True)
                        concluded.set()

                    print(f"  RNS: link up, sending {LENGTH}-byte Resource", flush=True)
                    state["resource"] = RNS.Resource(PAYLOAD, established_link, callback=done)

                link.set_link_established_callback(established)

        RNS.Transport.register_announce_handler(Linker())
        print("waiting for Retinue's announce, link and transfer...", flush=True)
        got = retinue.wait_for(r"RESOURCE_OK|RESOURCE_MISMATCH|UNEXPECTED_DATA \d+|RECEIVE_ERR .*|MODE_ERR .*", 90)
        received_at = time.monotonic()
        if got is not None:
            concluded.wait(timeout=PROOF_GRACE)
        resource = state.get("resource")
        sender_status = getattr(resource, "status", None)
        waited = time.monotonic() - received_at
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
        complete = sender_status == RNS.Resource.COMPLETE
        proof_as_proof = retinue.tap("to_rns", "Proof", RNS.Packet.RESOURCE_PRF)
        proof_as_data = retinue.tap("to_rns", "Data", RNS.Packet.RESOURCE_PRF)
        print("\n" + "=" * 72)
        print(f"Resource proof packets Retinue sent: PROOF-type {proof_as_proof}, DATA-type {proof_as_data}")
        ok = verdict("Retinue library received the exact bytes", bytes_ok, f"{LENGTH} bytes")
        ok &= verdict("RNS sender's Resource reached COMPLETE", complete,
                      f"status {status_name} after {waited:.1f}s")
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
    raise SystemExit(supervise(__file__, TITLE, 180))
