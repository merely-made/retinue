#!/usr/bin/env python3
"""Enforce the repository's 600-line ceiling on first-party Rust files.

A file over the ceiling fails unless validation/loc-baseline.toml lists it, and a
listed file may only shrink. An entry for a file that is now within the ceiling,
or that no longer exists, also fails: the baseline must shrink with the code.

    python3 validation/loc_limit.py           # check
    python3 validation/loc_limit.py --prune   # drop entries that no longer apply
"""

import argparse
from pathlib import Path
import sys
import tomllib

REPO = Path(__file__).resolve().parents[1]
BASELINE = REPO / "validation" / "loc-baseline.toml"
ROOTS = ("apps", "crates", "firmware", "testing", "fuzz")
SKIP = {"target", "vendor", ".venv", ".python"}
LIMIT = 600


def rust_files():
    for root in ROOTS:
        for path in sorted((REPO / root).rglob("*.rs")):
            if not SKIP.intersection(path.relative_to(REPO).parts):
                yield path


def line_count(path):
    with path.open("rb") as file:
        return sum(1 for _ in file)


def load_baseline():
    if not BASELINE.exists():
        return {}
    return tomllib.loads(BASELINE.read_text(encoding="utf-8")).get("files", {})


def write_baseline(entries):
    lines = [
        "# Files over the 600-line ceiling when it was first enforced. Entries may only",
        "# shrink and must be removed once a file is within the ceiling.",
        "",
        "[files]",
    ]
    lines += [f'"{path}" = {count}' for path, count in sorted(entries.items())]
    BASELINE.write_text("\n".join(lines) + "\n", encoding="utf-8")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prune", action="store_true", help="drop entries that no longer apply")
    args = parser.parse_args()

    counts = {path.relative_to(REPO).as_posix(): line_count(path) for path in rust_files()}
    baseline = load_baseline()
    errors = []
    for path, count in counts.items():
        allowed = baseline.get(path)
        if count > LIMIT and allowed is None:
            errors.append(f"{path}: {count} lines, over the {LIMIT}-line ceiling")
        elif allowed is not None and count > allowed:
            errors.append(f"{path}: {count} lines, grew past its baseline of {allowed}")
    stale = {
        path for path in baseline if counts.get(path, 0) <= LIMIT
    }
    if args.prune:
        write_baseline({p: min(c, counts[p]) for p, c in baseline.items() if p not in stale})
        stale = set()
    errors += [f"{path}: baseline entry no longer applies; remove it" for path in sorted(stale)]

    over = sum(1 for count in counts.values() if count > LIMIT)
    for error in errors:
        print(f"loc ceiling: {error}", file=sys.stderr)
    print(f"loc ceiling: {len(counts)} files, {over} over {LIMIT} (baselined), {len(errors)} errors")
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
