"""Run the complete mixed-runtime Retinue/RNS interoperability matrix.

Each gate runs in its own process because RNS owns process-global state and exits the
interpreter during teardown. The gate scripts return non-zero when their done-condition
fails, so this runner is suitable as one local release check.
"""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


HERE = Path(__file__).resolve().parent
GATES = (
    "interop_r1.py",
    "interop_ifac.py",
    "interop_r2.py",
    "interop_link.py",
    "interop_link_responder.py",
    "interop_reqresp.py",
    "interop_endpoint_stream.py",
    "interop_reliable_stream.py",
    "interop_resource_recv.py",
    "interop_resource_send.py",
    "interop_send_large.py",
    "interop_send_multiseg.py",
    "interop_library_resource_recv.py",
    "interop_library_resource_send.py",
    "interop_resource_cancel.py",
    "interop_resource_metadata.py",
    "interop_resource_proof_cache.py",
    "interop_reliable_responder_send.py",
    "interop_reliable_initiator_proofs.py",
    "interop_transport_node.py",
    "interop_link_liveness.py",
    "interop_transit_mtu.py",
    "interop_path_request.py",
    "interop_reliable_vanish.py",
    "interop_single_proof.py",
    "interop_ratchet_rotation.py",
    "interop_node_resource_metadata.py",
    "interop_library_resource_segments.py",
    "interop_library_request_resource.py",
    "interop_rebroadcast.py",
    "interop_ifac_resource.py",
    "interop_ifac_default.py",
    "interop_tcp_reconnect.py",
    "interop_egress_stall.py",
    "interop_listener_mode.py",
)


def main() -> int:
    failed: list[str] = []
    for gate in GATES:
        print(f"\n{'=' * 72}\nGATE {gate}\n{'=' * 72}", flush=True)
        completed = subprocess.run(
            [sys.executable, "-u", str(HERE / gate)],
            cwd=HERE,
            check=False,
        )
        if completed.returncode != 0:
            failed.append(gate)

    print("\n" + "=" * 72)
    if failed:
        print(f"LIVE INTEROP: FAIL ({', '.join(failed)})")
        return 1
    print(f"LIVE INTEROP: PASS ({len(GATES)} gates)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
