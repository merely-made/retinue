#!/usr/bin/env python3
# Design derived from Prns, github.com/KenAKAFrosty/Prns, Copyright (c) 2026 The Prns
# Authors, MIT OR Apache-2.0 (MIT elected). The discipline is theirs; no Prns text is
# copied here. See THIRD_PARTY_NOTICES.md and
# design_docs/2026-08-10_prns_donor_ledger.md.
"""Run the deterministic Retinue node-ingest corpus in a writable temporary copy."""

from __future__ import annotations

import argparse
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__)
    result.add_argument(
        "--target",
        default="retinue-node-ingest",
        help="fuzz target to run; its immutable seeds are fuzz/seeds/<target>",
    )
    result.add_argument("--seconds", type=int, default=900, help="libFuzzer wall-clock budget")
    result.add_argument("--dry-run", action="store_true", help="prove corpus isolation without invoking cargo-fuzz")
    return result


def main() -> int:
    args = parser().parse_args()
    if args.seconds <= 0:
        print("fuzz error: --seconds must be positive", file=sys.stderr)
        return 2

    root = Path(__file__).resolve().parents[1]
    # Seeds live beside the target that owns them, so adding a target cannot silently
    # borrow another target's corpus and report coverage it never had.
    seeds = root / "fuzz/seeds" / args.target
    if not seeds.is_dir() or not any(seeds.iterdir()):
        print(f"fuzz error: immutable seed corpus is missing: {seeds}", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix=f"{args.target}-") as directory:
        corpus = Path(directory) / "corpus"
        shutil.copytree(seeds, corpus)
        copied = sum(1 for item in corpus.rglob("*") if item.is_file())
        if args.dry_run:
            print(f"fuzz dry-run: copied {copied} immutable seeds into {corpus}")
            return 0

        probe = subprocess.run(["cargo", "fuzz", "--version"], cwd=root, check=False)
        if probe.returncode:
            print(
                "fuzz error: cargo-fuzz is required; install it with `cargo install cargo-fuzz`",
                file=sys.stderr,
            )
            return 2
        command = [
            "cargo",
            "fuzz",
            "run",
            args.target,
            str(corpus),
            "--",
            f"-max_total_time={args.seconds}",
            "-print_final_stats=1",
        ]
        return subprocess.run(command, cwd=root, check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main())
