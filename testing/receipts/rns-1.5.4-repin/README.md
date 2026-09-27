# RNS 1.5.4 live qualification and boundary repairs

**Date:** September 27, 2026. **Pins:** `rns==1.5.4`, `lxmf==1.1.1`.
**Baseline:** Retinue `acb82f6dd8c8f6ed46a15f5d0910c4b55e8e6ffa`, initially clean.
RNS 1.5.4 supersedes 1.5.2 as the live-oracle target within the measured scopes
below. Historical captures retain their producer versions. The
[wire plan](../../../design_docs/2026-09-27_wire_compatibility_plan.md) records
the audit, repairs, and remaining compatibility work.

## Acquisition and method

The existing oracle environment installed the published 1.5.4 wheel without
changing dependency versions. [install.json](install.json) retains selected pip
report fields and the download URL; [environment.txt](environment.txt) records
the installed packages. Wheel SHA-256 is
`862615b12d449750c0b3c451dfc2cb189fd6da1c573c477a29f36f97d2a66a19`, matching
the [PyPI release](https://pypi.org/project/rns/1.5.4/) dated September 11.
This is package-hash identification, not a newly verified publisher signature.
The GitHub release URLs for 1.5.3 and 1.5.4 returned 404 in this run; no detailed
upstream change list is inferred from that absence. Qualification rests on the
observations below, not an assumed unchanged implementation.

The existing drivers use public reference APIs/CLI and observe output/wire
behavior. No RNS or Prns implementation code was copied or translated in this
work. Prns was neither built nor run: its withdrawn-reference status, old
receipts, and unresolved provenance remain in the donor ledger.

## Baseline live results

| Check | Result | Evidence |
| --- | --- | --- |
| Stock-RNS Retinue matrix | 12/12 passed | [live/result.json](live/result.json), sibling logs |
| Outrider / stock LXMF | 7/7 passed | [outrider/result.json](outrider/result.json), sibling logs |
| Resource repetition | 12/12 passed, four gates interleaved over three rounds | [resource/result.json](resource/result.json), sibling logs |
| Persistent announce timebase | P1 rejects equal replacement; P2 rejects low after high; P3 accepts high after low | [timebase-result.json](timebase-result.json) |
| Forwarded route/freshness | 72/72 valid measurements, all decisions match 1.5.2 | [route-comparison.json](route-comparison.json) |
| Same-blob isolation | 6/6 valid measurements, all decisions match 1.5.2 | [same-blob-comparison.json](same-blob-comparison.json) |
| Signed-artifact recapture | Six current captures unchanged; old fixture untouched | [capture log](signed-artifact-capture.log), [earlier comparison](../rns-1.5.4-signed-artifact/README.md) |

The stock matrix covers bidirectional announce/IFAC, path resolution, both link
roles, request/response, best-effort Endpoint streaming, inbound/outbound
Resources, 120 KB and 2.5 MB transfers, and stock links through a Retinue transport.
The stream driver's old heading incorrectly called its raw link traffic a
Channel/Buffer test; that wording is corrected. It does not qualify stock-RNS
compressed reliable streaming. Reliable-stream tests below are Rust model and
host integration tests with their own narrower evidence boundary.

Route comparison pairs the same cell descriptors and compares admission,
route-transition and observed-hop decisions; it does not compare incidental
timestamps or random transport identities. The prior full result hash matches
the published 1.5.2 receipt. Full current results are retained as
[route-full-result.json](route-full-result.json) and
[same-blob-result.json](same-blob-result.json).

[routing-raw.zip](routing-raw.zip) preserves all 1,218 raw probe files, including
captures, configurations, logs and disposable local-test identities. These are
test-network identities, not deployment credentials. Its
[per-file manifest](routing-raw-manifest.json) records original bytes and hashes.
Unpacked evidence also remains under `validation/results/rns-1.5.4-repin`.
The archive makes this receipt independent of that ignored local directory.

## Repairs and final verification

The baseline runs completed before implementation edits. The boundary audit
then produced two bounded repairs:

- Endpoint bridges now reject an unrelated third interface before refreshing
  their lifetime. Node does the same before updating lifetime or deduplication
  state, so rejected traffic cannot suppress a later legitimate packet.
- Channel buffering keeps one pending decoded frame and feeds a read queue
  capped by `READ_BYTES`. The host driver drains all chunks before EOF; plain,
  compressed, and EOF-bearing regressions cover ordered delivery without loss.
  `recv_finished` now waits until buffered data has been drained. Decompression
  still allocates one whole decoded frame, which remains a separate limit gate.

Final code verification:

- **397** default Retinue/Outrider tests passed, including integration and doc
  tests: [rust-default.log](rust-default.log).
- **178** allocation-only Retinue library tests passed:
  [rust-alloc.log](rust-alloc.log). Its existing compression-only test helper
  emits a dead-code warning in this feature configuration.
- Core-only Retinue check passed: [rust-core.log](rust-core.log).
- Strict library Clippy passed: [clippy.log](clippy.log).
- Formatting passed after formatting the new regression tests.
- Registry verification passed: 21 manifests, 85 assets, 15 suites. The first
  attempt found the new receipt runner unregistered; that output remains in
  [validation-first.log](validation-first.log). The runner now belongs to the
  existing oracle suite, and [validation.log](validation.log) records the pass.
- The complete twelve-gate Retinue suite was rerun after the repairs:
  [live-after-repairs/result.json](live-after-repairs/result.json).

The runner's `baseline_commit` records the parent revision; these are working-tree
observations, not `validation/run.py record` clean-commit release certificates.
The [source digest manifest](source-digests.json) identifies the repaired source snapshot.
The containing Git commit preserves the implementation and these observations
together. Logs and raw evidence are retained with hashes; failures are not erased.
The [receipt file manifest](files.json) records byte lengths and SHA-256 hashes
for every other file in this receipt directory.

## Reproduction and remaining gates

Use the existing virtual environment after installing
`crates/retinue/oracle/requirements.txt`. Set
`CARGO_TARGET_DIR=C:/t/cargo-targets/retinue` and prebuild examples with
`cargo build -p retinue -p outrider --examples --locked --offline -j 2`.
Run this directory's `run.py` using the oracle Python, one lane at a time:
`live`, `outrider`, `resource`, and `routing`, each with `--output` naming a new
receipt directory. For routing, also supply a fresh `--raw-output` path under
`validation/results`. Existing outputs are refused rather than overwritten.
Each gate has a 1,200-second process limit and a retained log. No hardware gates
are included. `CARGO_NET_OFFLINE=true` keeps the Cargo subprocesses offline.

Rerun Rust checks with `cargo test -p retinue -p outrider --locked --offline -j 2`,
then `cargo test -p retinue --no-default-features --features alloc --lib --locked
--offline -j 2` and the core-only/Clippy commands named above. The environment
remains at RNS 1.5.4; the general pin and default signed-artifact capture agree.

This is local interoperability at tested loads, not complete reference parity or
a bounded flake rate. Open gates include firmware IFAC, topology-specific announce
rebroadcast, decoded-frame allocation caps, stock reliable compressed streams,
queue saturation, I2P, discovery metadata, natural elapsed route expiry,
public-network behavior, and physical/on-air requalification. The existing
Retinue Cargo target is retained for ordinary builds. No isolated Cargo home,
worktree, additional virtual environment, or hardware operation was used.
