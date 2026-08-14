# Program Sequencing: Deadline Order, Not Dependency Depth

Sequencing decision, 2026-08-12. Supersedes the ordering (not the lane split)
in [retinue work lanes](2026-08-09_retinue_work_lanes.md). Built from a
13-agent verification sweep that read code rather than status paragraphs:
eight domain audits, a dependency graph, a collision map, and three
adversarial critiques (solo-dev realism, hidden blockers, value ordering).
Roughly twenty-five asserted dependencies did not survive contact with the
code; the important ones are listed below.

## The verdict

Three independent critics reached the same conclusion: **more concurrency is
the wrong prescription for this program.** The diagnosed pathology is already
breadth. Nine separate audits found the same shape, a complete and tested
library with zero production call sites: `retinue::command::Verifier`,
outrider's propagation client and server, outrider's opportunistic lane,
`RatchetStore::rotate_if_due`/`encode_snapshot`/`restore`, IFAC, `profiles.rs`
`ScanPlan`, tulle's `AnnouncePacing`. The last mile that fixes each of them
(wire it, configure it, receipt it) is serial and review-heavy.

**Ceiling: two concurrent streams**, one bench and one host-side, plus a
one-shot hygiene slot that produces no new capability. Green CI is the licence
for the second stream.

## Action zero, before any gate

The first work item is not in any lane.

- **The tree is 79 dirty entries, 5 unpushed commits, 25 untracked**, and the
  untracked set includes `crates/radio-hand/src/profiles.rs` (367 lines, 3
  passing tests), `crates/retinue/src/announce_admission.rs` (471 lines), and
  every design doc and receipt from 2026-08-11 and 2026-08-12. One
  `git clean -fd` deletes the evidence base. `validation/run.py record`
  refuses a dirty worktree, so no exact-SHA evidence can be banked by any
  stream while it stays dirty.
- **CI has been red since 2026-07-19**, failing in `check` at step `Format`.
  Build, Test, Clippy, and Docs have not run in roughly three weeks. The fuzz
  job and the validation-registry job exist only in unpushed commits, so the
  human is currently the only integration test.
- **The push commits two licence violations as it stands.**
  `firmware/packages/meshtastic-t114-.../*.uf2` is a 1,467,392-byte GPL-3.0
  binary with no accompanying GPL-3.0 text and no `THIRD_PARTY_NOTICES.md`
  entry, and `announce_admission.rs` has no donor-ledger row. A licensing pass
  comes before the push, not after.

Order: licensing pass, commit the whole tree, fix `cargo fmt`, push, bump
mere's lock in its own commit.

## The deadline nobody scheduled

**ARDC intake closes 2026-09-01. That is 20 days.** Its five gates are
unstarted, the G0 pre-application email is still an unsent draft in the lane
doc, and the recorded funding position is that ARDC requires a **fiscal
sponsor**: an external party with lead time that appears in no gate inventory,
no dependency graph, and no collision map. It consumes almost no engineering
time and gates an entire payoff surface.

Same day-zero class, same reason (human latency, zero engineering days): the
**LB stack ruling** (nrf-softdevice vs trouble+nrf-sdc), which blocks a whole
lane, and **LOCK4's private disclosure record**, which is not merely unpaid
but unstarted (`design_docs/private/` does not exist), while the donor ledger
and assurance doc both declare the source lock cleared. Correct those two
sentences in the same commit.

## The reordering that matters

**LE3b is the product, and it needs none of the LE ladder.** The differentiated
claim in one sentence is: one radio catches a Meshtastic `0x2B` frame and a
MeshCore `0x12` frame through a shared detection group, with counted misses.
`profiles.rs` already carries a passing test asserting exactly that shape,
selvage pins both sync words, and the Executive already owns `apply_profile`
and `ensure_rx`. What is missing is a consumer, not a ladder. LE1 (three loops
collapsed, one of them a V4 low-power rewrite), LE2 (leases), LE4 (dispatch),
and LE5 (flock division) are all post-deadline.

**LE3's measurements do not gate on LE1 or LE2.** This was the most expensive
false ordering in the program. `profiles.rs` is radio-free and host-tested,
and the T114's session loop already runs through the Executive. Every physics
unknown that prices the entire LE ladder (CAD hit/miss, retune and apply cost,
CAD-to-RX handoff, receiver acquisition, worst-case off-time) is measurable on
the firmware that is on the bench today. Measure first, then size LE1 and LE2
against numbers instead of guesses. The same measurements are LB1's
pre-SoftDevice A/B control, taken for free.

