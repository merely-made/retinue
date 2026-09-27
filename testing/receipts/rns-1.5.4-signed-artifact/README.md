# RNS 1.5.4 signed-artifact receipt

Recorded September 26, 2026 (America/New_York; September 27 UTC). Baseline:
Retinue `96c562d92d94ee580c7a8bb6d213d08cf45bf790`, initially clean. This receipt
adds a versioned fixture and extends the existing Rust replay to both versions.

## Observation and acquisition

The existing oracle environment temporarily installed PyPI `rns==1.5.4` with
`--no-deps`, then ran the project capture driver against the shipped `rnid` CLI.
The driver checks the exact reported version before capture. It invokes the tool
and reads output files; no reference implementation source was copied, translated,
or reviewed for this capture. The six inputs are the same project-selected test
identities, messages, and metadata used in the fresh 1.5.2 capture. The published
test secrets in the fixtures are not credentials.

- [install.json](install.json): selected fields from pip's install report,
  including package URL, wheel SHA-256, pip and platform versions. This records
  package acquisition, not independent verification of an upstream signing key.
- [environment.txt](environment.txt): installed package versions at capture time.
- [capture.txt](capture.txt): capture output and emitted artifact sizes.
- [comparison.json](comparison.json): baseline commit, timestamp, driver/fixture
  hashes and per-case input/byte comparisons.
- [rust-replay.txt](rust-replay.txt): the focused Rust test transcript.
- [restored-environment.txt](restored-environment.txt): restored CLI version and
  dependency check.

PyPI's [1.5.4 release](https://pypi.org/project/rns/1.5.4/) is dated September 11.
Wheel SHA-256:
`862615b12d449750c0b3c451dfc2cb189fd6da1c573c477a29f36f97d2a66a19`.

## Results and limits

All six inputs and resulting artifact byte strings match between 1.5.2 and 1.5.4:
two identities, each signing a detached artifact (224 bytes), embedded message
with typed metadata (300 bytes), and bare embedded message (248 bytes).

The three Rust tests pass across all twelve version-labelled cases: exact byte
reproduction; verification of signer, metadata and message; rejection of wrong
signers/messages and modified signatures. Formatting also passes.

The existing `crates/retinue/tests/fixtures/rns_signed_artifact.json` remains
unchanged, raw Windows checkout SHA-256
`a8396e707316850989283077a997c6de5e8fcf839139fa139270a0457e616c69`.
The new `rns_signed_artifact_1_5_4.json` has SHA-256
`1c562795c02f85322df7fa563c309ecd6c2ce3f13514c3f88360aa5140d948ab`.
Whole-file hashes differ because their producer metadata records different versions.
`comparison.json` also records the old fixture's committed-blob hash, since the
historical file is subject to Git newline conversion. The new capture and receipt
files disable that conversion; the capture driver is explicitly LF-normalized.

This is a six-case signed-artifact qualification. It does not establish full format
coverage, live transport, routing, firmware, radio, or LXMF compatibility with
1.5.4. The broader suite retains its 1.5.2 pin and existing receipts. Passing output
comparisons do not resolve the historical Prns provenance concerns; the
[donor ledger](../../../design_docs/2026-08-10_prns_donor_ledger.md), archived
donor-input fixtures, and notices remain in place.

## Reproduction

From the repository root, with the existing oracle virtual environment available:

```powershell
& crates/retinue/oracle/.venv/Scripts/python.exe -m pip install --no-deps rns==1.5.4
try {
    & crates/retinue/oracle/.venv/Scripts/python.exe -u crates/retinue/oracle/capture_signed_artifact.py --rns-version 1.5.4
    if ($LASTEXITCODE -ne 0) { throw 'Capture failed' }
} finally {
    & crates/retinue/oracle/.venv/Scripts/python.exe -m pip install --no-deps rns==1.5.2
}
$env:CARGO_TARGET_DIR = 'C:\t\cargo-targets\retinue'
cargo test -p retinue --locked --offline --test signed_artifact -j 2
cargo fmt --all --check
```

To repeat the comparison, parse both fixture JSON files, pair cases by `name`,
and compare every input field and `artifact_hex`. Require six unique cases per
file. The checked-in `comparison.json` records that comparison from this run.

After capture, `rnid --version` reported `rnid 1.5.2` again. No hardware was
accessed. The existing `C:/t/cargo-targets/retinue` was reused for replay; no
isolated Cargo home, worktree, or additional virtual environment was created.
