# Prns donor ledger

**Reference withdrawn, 2026-09-26.** The owner has raised unresolved concerns
about Prns's provenance. Prns is no longer an approved implementation donor,
recommended dependency, or trusted independent reference for new qualification.
This is a project trust decision, not a finding of infringement. Recommendations
below to harvest, adopt or prefer Prns are historical and superseded. Existing
attribution and measured observations remain; see the Prns donor ledger.

**Date:** 2026-08-10. **Closes:** shared source lock items 1 through 3.
**Scope:** every place in this repository that owes anything to Prns, itemized,
with the inbound license elected for each.

**Corrected 2026-08-25.** This line read "items 1 through 4". It never closed
item 4: §5 of this same document has said from the first draft that disclosure
was owed and unpaid, so the header contradicted the body for a fortnight. The
2026-08-12 sequencing document flagged both this sentence and the matching one
in the assurance status and asked for them to be fixed together; this is that
fix. Item 4's current state is in §5.

The [work lanes](2026-08-09_retinue_work_lanes.md) hold all four lanes behind a
provenance and evidence boundary. This is that boundary. It is deliberately
specific about *what kind* of debt each item is, because "derived from" covers
two very different things and conflating them makes the ledger useless: a file
that shares no text with the donor has different obligations from one that
quotes it, and a vector taken from a donor is not the independent evidence a
vector taken from RNS is.

## Retained material and reassessment

Current source headers and tests confirm announce-admission design, signed-artifact
layout, validation/fuzzing design and a quoted 224-byte test vector. Hopspot
restore binaries are also retained. No implementation, test or package is removed
by this documentation change. Historical overlap figures have not been rerun and
do not establish upstream provenance. Notices remain while affected material is
assessed for direct protocol corroboration or independent replacement.

Earlier assertions of ground-up authorship and license sufficiency are historical
assessments, not renewed assurances of upstream rights. Peer receipts establish
observed outcomes against named binaries, not independently verified authorship.
This withdrawal does not lift the separate security disclosure embargo.

## Retained-material audit (2026-09-26)

**Baseline:** Retinue `a3b434d`, initially clean; read-only adjacent Mere
`5f897592`. This audit examines Retinue's current files, capture drivers,
fixtures, package manifests, consumers and recorded provenance. It does not
establish Prns's original authorship, compare against a newly fetched upstream
source tree, or issue a legal clearance. Historical overlap measurements are
not rerun or treated as proof of independent origin. No implementation,
fixture, binary or catalog entry was removed, and no board was accessed.

### Dispositions