**FT5 is the cheapest civic-relevant gate and it is startable now.** The
scaling doc's own build order says forwarder-side island policy first;
`RoutingPolicy` already carries `InterfaceSelector`, `relays_announce_from`,
and a hop ceiling, so the missing piece is a local-set key. Host-side, no
bench. It answers the question a county actually asks: will our nodes flood
the wider mesh.

**CV2 gates on FT1's configuration surface, not its hardware receipt.**
`postilion/src/lib.rs:229,247` builds `AirtimeBudget::new(60_000, 60_000)`: a
3,600,000 ms allowance in a 60,000 ms window, a 6000% duty allowance, with
announce pacing left `Unlimited`. Nothing is a slice of a budget nobody set.
Separately the board's ledger hard-resets its one-hour window
(`executive.rs:473-490`) rather than sliding, so a transmitter straddling the
boundary can spend close to 2x the intended budget against the regulatory
floor. For an amateur-radio grant application, that is the wrong pair of facts
to have on disk.

**CV1 and CV7 are post-deadline, as the civic doc's own phasing already says.**
The declared critical path (interface metadata, FT3, FT4, CV1) is
topologically correct and economically wrong: FT3 is at zero on both tiers and
FT4 needs a multi-route table restructure before any metric can select
anything. Land the interface-metadata widening as one mechanical
zero-behaviour commit while the tree is quiet, because it touches every
`InterfaceSink` impl at once and collides with murm and IFAC work later. Do
not start FT3 on the back of it.

## Corrections owed to standing docs

Found by reading code against the docs. Each is a wrong premise that
downstream work would inherit.

1. **The listener-executive doc's diagnosis is backwards.** It blames "handing
   protocols the event loop through `Channel::serve`"; `channel.rs:8-19`
   carries a section titled "Why serve takes an event rather than owning a
   loop." LE1 is not reclaiming a loop from adapters. It is collapsing three
   loops that already live outside any adapter (`channel.rs:251-306`
   `await_host`, `t114-phy/main.rs:458-560`, and `heltec-v4-phy/main.rs:380-504`,
   which bypasses the Executive entirely). The V4 half is a low-power rewrite,
   not a refactor, which the doc's sizing understates.
2. **The 2026-08-08 receive-future cancellation doc is stale and load-bearing.**
   Its status still reads "characterised, deliberately not fixed yet," while
   the arm/collect split shipped the same day (5b95ee2, 1dd95a9, 2a2a245, the
   last two recorded as proven on RF). The listener-executive doc cites the
   superseded prescription as binding. The constraint that actually binds is
   narrower: arm continuous RX once, race only `wait_for_irq`, never race the
   collect. The fix also has no acceptance receipt, which is a real evidence
   hole under the most safety-critical firmware change in the program.
3. **FS2 is a type, not a path.** `retinue::command::Verifier` has zero
   production call sites. The board's live command surfaces are
   unauthenticated: `rnode.rs:152-179` takes frequency, bandwidth, TX power,
   and radio state straight from the USB line, and `probes.rs` exposes
   plaintext `bootloader`, `crashtest`, and `crash clear`. A field node cannot
   hold an operator public key at all, since `Settings` is
   `{identity, channel, region}` and the allowlist is a RAM-only
   `heapless FixedVec`. **CV3 and CV5 do not ride FS2 unchanged**, contrary to
   what I said before the sweep.
4. **mere's DOC_README puts the constitution and tessera in `moothold`; both
   are in `gemot`.** Anyone starting the scope-artifact work from the docs
   writes into the wrong crate. My 2026-08-12 radio-scopes-as-moots note
   inherits this and needs the same correction.
5. **The partition merge rule is unreachable, not undecided.**
   `ConstitutionStore::accept` returns `StaleRevision` before insertion and is
   the sole ingestion path, so choosing between the two candidate rules changes
   nothing until the retention gate moves. `Constitution.revision` is a content
   digest rather than an ordinal, so a cold board cannot order two artifacts.
   The three-seam framing was wrong: seam 1 is downstream of a code change, and
   the real package is three rulings (retention gate, revision ordinal,
   digest/codec boundary) that Mark writes, not a stream that codes.

## Hidden blockers worth pricing now

- **Certified envelope versus regulatory ceiling.** `region.rs:73` sets US915
  `max_power_dbm: 30` and `executive.rs:34` sets `HARDWARE_MAX_DBM: 22`, so a
  US board transmits up to 158 mW. The T114's grant (FCC ID 2A2GJ-HT-N5262)
  certifies roughly 18 mW peak conducted on the LoRa band. The compliance model
  is region-shaped while the obligation is device-shaped. Fix: a per-target
  `CertifiedEnvelope { fcc_id, max_conducted_dbm, band }` and a four-way min at
  the clamp, with the FCC ID recorded in the package manifests.
