"""Link liveness gate: idle links both ways survive, then carry a 431-byte request.

Retinue (`library_oracle liveness`) opens a resource link to a stock RNS destination, and
RNS opens one to Retinue's. Both links are then left idle for HOLD seconds. RNS keeps a
link only while it hears from the peer: as initiator it expects answers to its keepalives,
as responder it expects the initiator's keepalives, both at intervals derived from the RTT
the initiator reports. After the hold, each side sends a request that packs to exactly
431 bytes, RNS's link MDU at MTU 500, and each answers by echoing the data.

Must hold: neither link closes during the hold (RNS's closed callbacks stay silent and both
links are ACTIVE when the requests go out); keepalives crossed the wire in both directions;
Retinue's 431-byte request is answered with its echo; RNS's 431-byte request reaches
Retinue as one packet and its echo comes back as one packet (`IN_RESPOND Data`); the
Retinue process exits zero.

Before link liveness, Retinue sent no keepalives, answered none and reported a fixed 0.05 s
RTT: RNS tore both links down within about half a minute. Before the 431-byte MDU, Retinue
refused its own request and answered RNS's with a Resource.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_link_liveness.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import threading
import time

from library_gate import Retinue, payload, start_rns, supervise, verdict

TITLE = "LINK LIVENESS INTEROP"
HOLD = 90
SEED = 0x7C4
LIVENESS_SEED = bytes([0x5C] * 64)  # RNS_LIVENESS_SEED in library_oracle.rs
RETINUE_SEED = bytes([0x47] * 64)  # IDENTITY_SEED in library_oracle.rs
LINK_MDU = 431
# fixarray(3) + float64 time (9) + bin8 path hash (18) + bin16 header (3) = 31 bytes.
RNS_DATA = payload(LINK_MDU - 31, SEED ^ 0x55)


def main() -> int:
    retinue = Retinue("liveness", HOLD, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS

        start_rns(port)
        state: dict[str, object] = {"closed": []}
        done = threading.Event()

        identity = RNS.Identity.from_bytes(LIVENESS_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "liveness-rns")

        def echo(path, data, request_id, link_id, remote_identity, requested_at):
            state["served"] = len(data) if data else 0
            return data

        sink.register_request_handler("/mdu", response_generator=echo,
                                      allow=RNS.Destination.ALLOW_ALL)

        def closed(role):
            def callback(link):
                state["closed"].append((role, time.monotonic(), link.teardown_reason))
                print(f"  RNS: {role} link closed, reason {link.teardown_reason}", flush=True)
            return callback

        def inbound_established(link):
            if "inbound" in state:
                return
            state["inbound"] = link
            link.set_link_closed_callback(closed("inbound"))
            print("  RNS: inbound link up", flush=True)

        sink.set_link_established_callback(inbound_established)

        def announce_until_linked():
            while "inbound" not in state and retinue.proc.poll() is None:
                sink.announce()
                done.wait(timeout=1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()

        retinue_hash = RNS.Destination.hash(RNS.Identity.from_bytes(RETINUE_SEED),
                                            "retinue", "liveness-retinue")
        deadline = time.monotonic() + 60
        while not RNS.Transport.has_path(retinue_hash) and time.monotonic() < deadline:
            time.sleep(0.2)
        if not RNS.Transport.has_path(retinue_hash):
            print("RNS never heard Retinue's announce", flush=True)
            return 1
        remote = RNS.Destination(RNS.Identity.recall(retinue_hash), RNS.Destination.OUT,
                                 RNS.Destination.SINGLE, "retinue", "liveness-retinue")
        established = threading.Event()
        outbound = RNS.Link(remote, established_callback=lambda link: established.set(),
                            closed_callback=closed("outbound"))
        if not established.wait(timeout=30):
            print("RNS outbound link never came up", flush=True)
            return 1
        print(f"  RNS: outbound link up, RTT {outbound.rtt:.4f} s", flush=True)
        deadline = time.monotonic() + 30
        while "inbound" not in state and time.monotonic() < deadline:
            time.sleep(0.1)
        inbound = state.get("inbound")
        if inbound is None:
            print("Retinue's link never reached RNS", flush=True)
            return 1
        time.sleep(1)  # the initiator's RTT packet activates the inbound link
        print(f"  RNS: inbound link RTT {inbound.rtt}, keepalive {inbound.keepalive:.1f} s; "
              f"outbound keepalive {outbound.keepalive:.1f} s", flush=True)

        print(f"holding both links idle for {HOLD} s...", flush=True)
        time.sleep(HOLD)
        statuses = (inbound.status, outbound.status)
        closed_during_hold = list(state["closed"])

        response: dict[str, object] = {}
        replied = threading.Event()

        def on_response(receipt):
            response["data"] = receipt.response
            replied.set()

        def on_failed(receipt):
            response["failed"] = True
            replied.set()

        receipt = outbound.request("/mdu", data=RNS_DATA, response_callback=on_response,
                                   failed_callback=on_failed, timeout=20)
        replied.wait(timeout=25)
        retinue.wait_for(r"OUT_REQUEST_\w+.*", 30)
        process_code = retinue.wait_exit(30)

        print("\n" + "=" * 72)
        print(f"RNS link statuses after the hold: {statuses}; closes during hold: "
              f"{closed_during_hold}")
        keepalives_in = retinue.tap("to_retinue", "Data", RNS.Packet.KEEPALIVE)
        keepalives_out = retinue.tap("to_rns", "Data", RNS.Packet.KEEPALIVE)
        print(f"keepalives to Retinue {keepalives_in}, to RNS {keepalives_out}; "
              f"RTT packets to RNS {retinue.tap('to_rns', 'Data', RNS.Packet.LRRTT)}")
        ok = verdict("no link closed during the hold", not closed_during_hold,
                     str(closed_during_hold))
        ok &= verdict("both RNS links ACTIVE after the hold",
                      statuses == (RNS.Link.ACTIVE, RNS.Link.ACTIVE), str(statuses))
        ok &= verdict("keepalives crossed in both directions",
                      keepalives_in > 0 and keepalives_out > 0,
                      f"{keepalives_in} in, {keepalives_out} out")
        ok &= verdict("Retinue held both links", retinue.find(r"HELD 2") is not None)
        ok &= verdict("Retinue's 431-byte request answered with its echo",
                      retinue.find(rf"OUT_REQUEST_OK {LINK_MDU}") is not None)
        ok &= verdict("RNS served a 400-byte request body",
                      state.get("served") == LINK_MDU - 31, str(state.get("served")))
        ok &= verdict("RNS's 431-byte request reached Retinue as one packet",
                      retinue.find(rf"IN_REQUEST {LINK_MDU}") is not None)
        ok &= verdict("Retinue answered in one packet", retinue.find(r"IN_RESPOND Data") is not None)
        ok &= verdict("RNS received the echo", response.get("data") == RNS_DATA,
                      "failed" if response.get("failed") else "")
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
