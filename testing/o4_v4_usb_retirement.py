"""Bounded unread-request and fail-closed V4 USB observation probe."""
from __future__ import annotations

import argparse
import hashlib
import json
import time
from pathlib import Path

from serial.tools.list_ports import comports

from o3_usb_bench import Port, discovery, observation_request, tx, utc_now

V4_SERIAL = "44:1B:F6:6A:FA:64"
T114_SERIAL = "TULLE-T114-01"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--v4", default="COM7")
    parser.add_argument("--t114", default="COM10")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = {
        "started_utc": utc_now(),
        "runner_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "imported_helper_sha256": hashlib.sha256(
            Path(__file__).with_name("o3_usb_bench.py").read_bytes()
        ).hexdigest(),
        "limits": {"requests": 256, "write_seconds": 5, "peer_frames": 3},
        "ports": [
            {"device": p.device, "vid": p.vid, "pid": p.pid, "serial": p.serial_number}
            for p in comports() if p.device in (args.v4, args.t114)
        ],
        "requests_sent": 0,
        "transcript": [],
    }
    try:
        ids = {p["device"]: (p["vid"], p["pid"], p["serial"]) for p in report["ports"]}
        if ids.get(args.v4) != (0x303A, 0x1001, V4_SERIAL) or ids.get(args.t114) != (0x1915, 0x521F, T114_SERIAL):
            raise RuntimeError("bench identities do not match V4 DUT and T114 peer")
        port = Port(args.v4, 115200, report["transcript"])
        try:
            report["baseline_discovery"] = discovery(port, 1)[0]
            deadline = time.monotonic() + 5
            for request_id in range(1000, 1256):
                if time.monotonic() >= deadline:
                    break
                try:
                    port.write(observation_request(request_id))
                    report["requests_sent"] += 1
                except Exception as error:
                    report["write_stop"] = f"{type(error).__name__}: {error}"
                    break
            time.sleep(1)
        finally:
            port.close()
        if report["requests_sent"] == 0:
            raise RuntimeError("unread pressure sent no requests")

        fresh = Port(args.v4, 115200, report["transcript"])
        try:
            try:
                report["fresh_discovery"] = discovery(fresh, 2000)[0]
            except TimeoutError as error:
                report["fresh_discovery_error"] = f"{type(error).__name__}: {error}"
        finally:
            fresh.close()
        if "fresh_discovery_error" not in report:
            raise RuntimeError("unread pressure did not retire the fresh observation session")

        peer = Port(args.t114, 115200, report["transcript"])
        try:
            report["peer_transmits"] = [tx(peer, i, f"o4-retired-{i}".encode()) for i in range(3)]
        finally:
            peer.close()
        report["passed"] = True
        report["qualification"] = (
            "Fresh-session absence is consistent with retirement; the firmware latch is not "
            "directly instrumented. Peer TX acknowledgements do not prove V4 reception."
        )
        return 0
    except Exception as error:
        report["passed"] = False
        report["error"] = f"{type(error).__name__}: {error}"
        return 1
    finally:
        report["finished_utc"] = utc_now()
        args.output.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    raise SystemExit(main())
