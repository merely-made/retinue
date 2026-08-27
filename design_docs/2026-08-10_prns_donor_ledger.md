# Prns donor ledger

**Date:** 2026-08-10. **Closes:** shared source lock items 1 through 4.
**Scope:** every place in this repository that owes anything to Prns, itemized,
with the inbound license elected for each.

The [work lanes](2026-08-09_retinue_work_lanes.md) hold all four lanes behind a
provenance and evidence boundary. This is that boundary. It is deliberately
specific about *what kind* of debt each item is, because "derived from" covers
two very different things and conflating them makes the ledger useless: a file
that shares no text with the donor has different obligations from one that
quotes it, and a vector taken from a donor is not the independent evidence a
vector taken from RNS is.

## 1. The pin

| | |
| --- | --- |
| Donor | [Prns](https://github.com/KenAKAFrosty/Prns), a ground-up Rust Reticulum implementation |
| Commit | `72b6b30d27cac910ce20d370e1dc711fe9b95955` |
| Version | 0.3.4 |
| Upstream license | MIT OR Apache-2.0, "Copyright (c) 2026 The Prns Authors" |
| Inbound license elected | **MIT**, for every seam below |
| Local checkout | `Code/crates/prns`, verified clean at the pinned commit on 2026-08-10 |
| Peer version | RNS 1.4.2, the same release the oracle venv installs |

MIT is elected uniformly because its only obligation is retaining the copyright
and permission notice, which is satisfied by `crates/retinue/NOTICE` and by the
root `THIRD_PARTY_NOTICES.md`. Retinue's own files stay MPL-2.0.

## 2. The itemized seams

Three kinds of debt appear below, and they are not interchangeable.

- **Design derived.** The idea, structure, or discipline was read from Prns and
  reimplemented. No text was copied.
- **Layout derived.** A wire or file format was read from Prns and implemented
  independently. The format is a protocol fact; the code is ours.
- **Quoted.** Prns's text is reproduced verbatim.

### H1, announce-ingress admission (design derived; row added 2026-08-26)

| File | Prns counterpart | Debt |
| --- | --- | --- |
| `crates/retinue/src/announce_admission.rs` | `prns-core/src/routing/announce/interface_announce_limit/`, `destination_announce_limit/` | State-machine design read and reimplemented |

The harvest brief called H1 the cleanest seam, and it was harvested as one:
the 3/10 Hz new/established thresholds, the burst latch and penalty, and the
held-announce drip release are Prns's design, restated over Retinue's bounded
host tables and public diagnostics. The module header declares the derivation.
No Prns text was copied. This row was owed from the moment the module landed;
the 2026-08-12 sequencing doc flagged the omission and it is discharged here.

### H5, the validation hub (design derived)

| File | Prns counterpart | Overlap |
| --- | --- | --- |
| `validation/run.py` | `validation/run.py` | 559 lines vs 1587; 3.5% line overlap |
| `validation/manifest.toml` | `validation/manifest.toml` | 130 lines vs 1293; 3.7% |
| `validation/security/unsafe_audit.py` | `validation/security/unsafe-audit.py` | 276 lines vs 365; 8.1% |
| `validation/result.schema.json` | `validation/evidence-schema.json` | 46 lines vs 43; 22% |
| `validation/README.md` | `validation/README.md` | 45 lines vs 28; 22% |
| `validation/run_fuzz.py` | (fuzz suite entries) | no counterpart file |
| `validation/security/unsafe-policy.toml` | policy embedded in their auditor | restructured |
| `validation/security/flash_classification.py`, `flash-policy.toml` | none | follows their policy-TOML-plus-stdlib-auditor shape |
| `fuzz/` targets and seeds | `engine_ingest_never_panics` | shape only |

Those overlap figures were measured, not estimated, with a line-level
sequence matcher. The shared lines are `from __future__ import annotations`,
the import block, closing braces, and generic control-flow tokens. On the two
larger files the substantive identical lines number 29 and 21 respectively, and
every one is boilerplate. **No Prns text was copied into this tree.**

What *was* taken is the discipline, and it is the valuable part: drift
detection instead of duplication, so the registry re-discovers assets from Git
and fails on asymmetry in either direction; orphan-asset detection with
expiring exemptions; per-suite evidence against a strict schema with the
worktree checked clean before and after; the pr / release / scheduled tier
split with the rule that scheduled evidence cannot substitute for exact-SHA
release evidence; and the policy-file-plus-stdlib-auditor shape that
`flash_classification.py` now follows too. Prns's doctrine that a skipped CI
result is not green is quoted approvingly and acted on.

### H7, the signed artifact (layout derived, one quotation)

| File | Debt |
| --- | --- |
| `crates/retinue/src/artifact.rs` | Envelope layout read from `prns-core/src/identity/signed_artifact.rs` |
| `crates/retinue/src/msgpack.rs` | Written to serve it; no Prns counterpart was read |
| `crates/retinue/tests/signed_artifact.rs` | **Quoted**: the 224-byte `RNS_RSG` hex constant from Prns's tests |

The layout is a description of an RNS wire format: which fields exist, that the
metadata map opens with `signer` and `pubkey`, that the signature covers the
encoded envelope rather than the message. Reading it from Prns saved a
reverse-engineering pass and nothing else.

The one quotation is deliberate and is the ledger's most interesting entry. Our
vectors were captured independently by running RNS 1.4.2's `rnid` executable,
so they are independent oracle evidence. Prns's published constant is quoted
beside them and asserted equal. That makes the test say something neither
project could say alone: RNS corroborates the vector Prns publishes, and a
donor's self-tests cannot corroborate themselves. It is quoted as evidence
*about* Prns, not used as an implementation input.

### Not derived, recorded so the boundary reads in both directions

- `crates/retinue/src/command.rs` (FS2) owes Prns nothing. Prns supplies no
  command authorization, and the
  [carrier decision](2026-08-10_fs2_command_carrier_decision.md) explicitly
  declines to build FS2 on the signed artifact.
- `design_docs/2026-08-10_fs4_custody_and_fs5_seizure.md` cites Prns's release
  custody process as a donor for its checklist, and cites Prns's cleartext
  flash storage as the counterexample the seizure paragraph rejects. Citing a
  process is not porting code, but it is worth naming in both directions.

## 3. Evidence labels

The harvest brief's rule, restated because it is the thing most easily lost:

> An untouched Prns executable is an independent external peer. A vector, test,
> or implementation derived from Prns is donor-conformance evidence in that
> seam, not an independent oracle.

Applied here:

- The RSG/RSM vectors in `crates/retinue/tests/fixtures/rns_signed_artifact.json`
  are **independent oracle evidence**. They came from `rnid`, not from Prns.
  `capture_signed_artifact.py` imports nothing from RNS and drives the shipped
  executable as a subprocess.
- Agreement between `retinue::artifact` and Prns on the same bytes is
  **donor-conformance evidence** for that seam, because the layout came from
  Prns. It is worth having and it is not the same claim.
- The validation registry produces no protocol evidence at all. It is tooling,
  and whether its shape came from Prns has no bearing on what the suites it
  indexes prove.

## 4. The untouched executable (lock item 3)

**Source preserved, binary not yet built.** The checkout at `Code/crates/prns`
is clean at the pinned commit and no local modification exists. Nothing in this
lane has written to it, and nothing should: it is the Peer lane's instrument.

Building the peer executable and running the three pairings is Lane 1's work
(H8), and this ledger does not do it. What matters for the lock is that the
source is preserved untouched and pinned, which it is, and that the seams
derived here are recorded *before* the interop receipts are captured, which
this document does. Once `retinue::artifact` exists, agreement with Prns in the
signed-artifact seam can no longer be read as an independent third corner.

## 5. Disclosure state (lock item 4)

The harvest brief records that the pinned tree contains a reproducible embedded
entropy issue that may affect cryptographic operations, and deliberately does
not publish the affected board, source path, reproduction, or impact.

**State as of 2026-08-10: not yet reported to the maintainer.** No disclosure
record exists in this repository, and the details are not in this document
either, which is the correct posture until a report has gone through Prns's
`SECURITY.md` and a state is recorded.

`design_docs/private/` is now gitignored so the record has a safe home in this
working tree without risking a commit. Writing it belongs to whoever holds the
reproduction; this ledger's job is to say that it is owed and unpaid.

Retinue keeps its hardware RNG live and does not copy the affected pattern. The
V4's entropy note in `firmware/heltec-v4-phy/src/store.rs` already refuses to
generate an identity from a pseudo-random source, which is the same class of
defect approached from the other side.

## 6. Where the notices live

- `crates/retinue/NOTICE`, following the `crates/tucket/NOTICE` precedent, with
  the MIT text in full.
- `THIRD_PARTY_NOTICES.md` at the repository root, aggregating this and the
  existing MeshCore and lora-phy notices.
- A header line on each derived file naming Prns, the commit, and the license.
- Provenance paragraphs updated in `README.md`, `crates/retinue/README.md`, and
  `crates/retinue/src/lib.rs`, which previously said the implementation inputs
  were the public protocol material and Beechat and nothing else. That sentence
  was true when written and is not true now.
