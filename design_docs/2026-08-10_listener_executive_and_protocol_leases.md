# Listener Executive and Protocol Leases

Design doc, 2026-08-10. Supersedes the channel-ownership clause of
[retinue-small structural decision 4](2026-07-31_retinue_small_plan.md) and
reframes [channel murmuration](2026-08-09_channel_murmuration.md); both carry
banners pointing here. The one-image ruling, the licensing edge, and the trunk
guard stand, translated below.

## The reframe

Retinue is not a protocol personality among channels. It is the board's
resident radio executive and protocol router:

```text
Retinue listen (scan plan)
  -> detect activity
  -> capture under one exact ReceiveProfile
  -> dispatch to adapter
  -> adapter speaks under a bounded radio lease
  -> Retinue resumes listening
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
| switch-by-reboot; boot-selected channel field | dies; there is no switch |
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
