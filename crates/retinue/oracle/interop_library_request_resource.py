"""Requests sent as Resources: a 2 KiB request each way with stock RNS.

A request whose packed form exceeds the link MDU goes as a Resource flagged as a request
(`u`, 0x08), its `q` the truncated hash of the packed request, which is also the request
id the response names (Link.py 473-510, 885-895, 1036-1043). Each side here sends a
2 KiB request to a handler that echoes it, so the response travels as a Resource too.

Retinue's side is the public Endpoint API (`library_oracle request-resource`):
`ResourceSession::request` on a link to RNS, and `receive_raw_request` plus
`respond_auto` on a link from RNS.

Must hold: RNS's handler receives Retinue's request whole and Retinue gets the echo;
Retinue receives RNS's request whole, under the id RNS computed, and RNS gets the echo;
both requests crossed as Resource advertisements. Retinue's process exits zero.

Before this, Retinue refused to send such a request and ignored RNS's.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_library_request_resource.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import threading
import time

from library_gate import Retinue, payload, start_rns, supervise, verdict

TITLE = "LIBRARY REQUEST RESOURCE INTEROP"
LENGTH = 2048
SEED = 0x2B16
REQUEST_SEED = bytes([0x5D] * 64)  # RNS_BIG_REQUEST_SEED in examples/library_oracle/main.rs
RETINUE_SEED = bytes([0x47] * 64)  # IDENTITY_SEED in examples/library_oracle/main.rs
RETINUE_DATA = payload(LENGTH, SEED)
RNS_DATA = payload(LENGTH, SEED ^ 0x55)


def main() -> int:
    retinue = Retinue("request-resource", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS

        start_rns(port)
        state: dict[str, object] = {}
        done = threading.Event()

        identity = RNS.Identity.from_bytes(REQUEST_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "bigreq-rns")

        def echo(path, data, request_id, link_id, remote_identity, requested_at):
            state["served"] = data
            state["served_id"] = request_id
            return data

        sink.register_request_handler("/echo", response_generator=echo,
                                      allow=RNS.Destination.ALLOW_ALL)

        def inbound_established(link):
            state.setdefault("inbound", link)
            print("  RNS: inbound link up", flush=True)

        sink.set_link_established_callback(inbound_established)

        def announce_until_linked():
            while "inbound" not in state and retinue.proc.poll() is None:
                sink.announce()
                done.wait(timeout=1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()

        retinue_hash = RNS.Destination.hash(RNS.Identity.from_bytes(RETINUE_SEED),
                                            "retinue", "bigreq-retinue")
        deadline = time.monotonic() + 60
        while not RNS.Transport.has_path(retinue_hash) and time.monotonic() < deadline:
            time.sleep(0.2)
        if not RNS.Transport.has_path(retinue_hash):
            print("RNS never heard Retinue's announce", flush=True)
            return 1
        remote = RNS.Destination(RNS.Identity.recall(retinue_hash), RNS.Destination.OUT,
                                 RNS.Destination.SINGLE, "retinue", "bigreq-retinue")
        established = threading.Event()
        outbound = RNS.Link(remote, established_callback=lambda link: established.set())
        if not established.wait(timeout=30):
            print("RNS outbound link never came up", flush=True)
            return 1
        time.sleep(1)  # the RTT packet activates Retinue's side

        response: dict[str, object] = {}
        replied = threading.Event()

        def on_response(receipt):
            response["data"] = receipt.response
            replied.set()

        def on_failed(receipt):
            response["failed"] = True
            replied.set()

        receipt = outbound.request("/echo", data=RNS_DATA, response_callback=on_response,
                                   failed_callback=on_failed, timeout=40)
        rns_request_id = receipt.request_id.hex() if receipt else None
        replied.wait(timeout=45)
        retinue.wait_for(r"OUT_REQUEST_\w+.*", 60)
        process_code = retinue.wait_exit(30)

        adverts_in = retinue.tap("to_retinue", "Data", RNS.Packet.RESOURCE_ADV)
        adverts_out = retinue.tap("to_rns", "Data", RNS.Packet.RESOURCE_ADV)
        in_request = retinue.find(r"IN_REQUEST (\d+) ([0-9a-f]{32})")
        out_id = retinue.find(r"OUT_REQUEST_ID ([0-9a-f]{32})")
        print("\n" + "=" * 72)
        print(f"advertisements: to Retinue {adverts_in}, to RNS {adverts_out}")
        ok = verdict("RNS served Retinue's 2 KiB request whole", state.get("served") == RETINUE_DATA)
        ok &= verdict("RNS named Retinue's request by its packed hash",
                      out_id is not None and state.get("served_id", b"").hex() == out_id.group(1))
        ok &= verdict("Retinue received the echo", retinue.find(r"OUT_REQUEST_OK \d+") is not None)
        ok &= verdict("Retinue received RNS's request under RNS's id",
                      in_request is not None and in_request.group(2) == rns_request_id,
                      f"{in_request.group(2) if in_request else None} vs {rns_request_id}")
        ok &= verdict("Retinue answered with a Resource", retinue.find(r"IN_RESPOND Resource") is not None)
        ok &= verdict("RNS received the echo", response.get("data") == RNS_DATA,
                      "failed" if response.get("failed") else "")
        ok &= verdict("both requests and both responses went as Resources",
                      adverts_in >= 2 and adverts_out >= 2, f"{adverts_in} in, {adverts_out} out")
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
