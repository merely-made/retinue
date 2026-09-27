# RNS 1.5.4 topology and reliable-stream lanes

**Date:** September 27, 2026. **Baseline:** `dda4257` (clean at lane start).
**Status:** three software lanes implemented and verified within the scope below.

This extends the [repin receipt](../rns-1.5.4-repin/README.md) under the
[wire plan](../../../design_docs/2026-09-27_wire_compatibility_plan.md).
Three Sol agents own topology tests, bounded stream decoding, and the stock-RNS
reliable peer gate; the parent reviews and integrates their changes.

## Method and scope

The existing oracle environment remains at RNS 1.5.4 and LXMF 1.1.1. The new
driver uses RNS public APIs, project-authored inputs and observed link traffic.
No upstream implementation was copied or translated. Historical fixtures and
the withdrawn Prns reference retain their provenance and recorded uncertainty.

The topology test feeds equivalent signed announces to the shared-radio Node
and multi-interface Endpoint. Their deliberately different recipient sets are
tested alongside learning, duplicate suppression and egress policy. These are
software observations, not new physical relay receipts.

The reliable peer gate uses two real local TCP links. One exercises ordinary
RNS Buffer writes, reads and half-close. The other sends a public RNS
StreamDataMessage with compressed data and EOF in the same message. Each checks
the actual payload and EOF at both ends. This expands the earlier raw-link
Endpoint stream gate rather than renaming its historical evidence.

The decoded-frame contract limits owned decompressed output, separately from
the ready-to-read queue and bz2's internal workspace. An error terminates the
stream and reaches the host caller as InvalidData after any delivered prefix.
It cannot become a healthy EOF. Channel admission can precede decoding when
earlier data fills the read queue; a previously queued packet can therefore
already have been proved. The driver refuses a proof when failure is already
known and reports terminal failure when deferred decoding discovers it.

## Attempts and results

The initial topology build was canceled while waiting for the shared Cargo
package-cache lock. [topology-initial.log](topology-initial.log) preserves the
command and observed output. It is not a test receipt.

| Gate | Result | Evidence |
| --- | --- | --- |
| Full Retinue/Outrider tests | 403 passed, including doctests | [tests.log](tests.log) |
| Final library tests after the feature-guard repair | 266 passed | [final-libraries.log](final-libraries.log) |
| Allocation-only tests | 178 passed | [alloc-tests.log](alloc-tests.log) |
| Core-only and compression-only library checks | Passed | [core-only.log](core-only.log), [compression-only.log](compression-only.log) |
| Strict library Clippy | Passed | [clippy.log](clippy.log) |
| Expanded stock-RNS local matrix | 13/13 passed | [live/result.json](live/result.json), sibling per-gate logs |
| Formatting and validation registry | Passed; 21 manifests, 86 assets, 15 suites | [format-final.log](format-final.log), [registry-final.log](registry-final.log) |

[checks.json](checks.json) records commands and outcomes. The
[source digest manifest](source-digests.json) identifies source and the reliable
example binary; [files.json](files.json) hashes the receipt files. These are
working-tree observations, preserved together with code in the containing Git
commit, rather than clean-commit release certificates. The old repin receipt
still records its actual twelve gates; the maintained live runner now has thirteen.

The first default test compilation found an unqualified `vec!` in the new
ReliableChannel test; [tests-initial.log](tests-initial.log) retains that failure.
After correction, [tests.log](tests.log) records 403 passing tests, including
doctests. The paired topology tests, actual host decode-error/close test and
deferred-proof test are included.

The compression-only library build exposed a preexisting feature boundary:
the bz2 I/O adapter needed `std`, but the crate imported it only for Tokio or
tests. [compression-only-initial.log](compression-only-initial.log) retains the
failure. The guard now includes `compression`; the same check passes in
[compression-only.log](compression-only.log). This does not enable Tokio.

The first live attempt was canceled before listening because Cargo was waiting
for its shared cache. The next exchanged all bytes successfully but revealed
harness teardown errors: config deletion before RNS persistence and early
BufferedWriter finalization. All attempts are retained under `reliable/`.
[live-supervised.log](reliable/live-supervised.log) records the corrected driver:
the parent owns temporary storage until RNS exits and the gate runs a prebuilt
Rust example. It passed ordinary 339-byte request / 663-byte reply traffic and
an 8,209-byte request in a single 73-byte compressed EOF frame, followed by a
22-byte reply. Both sides observed EOF; the Rust process exited zero. RNS's
socket-reconnect warning follows the deliberate endpoint shutdown.

## Remaining gates

Firmware IFAC needs both physical carrier shells and correct packet budgets,
including Type2 relay growth. The plan records these prerequisites. Queue
pressure, natural route expiry, public-network behavior and physical/on-air
acceptance remain open. Output-vector capacity measurements do not establish a
total-process or decoder-workspace peak-memory bound.

Ordinary builds reuse `C:\t\cargo-targets\retinue`. To avoid an unrelated
resolver holding the shared cache, the gates used a temporary offline Cargo
home at `C:\t\cargo-homes\retinue-wire-compat`, seeded with 614 cached files
(116,486,456 bytes) selected from the lockfile. No credentials were copied.
[environment.json](environment.json) records versions and cache details.
There is no separate target, worktree, new virtual environment or hardware run.
After all gates completed, its exact path, marker and lack of active owners were
verified. Automatic approval review rejected deletion with only "blocked by
policy" as its reason. The cache remains owned by this completed verification;
[cleanup.log](cleanup.log) records the retained path and failed cleanup attempts.

## Reproduction

Use the pinned oracle Python environment and the repository's ordinary Cargo
cache/target. The temporary home was an operational workaround, not a dependency.
Run the Cargo commands in `checks.json`. After prebuilding the examples, run
`crates/retinue/oracle/run_live.py` for the thirteen gates, or this receipt's
predecessor runner `testing/receipts/rns-1.5.4-repin/run.py live --output <new-directory>`
to preserve each gate's output. That runner imports the current gate list;
historical recorded outcomes remain unchanged. Set `CARGO_TARGET_DIR` to the
prebuilt target and `CARGO_NET_OFFLINE=true` for the Cargo-backed gates.
