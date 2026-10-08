"""Library-path Resource send gate: Retinue's Endpoint publishes, stock RNS receives.

Unlike interop_resource_send.py, which hand-builds the advertisement and serves parts
from its own example, the Retinue side here is `Endpoint::open_resource` plus
`ResourceSession::publish`, the path applications use. Retinue publishes a ~300 kB
incompressible payload (hundreds of parts: several RNS request windows and several
hashmap updates, below the 1 MiB single-segment limit) to an RNS destination that
accepts every Resource.

Three things must hold: RNS's receiving Resource concludes COMPLETE, the bytes RNS
assembled are exactly the published bytes, and Retinue's `publish` returns Ok, which it
does only after it accepts the proof RNS sends back.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_library_resource_send.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import sys
import threading

from library_gate import RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "LIBRARY RESOURCE SEND INTEROP"
LENGTH = 300_000
SEED = 0x5E2D
SINK_SEED = bytes([0x5A] * 64)  # RNS_SINK_SEED in library_oracle.rs
PAYLOAD = payload(LENGTH, SEED)


def main() -> int:
    retinue = Retinue("resource-send", LENGTH, SEED)
    exit_code = 1
    try:
        port = retinue.wait_port()
        import RNS

        start_rns(port)
        concluded = threading.Event()
        state: dict[str, object] = {}
        identity = RNS.Identity.from_bytes(SINK_SEED)
        sink = RNS.Destination(identity, RNS.Destination.IN, RNS.Destination.SINGLE,
                               "retinue", "library-sink")

        def resource_concluded(resource):
            state["status"] = resource.status
            try:
                data = resource.data.read() if hasattr(resource.data, "read") else bytes(resource.data or b"")
            except Exception as error:  # report, do not mask the verdict
                data = b""
                state["read_error"] = repr(error)
            state["data"] = data
            print(f"  RNS receiver concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}, "
                  f"{len(data)} bytes", flush=True)
            concluded.set()

        def link_established(link):
            state["link"] = link
            link.set_resource_strategy(RNS.Link.ACCEPT_ALL)
            link.set_resource_concluded_callback(resource_concluded)
            print("  RNS: inbound link up, accepting all Resources", flush=True)

        sink.set_link_established_callback(link_established)
        print(f"RNS sink {sink.hash.hex()}", flush=True)

        def announce_until_linked():
            while "link" not in state and not concluded.is_set() and retinue.proc.poll() is None:
                sink.announce()
                concluded.wait(timeout=1.0)

        threading.Thread(target=announce_until_linked, daemon=True).start()
        print("waiting for Retinue's link and Resource...", flush=True)
        concluded.wait(timeout=90)
        retinue.wait_for(r"PUBLISH_OK \d+|PUBLISH_ERR .*|MODE_ERR .*", 70)
        process_code = retinue.wait_exit(30)

        status = state.get("status")
        data = state.get("data", b"")
        print("\n" + "=" * 72)
        if state.get("read_error"):
            print(f"RNS data read error: {state['read_error']}")
        ok = verdict("RNS receiver's Resource reached COMPLETE", status == RNS.Resource.COMPLETE,
                     RESOURCE_STATUS.get(status, str(status)))
        ok &= verdict("RNS assembled the exact bytes", data == PAYLOAD, f"{len(data)} of {LENGTH} bytes")
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
    raise SystemExit(supervise(__file__, TITLE, 180))
