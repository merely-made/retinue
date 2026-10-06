"""Reproduce the bounded local RNS 1.5.4 qualification, retaining each gate's log.

Run with the oracle venv and CARGO_TARGET_DIR set. Output directories must be new;
failed runs are evidence and must not be overwritten. No hardware gates are included.
"""

import argparse
from datetime import datetime, timezone
from importlib.metadata import version
import json
import os
from pathlib import Path
import subprocess
import sys
import time

REPO = Path(__file__).resolve().parents[3]
ORACLE = REPO / "crates/retinue/oracle"
sys.path.insert(0, str(ORACLE))
from run_live import GATES


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane", choices=("live", "outrider", "resource", "routing"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--raw-output", type=Path,
                        default=REPO / "validation/results/rns-1.5.7-repin")
    args = parser.parse_args()
    assert version("rns") == "1.5.7", "requires rns==1.5.7"
    assert version("lxmf") == "1.2.0", "requires lxmf==1.2.0"
    args.output.mkdir(parents=True, exist_ok=False)
    if args.lane == "live":
        gates = [(name, ORACLE / name, []) for name in GATES]
    elif args.lane == "outrider":
        gates = [(path.name, path, []) for path in sorted((REPO / "crates/outrider/oracle").glob("interop_*.py"))]
        assert len(gates) == 7
    elif args.lane == "resource":
        gates = [(f"round-{i}-{name}", ORACLE / name, []) for i in range(1, 4)
                 for name in ("interop_resource_recv.py", "interop_resource_send.py",
                              "interop_send_large.py", "interop_send_multiseg.py")]
    else:
        raw = args.raw_output.resolve()
        gates = [
            ("timebase", ORACLE / "probe_announce_timebase.py", ["--output", str(raw / "timebase/result.json")]),
            ("route-full", ORACLE / "probe_route_freshness.py", ["--profile", "full", "--output", str(raw / "route-full")]),
            ("same-blob", ORACLE / "probe_route_freshness.py", ["--profile", "same-blob-diagnostic", "--output", str(raw / "same-blob")]),
        ]
    result = {"baseline_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=REPO, text=True).strip(),
              "rns": version("rns"), "lxmf": version("lxmf"), "lane": args.lane,
              "started_utc": datetime.now(timezone.utc).isoformat(), "gates": []}
    for name, script, extra in gates:
        print(f"START {args.lane}/{name}", flush=True)
        started = time.monotonic()
        command = [sys.executable, "-u", str(script), *extra]
        with (args.output / f"{name}.log").open("w", encoding="utf-8") as log:
            process = subprocess.Popen(command, cwd=script.parent, stdout=log, stderr=subprocess.STDOUT)
            timed_out = False
            try:
                code = process.wait(timeout=1200)
            except subprocess.TimeoutExpired:
                timed_out = True
                if os.name == "nt":
                    subprocess.run(["taskkill", "/PID", str(process.pid), "/T", "/F"], stdout=log, stderr=log)
                else:
                    process.kill()
                code = process.wait()
        result["gates"].append({"name": name, "command": command, "returncode": code,
                                "timed_out": timed_out, "elapsed_seconds": round(time.monotonic() - started, 3)})
        result["all_passed"] = all(g["returncode"] == 0 and not g["timed_out"] for g in result["gates"])
        (args.output / "result.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(f"END {name}: code={code}, timeout={timed_out}", flush=True)
    return 0 if result["all_passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
