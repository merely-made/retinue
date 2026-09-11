# Listener Executive and Protocol Leases

> **Product framing superseded, 2026-09-10:**
> [current murmuration direction](2026-08-09_channel_murmuration.md#current-direction-coordinated-runtime-personalities)
> restores a configurable home, explicit excursions and a pinned personality.
> Tulle owns coordination among our implementations; radio-hand enforces the
> embedded interruption lifecycle. The mandatory resident-listener identity,
> rejection of home/pin controls and receive-driven activation narrative below
> are historical. Existing scan/lease receipts and open LE gates retain their
> measured scope. A resident supervisor is optional future integration, and
> arbitrary third-party firmware runs independently.

Design doc, 2026-08-10. Supersedes the channel-ownership clause of
[retinue-small structural decision 4](2026-07-31_retinue_small_plan.md) and
reframes [channel murmuration](2026-08-09_channel_murmuration.md); both carry
banners pointing here. The one-image ruling, the licensing edge, and the trunk
guard stand, translated below.

**Receipt status, 2026-08-20:** the LE3a/LE3b physical scan slice is complete
on one T114 with two V4 transmitters. The
[scan-physics receipt](2026-08-20_le3_t114_scan_physics_receipt.md) records two
CAD groups, three exact receive profiles, measured transition/acquisition
costs, the runtime cycle-budget refusal, the fixed `0x12`/`0x2b` miss and
capture pair, and the driver defects found on air. The final unattended
rotating executive and long-run miss-rate calibration remain outside that
receipt.

**Scope correction, 2026-08-30:** this document governs resident adapters in
one running image. The
[wall-node management plan](2026-08-30_wall_node_management_plan.md) separately
admits durable selection among verified firmware images on boards with measured
slot and rollback capacity. A boot-selected adapter channel still dies; a
rollbackable image choice does not. The shared board executive is
`radio-hand`; Retinue is its RNS adapter and router.

**Implementation planning pass, 2026-09-06:** the V4 ownership premise has
changed since the 2026-08-12 audit. `firmware/heltec-v4-phy/src/radio_owner.rs`
now has one non-copy `V4RadioOwner` for the SX1262, current radio state, face,
settings store, and live configuration quiet windows. Its quiet guard stops at
a completed-event boundary, holds the CPU awake, settles IRQ state, performs
durable/apply work, and either restores RX or resets. The V4 still has separate
boot-selected RNode and direct-modem loops, and the T114 still runs the older
`Executive` plus `Channel` session loop. LE1 therefore starts by extracting a
board-neutral interruption lifecycle from the proven V4 shape and making the
T114 scheduler use it. It does not start by replacing the V4 loop wholesale.

The [2026-09-06 coordination agreement](2026-08-09_retinue_work_lanes.md#coordinated-appliance-listening-and-observation-work-2026-09-06)
settles shared lifecycle, admission and observation decisions before code work.
Its expiry rule supersedes the initial planning suggestion of expiry plus
hysteresis: expired coverage is unusable immediately; hysteresis governs
re-admission only.

The implementation order below remains the embedded LE lifecycle sequence;
the current murmuration section defines a separate, radio-free controller slice
that can proceed before hardware integration. This sequence challenges
the older implication that the entire channel rewrite, all protocol adapters,
or emergent peer negotiation must land before a useful scheduler slice can be
tested.

## Current implementation sequence

**Foundation receipt, 2026-09-06:** step 1 has a partial implementation in
`crates/radio-hand/src/scheduler.rs`. Ten host scenarios cover static keeper
assignments, bounded leases, peer loss, deadlines, recovery holds, clock and
capacity errors, and hardware acknowledgement of restored listening. A keeper
cannot delegate its own required profile in this first model; rotating custody
and scan-slot selection remain later work. Review corrected reciprocal keeper
admission, early restore budgets, withdrawal followed by renewal, and repeated
late completion. The model reports recommendations only and has no firmware
consumer. LE1, LE2, LE4 and LE5 remain open.

1. **Land the radio-free scheduler kernel in `radio-hand`.** Keep the existing
   `DetectionProfile`, `ReceiveProfile`, and `ScanPlan` types. Add static board
   assignments, bounded lease admission/deadlines, peer liveness inputs, and a
   deterministic next-obligation decision. Drive it with a caller-supplied
   monotonic time and synthetic completed events. This slice neither touches a
   radio nor defines the shared observation vocabulary.
2. **Extract one interruption lifecycle.** Name the states needed by scan
   retunes, lease grant/revocation, live configuration writes, and sleep:
   completed event, RX owed, stopping, quiet/leased, restoring, listening, and
   reset required. Preserve the V4 rules: collect a completed frame before an
   interruption; never cancel collection; hold sleep off while state is
   uncertain; restore RX before releasing the guard; reset after an abandoned
   or failed transition whose hardware state is unknown. Adapt the current V4
   quiet window to this contract before adding another independent guard.
3. **Add a T114 radio adapter for scheduled operations.** The adapter applies
   one detection or exact receive profile, waits only at the cancellable IRQ
   boundary, completes CAD or frame collection without racing it, and returns
   a typed completed result. Retain the existing channel path behind its
   current boot selection while this adapter is exercised by a bench-only
   scheduler build.
4. **Run one board from a static listening assignment.** Admit the receipted
   two-detection/three-receive registry, then run it unattended rather than by
   host probe commands. A malformed or overfull assignment is refused before
   radio work. This closes the scheduler portion deliberately left open by the
   LE3 physical receipt; it does not yet claim flock behavior.
5. **Put one bounded talk adapter behind the scheduler.** Start with one
   transaction whose maximum TX airtime, response profile, response window,
   and total deadline are known before grant. Each transmitted frame is its
   own indivisible operation. Success, failure, or deadline restores the
   assigned listening plan. Reticulum link establishment and foreign retry
   ladders remain later adapters because they add protocol state without
   proving the lease machinery more strongly.
6. **Run the minimal two-listener assignment experiment.** Give listeners A
   and B explicit required sets and advertised cover sets. Inject frames on
   both sets at recorded offsets while A receives a bounded talk lease. B must
   capture the profile delegated from A during that interval. Then stop B's
   liveness advertisements: at the declared expiry, A must
   restore its required set and refuse a lease that would leave it uncovered.
7. **Only after recorded demand, add adaptive division.** Peer negotiation,
   demand weighting, protocol-specific participation policy, and emergent
   schedules consume the static-assignment and lease receipts. They are not
   prerequisites for LE4 or the first LE5 experiment.

The recommended first bounded Terra slice is step 1: a host-tested
`radio-hand` scheduler model with static assignments, expiring peer cover, and
lease admission/refusal. It has no hardware dependency and can establish the
hard policy cases before async radio code obscures them. Its tests should use a
recorded sequence of synthetic time, peer advertisement, lease request, lease
completion, and peer expiry rather than mirror individual methods.

## Ownership map

| Concern | Owner | Stop line |
| --- | --- | --- |
| Scheduler state, static assignment, peer-cover expiry, lease admission and deadline | `crates/radio-hand` | No protocol decoding, board registers, durable UI schema, or event wire vocabulary |
| Detection/receive/transmit radio shapes and host transport abstractions | `crates/tulle` and `crates/selvage` | No flock policy or board ownership |
| T114 SPI/IRQ/profile application and scheduled-operation adapter | `firmware/t114-phy` | No assignment decisions or protocol semantics |
| V4 radio/store custody, sleep hold, quiet-window integration, and RX restoration | `firmware/heltec-v4-phy::V4RadioOwner` | No duplicate scheduler policy; V4 scanning remains a later capability decision |
| RNS decoding, routing, link and announce obligations | Retinue adapter in `radio-hand` | No direct radio, flash, sleep, or event-loop ownership |
| Sennet and Tucket protocol state | Their adapters | No direct radio or indefinite response wait |
| Registry and participation settings projected for people | Signalman | Firmware remains authoritative about what fits and what was admitted |
| Capture, transmit, refusal, listening-interval, and interruption event vocabulary | Observation plan | This plan supplies required facts and state transitions, not the shared codec or storage format |

One implementation owner must hold each shared firmware file at a time. In
particular, changes to `radio-hand::executive`, the T114 outer loop, and
`V4RadioOwner` should be separate commits with focused consumers. The board
adapters may depend on the radio-free kernel; neither board implementation is
allowed to fork its policy.

## Decisions still required

- **Lease priority and refusal:** rule the first collision set before the talk
  adapter lands: required listening, an already-started TX, a declared response
  window, live configuration, and sleep. The safe initial rule is that an
  in-flight TX finishes; configuration waits for a completed-event boundary;
  required lone-board coverage refuses a new lease; a lease deadline ends any
  response window; sleep is permitted only while the owner reports an armed
  wake-capable receive.
- **Assignment authority:** for the first experiment, assignments are signed
  configuration supplied by the controller and admitted locally. Peer
  advertisements can reduce duplicate cover only within that configured
  envelope; they cannot add a protocol, profile, or participation level.
- **Liveness clock:** choose the monotonic unit, advertisement lifetime, and
  hysteresis rule. Expiry must be computable without wall-clock time and must
  survive missed advertisements without synchronized herd switching.
- **Coverage floor:** define `required` separately from `preferred`. A board
  may delegate preferred coverage. It may delegate required coverage only
  while a live peer explicitly covers it, and must restore it on expiry before
  granting work that conflicts with the restored set.
- **Lease cancellation:** decide which adapter states can be cleanly resumed
  after deadline and which are discarded. The executive owns the deadline;
  protocol retry state never extends it implicitly.
- **V4 role:** the existence of `V4RadioOwner` removes the old ownership
  blocker, but continuous multi-profile scan still competes with its measured
  low-power behavior. The two-listener proof stays T114-first. A V4 may later
  advertise listener, occasional listener, or lease-only talker capability.
- **Durable mutation cost:** the current V4 control carrier enters a live quiet
  window and journals authenticated counters. Scheduler configuration and
  routine peer liveness must not turn ordinary observation into repeated flash
  interruption. Durable assignment changes and ephemeral peer state need
  separate storage treatment.
- **Event contract:** the observation lane must define stable event identity,
  timestamps, bounded storage, and export. This plan requires it to represent
  assignment admitted/refused, listening interval start/end, lease
  granted/refused/expired/completed, peer cover seen/expired, return-to-listen,
  and reset-required. Firmware work should not mint competing encodings first.

## Minimal two-listener done conditions

The previous scan bench had one T114 listener and two V4 transmitters. It does
not establish that two T114 listeners are available. Inventory the bench first:
use a second T114 if available, or add a separately receipted V4 listening
adapter before claiming the two-listener run. The single-T114 scheduler proof
can proceed independently. Initial assignments need only two exact receive
profiles; the third receipted profile is a useful later load case, not a gate.

- Both listener images identify their exact build and admitted static assignment.
  The registry contains at least the receipted `0x12` SF11 and `0x2b` SF11
  receive profiles, without claiming that shared CAD decodes both
  sync words.
- Before the talk lease, each board's observed schedule matches its admitted
  required and delegated sets. The complete cycle remains within the runtime
  budget; an intentionally overfull variant is refused.
- During A's bounded lease, timestamped injections on A's delegated profile
  are captured by B. Simultaneous injections on B's other required profile
  establish that delegation did not replace B's own floor. Counts include
  injections, captures, misses, damaged frames, and unknown outcomes.
- The receipt records lease request, grant or refusal reason, planned deadline,
  actual TX airtime, response-window end, and elapsed return-to-listen. A
  deadline or injected adapter stall cannot prolong the lease.
- On ordinary completion, failure, and deadline, A resumes its full local plan
  within a declared bound measured from the terminal lease event. An unknown
  radio state resets rather than advertising restored coverage.
- When B disappears, A notices at the configured expiry. A then restores the
  delegated required profile within one
  declared scan-cycle bound and refuses conflicting talk until restoration is
  observed. B's later return does not cause both boards to abandon the profile
  in the same cycle.
- The receipt reports per-profile opportunity and capture rates for the steady
  assignment, A's lease interval, the disappearance interval, and recovery.
  Passing means the configured thresholds hold in every interval; a short
  demonstration is not generalized into a long-run miss-rate claim.
- Unplugging the controller does not stop either listener, alter the static
  assignment, or extend a lease. Host tools collect the receipt but do not
  drive the schedule.

## The reframe

Retinue is not a durable board personality among channels. `radio-hand` is
the board's resident radio executive; Retinue is its RNS protocol adapter and
router:

```text
radio-hand listen (scan plan)
  -> detect activity
  -> capture under one exact ReceiveProfile
  -> dispatch to adapter
  -> adapter speaks under a bounded radio lease
  -> radio-hand resumes listening
```

Sennet mode and Tucket mode are not durable board states. They are leases: an
adapter borrows one exact PHY profile for a transmission and any declared
response window, then the radio returns to the executive's listening plan.
The board's identity is the listener, and speaking any protocol is a bounded
excursion from it.

## Why the channel model was wrong

Decision 4 ruled "exactly one channel is active" from the observation that the
SX1262 speaks one PHY profile at a time. That conflated two timescales: the
chip enforces one profile per *transaction*, at millisecond granularity, not
one citizenship per boot. The channel model froze a per-transaction hardware
constraint into a board identity, and everything downstream (switch-by-reboot,
teardown correctness as a gate, visits as special machinery) followed from it.

**Correction, 2026-08-12, from a code audit.** An earlier draft of this section
blamed "handing protocols the event loop through `Channel::serve`". That is
wrong, and `channel.rs:8-19` says so in a section titled *Why serve takes an
event rather than owning a loop*: `serve` handles one event. No adapter ever
owned the loop. What LE1 actually collapses is **three loops that already live
outside any adapter**: `channel.rs:251-306` `await_host` (its own select3 over
host attach, RX IRQ, and heartbeat), `firmware/t114-phy/src/main.rs:458-560`
(the session loop, which does run through the Executive), and
`firmware/heltec-v4-phy/src/main.rs:380-504`, which bypasses the Executive
entirely and drives `lora` directly because, per `channels.rs:1-7`, the V4
keeps its own hand on the radio for the low-power work. The V4 half is a
low-power rewrite rather than a refactor, and LE1's sizing must carry that.
An `Executive` type already exists (`executive.rs:219-632`) but is a borrowed
hardware view with no loop of its own; it is the TX chokepoint and regulatory
floor, not yet the arbiter this doc describes.

## The boundary

**Executive owns:** the radio, the scan plan, RX, airtime and dwell
accounting, dispatch, leases, and the mandatory return to listening. It runs
the only loop.

The [receive-future cancellation findings](2026-08-08_receive_future_cancellation.md)
bind this loop, but read that doc with care: its status line still says
"deliberately not fixed yet" and its prescribed per-task Embassy restructure
was **superseded the same day** by the arm/collect split (5b95ee2, then
1dd95a9 and 2a2a245, both recorded as proven on RF). The restructure is not a
prerequisite for LE1. The constraint that actually binds is narrower: arm
continuous RX once, race only `wait_for_irq`, never race the collect.

**Adapters own:** protocol knowledge and bounded protocol state: decoding,
encoding, retry/session state, and pending actions. They never own the radio,
the event loop, or an unbounded wait.

```rust
// Illustrative only, not implementation-ready.
pub trait ProtocolAdapter {
    /// Exact receive profiles this adapter can decode (registry subscription).
    fn receive_profiles(&self) -> &[ReceiveProfileId];
    /// Classify and decode a captured frame; produce bounded actions.
    fn ingest(&mut self, frame: &Frame) -> AdapterActions;
    /// Encode a pending transmission and declare its lease terms:
    /// profile, TX airtime, worst-case response window.
    fn next_tx(&mut self) -> Option<LeaseRequest>;
}
```

There is no `start`/`serve`/`stop`. An adapter never holds the radio; it holds
a lease, and lease revocation replaces channel teardown. The host-modem
personality (RNode) survives as an **exclusive compatibility mode**, not an
ordinary standing lease. An RNode host may select a profile and wait
indefinitely, which suspends the resident-listener guarantee by design.
Signalman and the package catalog must say so plainly. A future bounded RNode
transaction adapter may coexist with the scan plan, but today's host-controlled
modem cannot be smuggled into the lease model as a lease that never ends.

## The lease contract

- An adapter's `LeaseRequest` declares its worst-case window up front. Leases
  are not always short: a Reticulum link establishment is multi-round-trip and
  a Meshtastic implicit-ACK retry ladder runs seconds. Declared, they are
  schedulable; undeclared, they are theft.
- Success, failure, or deadline revokes the lease. The deadline is a runtime
  assertion, loud on divergence: a hostile or malformed peer frame must not be
  able to prolong a lease past its declared window.
- The executive owns preemption policy: what it refuses to grant, and what it
  cuts, when a higher-priority obligation needs the radio.
- An in-flight LoRa transmission is not preemptible. Preemption may refuse the
  next frame or end a declared response window; a multi-frame exchange is a
  succession of grants, never one secretly unbounded transmission.
- Lease airtime and listening dwell are two columns in the one FT1 ledger
  ([mesh scaling](2026-08-09_mesh_scaling_and_asymmetric_routing.md)).

## Detection profiles and receive profiles

The registry has three radio shapes rather than one overloaded PHY signature:

- A **DetectionProfile** names frequency, SF, BW, and the CAD parameters. CAD
  can report LoRa preamble activity only for the configured frequency, SF, and
  BW. It does not identify a protocol or prove that the packet can be decoded.
- A **ReceiveProfile** names one DetectionProfile plus the exact sync word,
  header mode, IQ polarity, CRC/preamble behavior, and every receive-side
  modulation parameter required by the driver, including CR/LDRO where
  applicable. This is the minimum unit the SX1262 can actually capture.
- A **TransmitProfile** supplies the exact TX modulation and packet parameters,
  power, ramp, and regulatory facts for a lease.

Adapters subscribe to ReceiveProfiles, many-to-many. Several ReceiveProfiles
may share one DetectionProfile. That lets them share a cheap CAD observation,
but not a receive window: after CAD, the executive still has to configure one
exact sync word and packet profile before the SX1262 can deliver a frame.
Meshtastic `0x2B` and MeshCore `0x12`, when otherwise configured alike, are
therefore two capture slots under one detection group. They cannot be
classified after one fixed-sync hardware receive.

**The scan budget is physics, not policy.** A frame is caught only if a slot
for its ReceiveProfile overlaps enough usable preamble for acquisition. A
useful guaranteed-catch inequality must include the profile's worst-case
off-time, retune/apply cost, CAD duration, CAD-to-RX handoff, receiver
acquisition, and a measured margin. Merely making the scan cycle shorter than
the nominal preamble is insufficient.

The current vendored `lora-phy` SX126x path uses eight CAD symbols, exits CAD
to standby (`CAD_ONLY`), and then requires a separate RX operation. That is the
baseline to measure before considering `CAD_RX` or another handoff. Shape of
the numbers (to be measured, not trusted): Meshtastic LongFast (SF11/BW250,
16-symbol preamble) offers ~131 ms of nominal preamble; an eight-symbol CAD at
that profile already costs roughly 65.5 ms before retune and RX acquisition.
Short-preamble, high-rate profiles structurally suffer. Semtech's guidance
that CAD requires the correct SF/BW
([Semtech CAD FAQ](https://www.semtech.com/design-support/faq/faq-lora/P20))
is the constraint the registry budget lives under.

Consequences:

- A lone board covers several ReceiveProfiles with an honest, measured miss
  probability. The firmware asserts its scan budget at runtime: if the
  configured registry cannot meet its acquisition budget, that is a loud
  configuration diagnosis, never a silent degradation. Profiles in the same
  detection group still consume separate capture dwell when their sync words
  or packet parameters differ.
- Continuous coverage of the whole registry is what a *retinue* of boards is
  for. Murmuration is ReceiveProfile division among flock members: coordinated
  ears, not coordinated absences. One board speaks under lease while its
  companions keep listening.

## Participation levels, and where the trunk guard lands

Hearing a frame is not citizenship. Rebroadcast duty, DM delivery, and ACK
listening all cost real dwell. Each adapter is granted a participation level:

- **monitor**: capture and classify only;
- **respond**: transmit when addressed, decline relay duty (the mute-client
  posture the murmuration doc already ruled);
- **member**: full protocol duty, which requires dwell that competes with the
  rest of the registry.

The trunk guard relocates from "which channel owns boot" to "which
participation levels the executive grants." A lone board defaults foreign
adapters to monitor or respond. Full membership of a foreign mesh is a duty
delegated to a flock member, never a mode the trunk drifts into. The board is
the trunk; adapters are how branches get heard.

Signalman edits the registry and participation levels per board. A
Retinue-backed package declares its installed adapters and recognizable
ReceiveProfiles; neighbor capability announcements let the flock divide
coverage.
Stock third-party firmware is marked `exclusive` in the catalog: flashing it
replaces the executive until a restore.

## Stateful sessions weight the plan

"Return to listening" is not return to a neutral state. An established RNS
link imposes standing keepalive obligations, so the scan plan weights
ReceiveProfiles by session state rather than rotating flatly. The murmuration
doc's dwell-versus-keepalive open question becomes a scheduling input here:
announce absence, tighten windows, or both.

## What dies, what survives

From decision 4:

| item | status |
| --- | --- |
| exactly one active channel; `start`/`serve`/`stop` | dies; adapters + leases |
| switch-by-reboot; boot-selected adapter channel field | dies as the resident adapter scheduler; verified image selection is a separate wall-node capability |
| channel selector UX | becomes participation-level config in signalman |
| one GPLv3 image, MPL crates, licensing edge | stands unchanged |
| trunk guard | stands, relocated to participation levels |
| flash residency of several protocol stacks | stands unchanged |

From murmuration, the design rules survive translated and the doc remains
their authority, read through the lease model:

| rule | translation |
| --- | --- |
| 1 dwell metering | scan-slot budget; same FT1 accountant |
| 2 coverage + hysteresis + floor | ReceiveProfile division; lone board keeps its required listening set |
| 3 beacon control plane | "which ReceiveProfiles I cover, during which window" |
| 4 persona disjointness, schedule as cover | strengthened: the scan plan runs regardless of traffic, so decorrelation is free |
| 5 visiting node = border gateway | applies to leases unchanged |
| 6 visit = store-and-forward window | the lease TX window |
| 7 honest abbreviated citizenship | the respond participation level |

CM1 (teardown-correct hot switch) is absorbed: with adapters that never own
the radio there is nothing to tear down, and its invariant surface becomes
LE2's lease-revocation assertions. CM2 through CM5 carry with their meaning
shifted from visit schedules to scan plans and coverage division.

## Proof ladder

LE numbering, clear of N/CM/FT/FS/H. Each gate is a done condition.

**LE1: The boundary exists.** The adapter trait lands in `radio-hand`; the
executive owns the single loop; the existing modem and node personalities are
re-expressed behind the boundary (the modem explicitly as exclusive
compatibility mode).
*Validation:* existing acceptance receipts re-run and pass on the T114 under
the executive; behavior parity, counted blocks per the RF receipt rule.

**LE2: Leases revoke.** Timeout, success, or failure revokes; the executive
resumes the scan plan within a measured deadline, asserted at runtime.
*Validation:* a hostile or malformed frame stream cannot prolong a lease past
its declared window; the return-to-listen deadline holds under injected
adapter misbehavior. Absorbs CM1.

**LE3: Detection and capture are both honest.** LE3a registers at least two
DetectionProfiles and measures their CAD hit and miss behavior. LE3b registers
at least two ReceiveProfiles, including two that share a DetectionProfile but
use different sync words, and captures each only while its exact receive
configuration is active. There is no claim that one hardware RX decodes both.
*Validation:* measured off-time, retune, eight-symbol CAD, RX handoff, and
acquisition against each profile's usable preamble; the scan-budget assertion
fires on a deliberately overfull registry; per-ReceiveProfile miss rate tracks
the predicted capture dwell. A fixed `0x12` receive window demonstrably misses
the otherwise-matching `0x2B` frame, then the `0x2B` window captures it.
*Physical slice:* receipted 2026-08-20 on the T114. D1/D2 CAD behavior, exact
SF11 `0x12`, SF11 `0x2b`, and SF9 `0x2b` captures, the 900 ms cross-sync miss,
and a 2840/3000 ms admitted cycle are measured. Scheduler-wide probability
calibration remains a later runtime receipt rather than an inference from the
short bench.

**LE4: Dispatch and bounded talk.** A captured frame routes to its adapter;
the adapter transmits under lease and receives its bounded acknowledgement;
the executive restores the full scan plan.
*Validation:* end-to-end receipt on T114 and V4 with two different foreign
ReceiveProfiles, one per board, per the two-board proof shape.

**LE5: The flock divides.** Two boards divide a registry; one retains coverage
of the other's ReceiveProfiles while that one holds a lease.
*Validation:* frames injected on the covered ReceiveProfile during the peer's
lease are captured; the anti-herd rule holds (a lone board never drops its
required listening set); coverage matches the divided plan within tolerance.

CM3 (beacon schedule advertisement) and CM5 (metered dwell) follow LE5 under
their existing definitions, translated. Emergent scheduling still waits on
logged demand, per murmuration rule 2.

## Open questions

- **V4 low power.** A continuous scan plan is in direct tension with the V4's
  light-sleep, hand-on-radio path. LE gates are T114-first; whether the V4
  hosts a reduced scan plan or remains a lease-only talker is open (sharpens
  the murmuration doc's V4 question).
- **Regulatory.** The FCC 15.247 retuning question narrows: RX-side retuning
  is scanner behavior, not an emission, and TX stays on one profile per lease,
  unchanged from today. The remaining question is whether lease-driven TX
  across profiles over time reads as frequency hopping; answer before a sold
  unit ships, per the [FCC reselling doc](2026-07-20_fcc_reselling_flashed_radios.md).
- **Persona derivation.** Per-adapter or per-ReceiveProfile identity derivation
  (one hardened root versus independent roots) carries over from murmuration
  unchanged.
- **Preemption policy.** What the executive refuses next when obligations
  collide (standing RNS keepalive versus a foreign response window) needs a
  ruling before LE4 hardens. An in-flight frame remains indivisible.
- **Same-detection scheduling.** Whether one CAD hit should trigger a preferred
  ReceiveProfile, a short rotation across every compatible sync word, or a
  flock capability hint needs measured receipts. CAD alone cannot choose.
