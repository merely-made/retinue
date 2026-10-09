"""Node resource-metadata gate: stock RNS sends a Node-backed peer `Resource(data, metadata=...)`.

A retinue `Node`, behind a small TCP shell (examples/node_resource_oracle.rs), announces
until RNS links to it. RNS then sends a Resource with metadata. The Node must deliver the
exact data with the exact packed metadata (`Resource.py` 261-272, 707-749) and prove the
transfer, so the RNS sender concludes COMPLETE. Before review item 24, a Node dropped the
metadata and only counted the drop.

Build: cargo build -p retinue --examples --all-features --locked
Run:   python -u interop_node_resource_metadata.py   (CARGO_TARGET_DIR as for the build)
"""
from __future__ import annotations

import hashlib
import os
import subprocess
import sys
import threading
from pathlib import Path

from library_gate import REPO, RESOURCE_STATUS, Retinue, payload, start_rns, supervise, verdict

TITLE = "NODE RESOURCE METADATA INTEROP"
RUN_SECONDS = 90
# A few parts: RNS sizes an advertisement's hashmap by its static Link.MDU, not the link's
# 255-byte MTU, so a many-part offer would not fit the Node's link (`Resource.py` 1256-1257).
LENGTH = 1_500
SEED = 0x4E0D
PAYLOAD = payload(LENGTH, SEED)
METADATA = {"name": "rns.bin", "size": LENGTH}


class NodeOracle(Retinue):
    """The running node_resource_oracle example and its captured output."""

    def __init__(self) -> None:
        default_target = str(REPO.parent.parent / "target")
        target = Path(os.environ.get("CARGO_TARGET_DIR", default_target))
        name = "node_resource_oracle.exe" if os.name == "nt" else "node_resource_oracle"
        binary = target / "debug" / "examples" / name
        if not binary.is_file():
            raise FileNotFoundError(f"build the node_resource_oracle example first: {binary}")
        self.proc = subprocess.Popen(
            [str(binary), str(RUN_SECONDS)],
            cwd=REPO, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, bufsize=1,
        )
        self.lines: list[str] = []
        self.lock = threading.Lock()
        threading.Thread(target=self._pump, daemon=True).start()


def main() -> int:
    node = NodeOracle()
    exit_code = 1
    try:
        port = node.wait_port()
        import RNS
        from RNS.vendor import umsgpack

        start_rns(port)
        concluded = threading.Event()
        state: dict[str, object] = {}

        class Linker:
            aspect_filter = "retinue.node-resource"

            def received_announce(self, destination_hash, announced_identity, app_data):
                if state.get("started"):
                    return
                state["started"] = True
                remote = RNS.Destination(
                    announced_identity, RNS.Destination.OUT,
                    RNS.Destination.SINGLE, "retinue", "node-resource",
                )
                link = RNS.Link(remote)
                state["link"] = link

                def established(established_link):
                    def done(resource):
                        state["status"] = resource.status
                        print(f"  RNS sender concluded: {RESOURCE_STATUS.get(resource.status, resource.status)}",
                              flush=True)
                        concluded.set()

                    print(f"  RNS: link up, sending {LENGTH}-byte Resource with metadata", flush=True)
                    state["sent"] = RNS.Resource(PAYLOAD, established_link, metadata=METADATA, callback=done)

                link.set_link_established_callback(established)

        RNS.Transport.register_announce_handler(Linker())
        print("waiting for the Node's announce, link and transfer...", flush=True)
        node.wait_for(r"METADATA .*|METADATA_NONE|MODE_ERR .*", 60)
        concluded.wait(timeout=20)
        link = state.get("link")
        if link is not None:
            link.teardown()

        digest = node.find(r"RESOURCE (\d+) ([0-9a-f]{64})")
        bytes_ok = (
            digest is not None and int(digest.group(1)) == LENGTH
            and digest.group(2) == hashlib.sha256(PAYLOAD).hexdigest()
        )
        metadata_line = node.find(r"METADATA ([0-9a-f]+)")
        received = bytes.fromhex(metadata_line.group(1)) if metadata_line else None
        status = state.get("status")
        print("\n" + "=" * 72)
        ok = verdict("Node received RNS's exact data", bytes_ok, f"{LENGTH} bytes")
        ok &= verdict("Node received RNS's metadata exactly", received == umsgpack.packb(METADATA),
                      repr(umsgpack.unpackb(received)) if received else "none")
        ok &= verdict("RNS sender reached COMPLETE", status == RNS.Resource.COMPLETE,
                      RESOURCE_STATUS.get(status, str(status)))
        print("=" * 72)
        print(f"{TITLE}: {'PASS' if ok else 'FAIL'}", flush=True)
        exit_code = 0 if ok else 1
        return exit_code
    finally:
        node.kill()
        import RNS

        RNS.exit(exit_code)


if __name__ == "__main__":
    if sys.argv[1:] == ["--peer"]:
        raise SystemExit(main())
    raise SystemExit(supervise(__file__, TITLE, 180))