| Material | Finding | Recommended disposition and completion condition |
| --- | --- | --- |
| `tests/signed_artifact.rs::PRNS_PUBLISHED_RSG` | Exactly 224 bytes; equals the retained `rsg_prns_identity` capture. One dedicated test compares the quotation. | **Remove/replace fixture inputs.** Delete the quotation and its dedicated comparison in a follow-up; retain general reproduction, validation and rejection tests. |
| `oracle/capture_signed_artifact.py` and `tests/fixtures/rns_signed_artifact.json` | Six cases: detached, embedded with metadata and bare embedded for each of two identities. Three cases use Prns's test secret; metadata includes the string `Prns`, including in the Retinue-identity metadata case. The driver invokes `rnid` as a subprocess and reads emitted artifacts. Output producer and input origin are distinct. | **Replace donor-specific inputs.** Capture with project-chosen identities and metadata under a pinned RNS tool; retain old captures as historical evidence rather than silently rewriting their source labels. Preserve all three shape checks and negative checks. |
| `src/artifact.rs` | Header records envelope-layout derivation. Retained captures independently exercise the resulting wire bytes; present tests pass. Search of Retinue crates/apps found direct public API consumers in the signed-artifact test, not a production caller. External consumers were not exhaustively audited. | **Retain with attribution and stronger reference evidence.** Requalify with fresh donor-free capture inputs. A wire-layout origin does not itself establish implementation copying; passing tests also do not establish provenance clearance. |
| `src/msgpack.rs` | Its own provenance explicitly says the codec was written from the MessagePack specification without reading a codec implementation. The donor connection is the artifact format it serves. | **Retain.** Do not conflate this codec with a copied Prns MessagePack implementation. Keep the historical format attribution. |
| `src/announce_admission.rs` | Explicit state-machine design derivation; default interface thresholds 3/10 Hz, burst hold/penalty and held release. `lib.rs` gates it on `tokio`. `Endpoint` constructs it by default, feeds interface/destination observations and drains held announces through it. | **Replace after behavioral specification.** This is live host traffic control, not removable dead code. Define independently justified capacity, fairness, burst/release and destination-rate requirements, retain integration protection, then implement a replacement. Do not merely rename states or erase attribution. No conclusion about embedded Node derivation follows from this host module. |
| `validation/` and `fuzz/` | Explicit design attribution. Reviewed registry/schema, unsafe/flash policy machinery, fuzz launcher and Node-ingest harness show project-specific checks and types. No fresh upstream expression comparison was performed. | **Retain provisionally with notices.** No concrete copied implementation was established by this audit. Keep useful safeguards; if stronger separation is required, review individual functions against independent requirements instead of deleting the validation system. |
| Hopspot package | Three tracked binaries total 1,727,440 bytes, plus four descriptor/signature files. Every binary matches its declared size and SHA-256. Both `index.toml` and `windows-v4-staging-index.toml` still offer it. `catalog.rs` and `tests/hopspot_release.rs` have coupled expectations. | **Retire from active offering/distribution.** Update both catalogs, package documentation and affected tests together. Preserve manifest hashes, attribution and historical receipts. The signature check establishes correspondence to a named key, not upstream authorship. Decide archival handling before deleting evidence-bearing files. |

### Recovery and cross-repository consequences

`heltec-v4-current-recovery.md` uses the ESP32-S3 ROM loader and the Retinue
package, not Hopspot. The local Retinue payload exists and matches the current
manifest's size/hash. This is an available documented recovery route, not a
new physical recovery qualification. The manifest still describes a historical
working-tree build; do not present the hash check as a reproducible clean build.
Retiring Hopspot also withdraws the convenience of restoring that foreign
firmware specifically, even though Retinue recovery remains available.

Mere's `crates/dramatis/insigne/src/key.rs` has a test constant `RNS_IDENTITY`
explicitly identified as Prns's public fixture identity. Its test checks identity
text parsing/round-trip behavior. Replace it with a project-owned fixture in a
separate Mere change; that file was not edited here. This spot check is not an
exhaustive cross-workspace provenance audit.

### Verification and recommended execution order

`cargo test -p retinue --locked --offline --test signed_artifact -j 2` passed
**4/4** on the audit baseline. This replays existing fixtures; it does not rerun
RNS or prove original source authorship. A local script measured the quotation,
listed all six fixture cases and checked package payload sizes/hashes. No
upstream code, firmware execution or new reference capture was needed.

1. Replace donor-specific test inputs using pinned RNS captures and cover the
   Mere fixture separately; remove the now-redundant Prns quotation test.
2. Retire Hopspot catalog selections with corresponding documentation/tests,
   preserving evidence and the Retinue recovery instructions.
3. Specify and replace host announce admission without losing bounded queues,
   neighbor fairness or traffic-control behavior.
4. Retain artifact/MessagePack and validation tooling with notices; revisit only
   concrete unresolved derivations, not naming or generic organizational ideas.

Build output reused `C:/t/cargo-targets/retinue`, retained for ordinary Retinue
qualification. No isolated Cargo home or worktree was created. The audit closes
the retained-material inventory, not the follow-up replacements or upstream
provenance uncertainty.

## Applied disposition (2026-09-26)

The audit recommendations are implemented in this follow-up. Historical sections
below remain a record of inputs, not descriptions of the current implementation.