- **The direct-PHY wire protocol is unversioned and fixed-shape.** Four
  separately scheduled streams want to extend it; adding two duration bytes to
  `EVENT_TX` makes an older host read the first duration byte as the next event
  marker and desync silently. Version the seam once (hello exchange with a
  version and capability bitmap, self-delimiting events) before FT1 or LE3
  writes a byte.
- **LE4 has nothing to dispatch to.** `tucket` and `sennet` are std-only, not
  linked by `radio-hand`, and have no portable twin the way outrider ships
  `src/portable.rs`. Whether adapters decode on-board is an unmade scoping
  decision sitting under four gates, and resolving it toward on-board decode
  turns the split crypto row (two of digest, sha2, aes, hmac in the lock) into
  a flash-size problem.
- **The catalog rejects the evidence that exists.** `catalog.rs:218-231`
  forbids a `partial` package from listing receipts, promotion requires https
  URLs that do not exist, and all four index entries are partial with empty
  receipt lists. Eleven physical receipts on disk cannot be recorded. Fix the
  surface before generating more evidence into it.
- **Receipt vocabulary misleads.** `linkboy verify-recovery` never writes to a
  board, yet produced a receipt reading `result: "complete"` for a run whose own
  receipt says `recovery-required`. Non-writing verifications need their own
  result vocabulary, and every receipt should name the command that produced it.
- **The eFuse burn is irreversible** and halves the freely-reflashable V4 pool
  from two to one, which is the pool every RF peer and every A/B control draws
  from. It goes last, and the burned board gets recorded in the bench
  inventory.

## Bench reality

Live enumeration: **one T114** (`VID_1915&PID_521F`, TULLE-T114-01, COM10,
Retinue 0.0.1 / v47) and **two V4s** (COM6 on Retinue, COM7 still on Prns
Hopspot 0.3.4). The T114 is the universal bottleneck for LE1 through LE5, FT1,
FT2/FS6, LB1, and G4-T114. Every RF-pair gate additionally consumes a V4 as
peer or unpatched control, so no V4-only distribution work runs while an Air
pair gate is on the air.

**COM7 is double-booked in opposite directions**: DIST4 and F7 need it left
foreign so the cross-firmware graphical restore has something to restore from,
while mere's V0 power baseline needs Retinue on it. Run the graphical restore
off COM7 while it is still foreign, which discharges the gate and returns the
board in one session.

Batch the foreign-T114 window (DIST5, PRNS-T114-RF, F7's upstream half) into
one sitting with the loader snapshot captured first and the Retinue restore run
once at the end.

## The order

**Day zero (hours, not days).** Licensing pass, commit whole tree, fix
`cargo fmt`, push, bump mere's lock. Send the three human-latency items: ARDC
fiscal sponsor conversation and the G0 pre-application email, the LB stack
ruling, the LOCK4 disclosure record. Correction pass over the five standing
docs above.

**Window 1.** Stream A (bench, owns all three boards): build v48 carrying the
bounded tables, take the FT2/FS6 flood receipt, take LE3's physics on today's
firmware and label the numbers as LB1's control, close N0-UNPLUG and the FS5
read-only flash dump. Stream B (host-side, zero bench, disjoint crates): the
signalman-desktop self-drive harness on genet's existing `HostPointer` seam,
which converts every remaining graphical receipt from the external-UIA lane
the receipts record as failing into a repeatable run.

**Window 2.** Stream A becomes the distribution bench window: the graphical
restore off COM7 while foreign, then COM7 back to Retinue. Stream B becomes
FT1's configuration surface (real duty permille, selectable announce pacing
threaded from signalman, sliding window on the board), then CV2's precedence
classes on top of it, then FT5 with a static island id.

**Window 3, the demo.** LE3b: the Executive consumes a two-ReceiveProfile scan
plan, per-profile capture and miss counters join the existing `air` probe
surface, two transmitters on the bench. That receipt is the product claim, the
county story's centrepiece, and the grant's exhibit.

**Post-deadline.** LE1, LE2, LE4, LE5, LB bring-up, FT3, FT4, CV1, CV6, CV7,
the moot seams, H4.

**Forbidden as concurrent:** any two of LE1/LE2/LE4 (LE1 deletes the trait the
others amend); any FT gate beside the CV gate that consumes it; any mere coding
stream before the three rulings exist; the entire BLE lane until the stack
ruling and LE3's control data exist; FS4's eFuse burn anywhere but last.

**One writer per hot file, enforced by name:** `executive.rs`, `channel.rs`,
`endpoint.rs`, `postilion/src/lib.rs`, `tulle/src/airtime.rs`,
`signalman-desktop/src/{state,views}.rs`. Where two gates need the same
function, land a signature-freezing commit first (behaviour identical, call
sites updated), publish the shape, then split.
