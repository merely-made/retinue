"""Census a flaky live gate by FAILURE MODE rather than by rate.

Counting is the expensive instrument. Separating a 13% failure rate from a 7% one
needs roughly 390 runs per arm, so a 30-run block cannot say anything at all -- and
reading one as though it could is how an afternoon gets spent attributing a local
harness bug to an upstream release.

Classifying is the cheap instrument, and it is strictly more informative. Each
failure carries a mechanism instead of one bit. On 2026-08-23 seven classified
failures of `interop_reqresp` located three distinct bugs -- a discarded inbound
link request, an announce lost to the gate's handler-registration window, and a
peer dropped during connection setup -- none of which any number of counted runs
would have surfaced.

So: run a gate n times, fingerprint every failure, group the fingerprints, and keep
one exemplar log per group. A gate with zero failures in 300 runs is bounded under
1%; a gate with failures tells you their shapes.

Usage:

    ./.venv/Scripts/python.exe flake_census.py interop_reqresp.py 120

Writes exemplar logs to `census/mode<N>.log`, numbered to match the printed table.
"""

from __future__ import annotations

import collections
import re
import subprocess
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
CENSUS = HERE / "census"

# The venv the gates themselves run under; see this directory's README.
VENV = HERE / ".venv" / "Scripts" / "python.exe"
if not VENV.exists():
    VENV = HERE / ".venv" / "bin" / "python"

# Signals worth carrying into a fingerprint. Gate-agnostic: every live gate prints
# verdict lines, and the rest are conditions any of them can hit. Add sparingly --
# a signal that varies run to run without meaning anything splits one mode into
# many and hides the grouping this tool exists to produce.
SIGNALS = [
    ("done", re.compile(r"DONE ([\w= ]+)")),
    ("traceback", re.compile(r"Traceback")),
    ("io_error", re.compile(r"IO_ERROR|peer closed")),
    ("timeout_line", re.compile(r"TIMEOUT (\w+)")),
    ("sock_closed", re.compile(r"was closed, attempting to reconnect")),
    ("link_estab", re.compile(r"ESTABLISHED|LINK_ACCEPTED")),
]

# Every "some label: PASS|FAIL" line a gate prints, in order. These are the gate's
# own done-conditions, so they discriminate modes better than anything hand-picked.
VERDICT = re.compile(r"^\s*([A-Za-z0-9 /_>-]+?):\s+(PASS|FAIL)\s*$", re.MULTILINE)

# A run that died building is not a run of the gate. Gates launch retinue through
# `cargo run --example`, and a shared target directory under heavy parallelism
# manufactures failures outright -- stale rlibs, "found possibly newer version of
# crate" -- that have nothing to do with the protocol. Classifying one of those as
# a failure mode poisons the census, so they are discarded and counted separately.
BUILD_FAILURE = re.compile(
    r"error\[E\d+\]|could not compile|found possibly newer version|"
    r"failed to run custom build|error: linking with|no such subcommand",
)


def concurrent_builders() -> str:
    """Rough build load at census time.

    Every gate here is timing-sensitive localhost networking, so a census taken on
    a contended box measures the box as much as the gate. Recorded rather than
    corrected: this number belongs beside any rate the tool prints, and a rate
    measured under load must not be compared against one measured idle. Measured
    2026-08-23 during the RNS 1.5.0 re-pin: 54 rustc and 14 cargo on 16 cores.
    """
    try:
        out = subprocess.run(["tasklist"], capture_output=True, text=True,
                             errors="replace", timeout=20).stdout.lower()
        return f"{out.count('rustc.exe')} rustc / {out.count('cargo.exe')} cargo"
    except Exception:
        try:
            out = subprocess.run(["pgrep", "-c", "rustc"], capture_output=True,
                                 text=True, timeout=20).stdout.strip()
            return f"{out} rustc"
        except Exception:
            return "unknown"


def fingerprint(log: str) -> tuple[str, ...]:
    parts = [f"{label}={m.group(1).strip()}" if m and m.groups() else
             (f"+{label}" if m else f"-{label}")
             for label, rx in SIGNALS
             for m in (rx.search(log),)]
    parts.extend(f"{name.strip()}={verdict}" for name, verdict in VERDICT.findall(log))
    return tuple(parts)


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    gate = sys.argv[1]
    runs = int(sys.argv[2]) if len(sys.argv) > 2 else 120
    if not (HERE / gate).exists():
        print(f"no such gate: {gate}")
        return 2

    CENSUS.mkdir(exist_ok=True)
    modes: dict[tuple[str, ...], list[int]] = collections.defaultdict(list)
    exemplar: dict[tuple[str, ...], str] = {}
    passes = 0
    discarded: list[int] = []

    load_before = concurrent_builders()
    print(f"  load at start: {load_before}")

    for i in range(1, runs + 1):
        done = subprocess.run(
            [str(VENV), "-u", str(HERE / gate)],
            cwd=HERE, capture_output=True, text=True, errors="replace",
        )
        log = done.stdout + done.stderr
        if done.returncode == 0:
            passes += 1
            verdict = "pass"
        elif BUILD_FAILURE.search(log):
            discarded.append(i)
            verdict = "discarded (build, not the gate)"
        else:
            key = fingerprint(log)
            modes[key].append(i)
            exemplar.setdefault(key, log)
            verdict = "FAIL"
        print(f"  run {i}/{runs}: {verdict}", flush=True)

    runs -= len(discarded)
    fails = runs - passes
    ranked = sorted(modes.items(), key=lambda kv: -len(kv[1]))

    print("\n" + "=" * 74)
    print(f"CENSUS {gate}: {passes} pass / {fails} fail of {runs}"
          f"  ({100.0 * fails / runs:.1f}%)  in {len(ranked)} distinct mode(s)")
    print(f"load: {load_before} at start, {concurrent_builders()} at end")
    if discarded:
        print(f"discarded {len(discarded)} run(s) that died building rather than in "
              f"the gate: {discarded}")
    print("=" * 74)
    if not fails:
        print(f"\nNo failures in {runs} runs. Bounded under {100.0 * 3 / runs:.1f}%"
              f" (rule of three, 95% confidence).")
    for n, (key, hits) in enumerate(ranked, start=1):
        # Numbered to match the exemplar file, which is written here rather than
        # at capture time so the two orderings cannot drift apart.
        (CENSUS / f"mode{n}.log").write_text(exemplar[key], encoding="utf-8")
        print(f"\nMODE {n}  x{len(hits)}   runs {hits}   (exemplar: census/mode{n}.log)")
        for part in key:
            if not part.startswith("-"):
                print(f"     {part}")
        absent = [p[1:] for p in key if p.startswith("-")]
        if absent:
            print(f"     ABSENT: {', '.join(absent)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
