"""Record software gates. This runner never opens, configures or flashes a radio."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--allow-downloads", action="store_true", help="Allow acquiring missing locked dependencies")
    parser.add_argument("--gates", nargs="+", help="Run only the named gates when repairing a recorded failure")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[3]
    output = args.output.resolve()
    if output.exists():
        parser.error(f"refusing to overwrite evidence: {output}")
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(Path("C:/t/cargo-targets/retinue")) if os.name == "nt" else env.get("CARGO_TARGET_DIR", str(root / "target"))
    base = ["cargo"]
    commands = {
        "protocol-tests": base + ["test", "-p", "sennet", "-p", "tucket", "--locked", "--offline", "-j", "2"],
        "hardware-check": base + ["check", "-p", "sennet", "-p", "tucket", "--all-targets", "--features", "sennet/hardware,tucket/hardware", "--locked", "--offline", "-j", "2"],
        "clippy": base + ["clippy", "-p", "sennet", "-p", "tucket", "--all-targets", "--features", "sennet/hardware,tucket/hardware", "--locked", "--offline", "-j", "2", "--", "-D", "warnings"],
        "embedded-check": base + ["check", "-p", "sennet", "-p", "tucket", "--no-default-features", "--target", "thumbv7em-none-eabihf", "--locked", "--offline", "-j", "2"],
        "radio-hand-tests": base + ["test", "-p", "radio-hand", "--tests", "--locked", "--offline", "-j", "2"],
        "format": base + ["fmt", "--all", "--", "--check"],
        "registry": [sys.executable, "validation/run.py", "verify"],
        "capture-cli": [sys.executable, "testing/receipts/sennet-tucket-current/capture_cli_checks.py"],
        "stock-bench-admission": [sys.executable, "-B", "testing/receipts/sennet-tucket-current/stock_bench_checks.py"],
    }
    if args.gates:
        unknown = set(args.gates) - commands.keys()
        if unknown:
            parser.error(f"unknown gates: {sorted(unknown)}")
        commands = {name: commands[name] for name in args.gates}
    if args.allow_downloads:
        commands = {name: [arg for arg in command if arg != "--offline"] for name, command in commands.items()}
    output.mkdir(parents=True)
    result = {
        "baseline_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "evidence_kind": "working-tree software gates; no stock-peer or physical acceptance",
        "cargo_target": env["CARGO_TARGET_DIR"],
        "cargo_home": env.get("CARGO_HOME"),
        "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "source_sha256": {},
        "gates": {},
    }
    for folder in (root / "crates/sennet", root / "crates/tucket"):
        for path in sorted(folder.rglob("*.rs")):
            result["source_sha256"][str(path.relative_to(root)).replace("\\", "/")] = hashlib.sha256(path.read_bytes()).hexdigest()
    for name, command in commands.items():
        print(f"running {name}", flush=True)
        started = time.monotonic()
        with (output / f"{name}.log").open("w", encoding="utf-8") as log:
            process = subprocess.run(command, cwd=root, env=env, stdout=log, stderr=subprocess.STDOUT, text=True)
        result["gates"][name] = {
            "command": command, "exit_code": process.returncode,
            "seconds": round(time.monotonic() - started, 3),
            "log_sha256": hashlib.sha256((output / f"{name}.log").read_bytes()).hexdigest(),
        }
        (output / "result.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(f"{name}: exit {process.returncode}", flush=True)
    return int(any(gate["exit_code"] for gate in result["gates"].values()))


if __name__ == "__main__":
    raise SystemExit(main())