- Captured six fresh signed-artifact cases with pinned `rnid 1.5.2`, project
  identities and metadata; removed the copied constant/comparison test. Original
  captures remain in `testing/receipts/prns-retirement/rns_signed_artifact_legacy.json`.
- Removed Hopspot from both catalogs and both Windows/public stage assemblers.
  Its three binaries, manifest and four descriptor/signature files were moved
  byte-identically into the receipt archive. The repository still retains these
  files; assembled distributions no longer include them. Signature evidence tests
  follow the archive path. Retinue recovery remains independent of Hopspot.
- Replaced host announce admission with virtual arrival budgets under the contract
  below. A regression test caught that policy changes must preserve deferred work
  and counters. Rate debt now resets while cooldowns/counters survive within new
  capacities; evicted-interface deferred work retires without task-restart spin.
  Configured grace/penalty semantics differ from the old state machine; no exact
  RNS scheduler equivalence or new radio receipt is claimed.
- Retained artifact/MessagePack and validation tools with historical attribution.
  Replaced the adjacent Mere identity parser's donor test value with project-chosen
  bytes, without changing its parsing behavior or unrelated Mere work.
- Clarified acknowledgment: the maintainer respects the reference implementation's
  terms in their own use. This imposes no extra use restrictions on independently
  authored MPL-2.0 code and does not certify unresolved historical provenance.

Validation: 219 Retinue library tests, 5 endpoint-ingress tests, 3 signed-artifact
replay tests, 82 Linkboy library tests, the archived release-signature test and
Mere's focused identity-parser test passed. Strict library Clippy for Retinue and
Linkboy, registry verification, stage-script parsing and formatting also passed.
All eight archived release files were
compared byte-for-byte with the prior commit. No hardware was accessed. Reused
`C:/t/cargo-targets/retinue` and `C:/t/cargo-targets/mere` remain ordinary reusable
qualification targets; no isolated home or worktree was created.

## Replacement contract (2026-09-26)

The replacement host admission policy is a Retinue-owned scheduling choice,
not an RNS or Prns parity implementation. Requirements come from Endpoint's
bounded held queue and public policy surface: each interface has an isolated
rate budget; unknown-route bursts wait while known routes remain processable;
queue release waits for rate debt and cooldown, then is paced; repeated
announces to one destination have a separate rate budget and penalty. Capacity
zero retains no rows and fails closed for unknown routes / destination relay
when their limiting is enabled. Row eviction is deterministic oldest-use-first.
Changing policy resets rate debt while keeping retained interface counters and
in-flight cooldowns; release tasks wake to reconsider deadlines. Eviction retires
orphaned held work rather than restarting a task without its accounting row.

Implement virtual arrival deadlines: consuming an event advances a deadline by
one configured period; elapsed time pays down that debt. Interfaces allow two
immediate arrivals and defer further excess, capping accumulated debt at the
configured observation horizon. Destination grace allows that many extra
immediate arrivals; blocked attempts do not extend a penalty indefinitely.
Interface cooldown is the larger of hold and penalty, followed by release
spacing. Tests must cover flooding, neighbor isolation, known routes, eventual
release, destination grace/recovery, capacity zero, eviction and policy reset.
Existing public numeric defaults are retained as compatibility settings, not
claimed as newly invented constants. Historic Prns influence remains recorded;
this replacement is not a retrospective clean-room certification.

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

**State as of 2026-08-25: reported.** The paragraphs above are kept as written
because they record the posture at the time. The finding was re-reviewed against
current upstream, where it is still present, a disclosure record was written to
`design_docs/private/`, and the report was sent through Prns's `SECURITY.md`
private vulnerability reporting. Lock item 4's duty is discharged: this
repository is no longer holding an unreported finding.

**The publication embargo is not lifted, and item 4 is not closed.** The
affected board, source path, reproduction and impact stay out of this document
and every other committed one until the maintainer has triaged it and a
disclosure is agreed. What is recorded here is only that a record exists and a
report has been sent. Awaiting acknowledgement, remediation timeline, and
agreement on public disclosure; the private record carries those as they land.

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
