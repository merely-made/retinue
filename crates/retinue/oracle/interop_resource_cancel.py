"""Resource reject and cancel gate: each side stops promptly when the other refuses.

RNS ends a transfer it will not finish with a sealed cancel naming the resource: RCL from
the receiver (`Resource.reject`, `Resource.cancel`), ICL from the initiator. Four
phases, each on its own link:

  1. Retinue's accept hook refuses an RNS offer. RNS's Resource must conclude REJECTED
     promptly, not time out.
  2. RNS cancels its own transfer to Retinue part-way. Retinue's receive must end with
     ConnectionAborted promptly, not at its 60 s timeout.
  3. An RNS destination whose ACCEPT_APP callback refuses every Resource rejects a Retinue
     publish. Retinue's publish must end with ConnectionAborted promptly.
  4. A Retinue publish to that destination, now accepting, times out mid-transfer (RNS
     holds its first part request back past the timeout). Retinue's cancel must end RNS's
     receiving Resource as FAILED promptly, not after RNS's own retries run out.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_resource_cancel.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import threading
import time

from library_gate import RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "RESOURCE REJECT AND CANCEL INTEROP"
LENGTH = 600_000
SEED = 0xCA2CE1
SINK_SEED = bytes([0x5A] * 64)  # RNS_SINK_SEED in library_oracle.rs
PAYLOAD = payload(LENGTH, SEED)
PROMPT = 5.0  # seconds; every timeout in play is 60 s
STALL = 1.5  # seconds RNS holds its first part request in phase 4; Retinue gives up at 0.4


def main() -> int:
    retinue = Retinue("resource-cancel", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS

        start_rns(port)
        state: dict[str, object] = {}
        linked = threading.Event()
        want_link = threading.Event()

        class Linker:
            aspect_filter = "retinue.library-resource"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if not want_link.is_set():
                    return
                want_link.clear()
                remote = RNS.Destination(
                    announced_identity, RNS.Destination.OUT,
                    RNS.Destination.SINGLE, "retinue", "library-resource",
                )
                link = RNS.Link(remote)
                state["link"] = link
                link.set_link_established_callback(lambda _: linked.set())

        RNS.Transport.register_announce_handler(Linker())

        def open_link() -> object:
            linked.clear()
            want_link.set()
            if not linked.wait(timeout=60):
                raise RuntimeError("no link to Retinue")
            return state["link"]

        # Phase 1: Retinue rejects RNS's offer.
        print("phase 1: RNS offers, Retinue's accept hook refuses", flush=True)
        link = open_link()
        rejected = threading.Event()
        offered_at = time.monotonic()

        def on_reject(resource):
            state["reject_status"] = resource.status
            state["reject_after"] = time.monotonic() - offered_at
            rejected.set()

        offer = RNS.Resource(PAYLOAD, link, callback=on_reject)
        rejected.wait(timeout=30)
        retinue.wait_for(r"REJECTED_OFFER .*|REJECT_UNEXPECTED_PAYLOAD", 10)
        print(f"  RNS offer concluded {RESOURCE_STATUS.get(offer.status, offer.status)}", flush=True)
        link.teardown()
        retinue.wait_for(r"PEER_CLOSED|HOLD_END .*", 10)

        # Phase 2: RNS cancels part-way.
        print("phase 2: RNS sends, then cancels part-way", flush=True)
        link = open_link()
        canceled_at: dict[str, float] = {}

        def on_progress(resource):
            if "at" not in canceled_at and resource.get_progress() > 0.05:
                canceled_at["at"] = time.monotonic()
                canceled_at["progress"] = resource.get_progress()
                threading.Thread(target=resource.cancel, daemon=True).start()

        sending = RNS.Resource(PAYLOAD, link, progress_callback=on_progress)
        stopped = retinue.wait_for(r"SENDER_CANCELED .*|CANCEL_UNEXPECTED_PAYLOAD", 60)
        stopped_after = time.monotonic() - canceled_at["at"] if stopped and "at" in canceled_at else None
        print(f"  RNS sender {RESOURCE_STATUS.get(sending.status, sending.status)} at progress "
              f"{canceled_at.get('progress', 0):.2f}", flush=True)
        link.teardown()
        retinue.wait_for(r"PEER_CLOSED|HOLD_END .*", 10)

        # Phase 3: RNS rejects Retinue's publish.
        print("phase 3: Retinue publishes, RNS's accept callback refuses", flush=True)
        identity = RNS.Identity.from_bytes(SINK_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "library-sink")
        advertised: list[int] = []

        def refuse(advertisement):
            advertised.append(advertisement.get_data_size())
            return False

        receiver_failed = threading.Event()

        def receiver_concluded(resource):
            state["receive_status"] = resource.status
            state["receive_concluded_at"] = time.monotonic()
            receiver_failed.set()

        def sink_link(incoming):
            if "sink_link" not in state:
                state["sink_link"] = incoming
                incoming.set_resource_strategy(RNS.Link.ACCEPT_APP)
                incoming.set_resource_callback(refuse)
            else:
                incoming.set_resource_strategy(RNS.Link.ACCEPT_ALL)
                incoming.set_resource_concluded_callback(receiver_concluded)
                # Hold the first part request back past Retinue's publish timeout, so it
                # gives up with the transfer accepted and nothing yet moved.
                incoming.set_resource_started_callback(lambda _: time.sleep(STALL))

        sink.set_link_established_callback(sink_link)

        def announce_until_linked():
            while "sink_link" not in state and retinue.proc.poll() is None:
                sink.announce()
                time.sleep(1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()
        publish = retinue.wait_for(r"PUBLISH_REJECTED (\w+) (\d+)|PUBLISH_UNEXPECTED_OK|MODE_ERR .*", 90)

        # Phase 4: Retinue's publish times out part-way.
        print("phase 4: Retinue publishes and gives up part-way", flush=True)
        gave_up = retinue.wait_for(r"PUBLISH_GAVE_UP (\w+)|PUBLISH_UNEXPECTED_OK|MODE_ERR .*", 60)
        gave_up_at = time.monotonic()
        receiver_failed.wait(timeout=10)
        failed_after = state.get("receive_concluded_at", gave_up_at + 99) - gave_up_at
        process_code = retinue.wait_exit(30)

        reject_after = state.get("reject_after")
        rejected_line = retinue.find(r"REJECTED_OFFER (\w+)")
        offer_line = retinue.find(r"OFFER (\d+) (\d+) (\d+)")
        canceled_line = retinue.find(r"SENDER_CANCELED (\w+) (\d+)")
        print("\n" + "=" * 72)
        ok = verdict("Retinue's hook saw RNS's advertisement",
                     offer_line is not None and int(offer_line.group(1)) == LENGTH,
                     offer_line.group(0) if offer_line else "none")
        ok &= verdict("RNS's offer concluded REJECTED promptly",
                      state.get("reject_status") == RNS.Resource.REJECTED
                      and reject_after is not None and reject_after < PROMPT,
                      f"{RESOURCE_STATUS.get(state.get('reject_status'), state.get('reject_status'))} "
                      f"after {reject_after if reject_after is None else round(reject_after, 2)}s")
        ok &= verdict("Retinue's receive ended as refused",
                      rejected_line is not None and rejected_line.group(1) == "PermissionDenied",
                      rejected_line.group(0) if rejected_line else "none")
        ok &= verdict("Retinue's receive ended on RNS's cancel promptly",
                      canceled_line is not None and canceled_line.group(1) == "ConnectionAborted"
                      and stopped_after is not None and stopped_after < PROMPT,
                      f"{canceled_line.group(0) if canceled_line else 'none'}, "
                      f"{stopped_after if stopped_after is None else round(stopped_after, 2)}s after the cancel")
        ok &= verdict("RNS's accept callback saw Retinue's offer", advertised == [LENGTH], repr(advertised))
        ok &= verdict("Retinue's publish ended on RNS's rejection promptly",
                      publish is not None and publish.lastindex == 2
                      and publish.group(1) == "ConnectionAborted"
                      and int(publish.group(2)) < PROMPT * 1000,
                      publish.group(0) if publish else "none")
        ok &= verdict("Retinue's publish gave up part-way",
                      gave_up is not None and gave_up.lastindex == 1 and gave_up.group(1) == "TimedOut",
                      gave_up.group(0) if gave_up else "none")
        ok &= verdict("RNS's receiving Resource failed on Retinue's cancel promptly",
                      state.get("receive_status") == RNS.Resource.FAILED and failed_after < PROMPT,
                      f"{RESOURCE_STATUS.get(state.get('receive_status'), state.get('receive_status'))} "
                      f"{round(failed_after, 2)}s after Retinue gave up")
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
    raise SystemExit(supervise(__file__, TITLE, 300))
