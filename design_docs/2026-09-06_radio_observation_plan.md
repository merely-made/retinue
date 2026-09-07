# Radio observation and Signalman availability plan

**Status (2026-09-06): O1 partial.** The allocation-free codec and literal wire
fixtures are implemented and host-tested. Firmware recording, collection,
Signalman bundle/replay and the availability view remain unimplemented. Existing
counters and host events are findings, not substitutes for this plan's receipts.

## Purpose

Retinue needs one shared account of when a board was listening, transmitting,
unavailable, or unable to accept work. The immediate consumers are the wall
node, the listener executive, and Signalman. The later survey-and-placement
lane can combine those facts with positions, but it must not turn a terrain
prediction into measured radio evidence.

The useful first surface is an **availability timeline**: one row per board,
with exact listening intervals, transmit and quiet intervals, sleep, captures,
and refusals. It should answer three ordinary questions without interpreting a
protocol packet:

1. What was this radio available to hear?
2. What occupied it when it was unavailable?
3. Did another board cover that interval?

This plan does not record packet bodies, decode foreign protocols, create a
general logging framework, or make the control-status command a telemetry
transport. RNS remains a black-box interoperability peer under the repository's
source policy. GPL or AGPL implementation source is outside this work.

## Findings (2026-09-06)

- `radio-hand::Executive` already owns the T114 radio and keeps bounded
  `AirDiag` counters for RX setup, good and damaged receive, transmit outcomes,
  CAD, and unattended waits. Its LE3 probes also count observations by stable
  scan-profile id. Counters establish totals but cannot reconstruct intervals,
  ordering, overlap, or the cause of lost listening time.
- `firmware/t114-phy` has one long-lived `Executive`; its main loop races host
  input, RX interrupt, and heartbeat, then finishes a radio collection before
  dispatch. This is the clean first owner seam for observation emission.
- `V4RadioOwner` separately owns the V4 radio lifecycle and implements
  `QuietWindow`. Configuration work enters that window only at a completed
  event/frame boundary. Observation must be emitted inside this owner rather
  than inferred later from USB traffic.
- Direct-PHY currently reports RX, TX result, configuration, diagnostics, and a
  UI snapshot through tagged events. `tulle::direct_phy` and
  `direct_phy_serial` already discard diagnostics under pressure rather than
  blocking radio traffic. They provide a carrier pattern, not an observation
  schema.
- `postilion::Event` is application-level: peer appeared, authenticated message,
  or dropped message. Its management snapshot is a read-only host projection.
  Neither should be enlarged into a physical-radio history.
- `radio-face::LocalStatus` holds current status and cumulative frame counters;
  `HostSnapshot` holds short-lived host truth and one display event. These are
  bounded face models, not an event ledger.
- A verified WN1 `Status` request consumes the signed request counter by writing
  and reading back the durable control journal inside a live radio quiet
  window. High-rate or periodic observation reads on that path would themselves
  create listening gaps and flash wear. Observation collection needs a separate
  read-only path.
- Signalman's management projection already distinguishes captured host time
  from observation age. This is a useful precedent for keeping device uptime
  distinct from host civil time.

## Ownership and truth

`radio-hand` owns the observation vocabulary, fixed encoding, sequence rules,
and bounded recorder because it owns radio work on both boards. A channel or
protocol adapter may supply a bounded work id and assignment id, but it cannot
claim that listening or transmission began. Only the radio owner emits those
facts at the hardware transition.

Firmware owns `boot_id`, monotonic `uptime_ms`, and per-boot `sequence`. These
are the authoritative ordering coordinates. The host records when it received
each observation as `received_unix_ms` and may estimate a boot-to-civil-time
mapping. That mapping is explicitly host evidence and may be revised after a
clock correction. Firmware does not invent UTC from uptime or from the last
GNSS sentence.

Signalman owns durable capture files, retention settings, position joins, and
projections. Postilion may expose the source as a station attachment, but it
does not reinterpret physical events as peer or message events. `radio-face`
may later summarize the newest observation; it does not own or retain history.

The survey-and-placement plan consumes a versioned Signalman observation
bundle. GPX and position samples are joined by host time with an exposed error
bound. Predicted links and measured captures remain different evidence classes.

## Shared contract

The coordinator's [2026-09-06 shared decisions](2026-08-09_retinue_work_lanes.md#shared-decisions-settled-before-implementation)
settle the initial widths, boot failure, evidence and collection rules below.
Implementation progress is recorded at the foot; proposed consumers remain open.

The first compile-ready shape should live in a new
`crates/radio-hand/src/observation.rs` module and remain allocation-free. Names
below illustrate the required facts. The implemented codec and literal fixtures
settle the exact names and encoded maximum before firmware adoption.

```rust
// Illustrative, not compile-ready.
struct ObservationV1 {
    boot_id: u64,
    sequence: u64,
    uptime_ms: u64,
    kind: ObservationKindV1,
}

enum ObservationKindV1 {
    ListeningStarted { assignment: u16, profile: u8 },
    ListeningStopped { assignment: u16, reason: StopReason },
    RxCaptured { profile: u8, length: u16, rssi_dbm: i16,
                 snr_tenths_db: i16, capture_tag: u32 },
    RxDamaged { profile: u8 },
    TxStarted { profile: u8, length: u16, work: u32 },
    TxFinished { work: u32, outcome: TxOutcome },
    WorkRefused { request: RequestKind, reason: RefusalReason,
                  work: u32 },
    QuietStarted { cause: QuietCause },
    QuietStopped { cause: QuietCause },
    SleepStarted,
    SleepStopped { cause: WakeCause },
}

// Cursor loss is a separate record, not a source event with a reused sequence.
struct GapV1 { boot_id: u64, first_missing: u64, count: u64 }
```

An interval is reconstructed from paired starts and stops. A new start closes
an unmatched interval at the new event and marks it incomplete. Boot change,
disconnect, or a `Gap` also leaves an interval incomplete; Signalman must show
that uncertainty rather than drawing a continuous bar.

`profile` identifies an exact, versioned PHY profile registry entry. It is not
only a sync word. `assignment` identifies the scheduler's coverage duty.
`work` correlates a bounded request with its transmit or refusal and has meaning
only within one boot. `capture_tag` correlates host-side optional frame capture;
it is not a packet digest and does not prove packet identity.

The minimum refusal registry distinguishes missing region, duty budget spent,
channel busy, invalid profile, conflicting lease/assignment, radio fault,
storage/control quiet window, and power policy. Unknown numeric values survive
decode and export so newer firmware remains inspectable by an older host.

The codec has an explicit version byte, fixed endian, length, and checksum. It
uses the existing one-byte profile-id space with the source registry recorded
by the host. The checksum detects corruption and is not device authentication.
It rejects truncated and overlong records, preserves unknown kinds as bounded raw
records, and defines a maximum record size small enough for both board rings and
the existing USB packet discipline. Human text is a projection and never part
of the device record.

## Bounded recording and collection

Each board begins with a fixed-capacity RAM overwrite ring owned by the radio
owner. Recording is synchronous, allocation-free, and cannot wait for USB,
flash, a lock held by a channel, or a host acknowledgement. When capacity is
exhausted, the oldest records are replaced. The next readable record is a
synthetic `Gap` covering the lost sequence range. Saturating counters retain
`recorded`, `overwritten`, and `encode_failed` totals for bench diagnosis.

Persistent device history is deferred. If field evidence later requires it,
it gets a separate wear-budgeted store and partition. It must never share the
durable control journal, settings A/B slots, announce reservation, or their
quiet-window transaction. Power loss may erase undrained RAM observations; the
boot id and sequence discontinuity make that loss visible.

Collection is a separate cursor-based, read-only diagnostic stream. A request
names `(boot_id, after_sequence, max_records)`. The response returns bounded
records plus the oldest and newest available sequence. Reading neither advances
a durable replay counter nor mutates the ring. Re-reading the same cursor is
idempotent while the retained snapshot is unchanged; continued recording can
advance the oldest available record and produce a gap. A slow or absent host
cannot stall the executive.

The first carrier is attached USB on the direct-PHY diagnostic/event stream.
It must have its own tag and decoder path so ordinary RX/TX framing remains
compatible. BLE, Reticulum, and network collection are later carriers and need
an explicit device-authentication ruling before their evidence is called
device-authenticated. USB attachment alone supports a local bench or survey
claim, with carrier provenance retained in the bundle.

Signalman drains opportunistically and writes append-only bundle segments with
an atomic manifest update. Retention is a user setting expressed as both byte
ceiling and age ceiling. On host storage pressure it stops durable capture and
surfaces the gap; it does not feed back into radio scheduling. Packet-body
capture, if enabled later, is a separate opt-in file keyed by `capture_tag` with
its own smaller retention and disclosure setting.

Collection cost is measured as part of the feature. Receipts report bytes per
observation, CPU time to record and encode, ring capacity in seconds under the
test load, USB drain occupancy, overwritten count, and added radio-unavailable
time. The acceptance ceiling is zero quiet-window entries and zero control
journal writes caused by observation reads.

## Signalman projection and export

The first view is **Radio availability**. Its horizontal axis is civil time
where a host mapping exists and boot uptime otherwise. Each board row shows:

- listening intervals labelled by assignment and exact profile;
- transmit, control/storage quiet, and sleep intervals as distinct occupancy;
- receive captures and damaged frames as points on the active listening bar;
- refused work at the request time, with the reason available on inspection;
- gaps and unmatched interval edges as uncertainty, never as idle time.

A compact summary above the selection reports observed duration, listening
coverage by profile, occupied duration by cause, capture count, refusal count,
and missing-record count. These are calculated from the selected evidence and
carry the bundle and board ids used.

The versioned export bundle contains device identity as observed by the
carrier, boot ids, raw observation records, host receive timestamps, profile and
assignment registries, carrier provenance, and optional position-source
references. It does not contain prediction output. GPX remains an imported
source artifact with its original timestamps; Signalman stores the join rule
and error bound rather than rewriting positions into device facts.

## Implementation sequence

### O1. Contract, codec, and fixture replay

Add the allocation-free types and codec to `radio-hand`, plus literal byte
fixtures covering every initial kind, unknown-kind preservation, sequence exhaustion,
truncation, and a gap. Add a host-side bundle schema and fixture replay in
Signalman. A generator may produce fixtures, but tests pin literal bytes rather
than encode and decode the same in-memory value.

**Done conditions:** both embedded targets compile with the module; codec tests
prove the encoded ceiling and forward-compatible unknown handling; Signalman
replays a literal mixed-boot fixture into complete and incomplete intervals;
the bundle documents device, host-time, and carrier evidence separately.

This is the minimal early Luna-sized slice. It touches no radio loop and can
land after the semantic rulings below.

### O2. Recorder and deterministic timeline model

Implement a const-capacity overwrite ring in `radio-hand`, including synthetic
gaps and recorder counters. Implement Signalman's pure timeline reducer and
summary calculations against recorded fixtures.

**Done conditions:** planted overflow produces the exact missing sequence
range; disconnect, gap, restart, unmatched stop, and repeated start remain
visibly incomplete; property tests or exhaustive small-ring tests show
recording never exceeds capacity; projections do not infer listening between
unknown edges.

### O3. T114 owner emission and USB drain

Instrument `Executive` at successful hardware transitions and typed refusal
sites. Add a bounded direct-PHY observation request/response tag in firmware and
Tulle. Drain from Signalman without changing the application-level
`postilion::Event` vocabulary.

**Done conditions:** a host fixture and one physical T114 run show listen start,
capture, transmit interruption, return to listen, and a planted refusal in
order; detaching the host leaves radio behavior unchanged; an intentionally
undersized ring exports an explicit gap; repeated collection returns identical
records; observation reads cause zero flash writes and zero quiet windows.

### O4. V4 owner emission and cost receipt

Instrument `V4RadioOwner`, including every successful quiet-window entry/exit,
sleep edge, transmit outcome, receive, and refusal. Reuse the contract and
carrier semantics while preserving V4's existing command demultiplexing and
low-power lifecycle.

**Done conditions:** a physical V4 run distinguishes listening, control quiet,
transmit, and sleep; an interrupted or failed quiet exit remains visible; the
signed status counter and control journal remain byte-for-byte unchanged by
observation draining; the receipt reports recording and collection costs and
shows no missed ordinary direct-PHY frame attributable to draining under the
declared load.

### O5. Availability view and export

Wire the reducer into the Signalman desktop projection, add retention settings,
and export the versioned observation bundle. Join a supplied GPX fixture only
at the consumer boundary.

**Done conditions:** the headed view renders two boards with complementary
listening, one transmit gap, one refusal, one reboot, and one overflow gap; the
same bundle reloads to the same intervals and totals; disabling durable capture
stops host writes while live rendering continues; exported measurements remain
distinguishable from predicted links and imported positions.

### O6. Murmuration acceptance

Use the shared observations in the two-listener experiment rather than adding
scheduler-specific logging. Static assignments and peer disappearance come
from the listener-executive plan; this phase only establishes what happened.

**Done conditions:** one recorded run shows which exact profile each board was
assigned, every interval in which either board transmitted or was unavailable,
whether the other board still listened, capture/miss evidence, and recovery
after one listener disappears. Every coverage percentage reports missing
observation time separately.

## File ownership during implementation

| Owner | Files | Stop line |
| --- | --- | --- |
| Observation contract | `crates/radio-hand/src/observation.rs`, its tests, and the one `lib.rs` export | Owns schema, codec, ring, sequence rules; does not edit radio transitions |
| T114 integration | `crates/radio-hand/src/executive.rs`, `firmware/t114-phy/src/main.rs`, probe/host glue | Emits owner facts; does not change the schema while V4 work is active |
| V4 integration | `firmware/heltec-v4-phy/src/radio_owner.rs`, `main.rs`, `channels.rs` | Emits owner facts and preserves control/power lifecycle |
| Carrier | `crates/tulle/src/direct_phy.rs`, `direct_phy_serial.rs`, firmware event constants | Moves bounded bytes; does not interpret intervals or persist them |
| Signalman model/export | new files under `apps/signalman/src/observation/` and focused tests/fixtures | Adds host time, retention, bundle, reducer; does not alter `postilion::Event` semantics |
| Signalman desktop view | the external `signalman-desktop` workspace after its current path/pin is re-established | Projects the model; no firmware or schema ownership |
| Survey and placement | its own plan and Signalman consumer files | Imports bundles and GPX; does not rewrite raw observation facts |

One agent owns each shared firmware file at a time. The contract lands before
either firmware integration. T114 and V4 integrations may proceed in parallel
only after the codec fixture is frozen; carrier constants and Signalman decoder
remain one owner's change until both boards replay the same fixture.

## Initial semantic questions (settled 2026-09-06)

The questions below record the Sol pass. The subsequent shared-decision
section in the work-lane document rules them before O1 dispatch; its choices
supersede the original recommendation here.

1. **Boot identity:** whether `boot_id` is fresh random data every boot, a
   crash-monotonic reservation, or a tuple derived from reset facts. Random is
   cheapest but needs a defined entropy-failure behavior.
2. **Sequence width and uptime width:** accept per-boot `u32` sequence wrap and
   `u64` milliseconds, or pay a different encoded budget. Wrap comparison must
   be uniform across firmware and host.
3. **Capture correlation and privacy:** keep `capture_tag` as a local opaque
   correlation only, or admit a truncated packet digest. A digest makes field
   correlation easier but creates durable traffic fingerprints.
4. **Evidence authority:** which carriers can authenticate the board, and how
   Signalman labels attached-USB evidence before board-signed responses exist.
   This cannot be inferred from controller authentication.
5. **Quiet causes:** whether configuration flash, settings, announce
   reservation, recovery, display/power work, and scheduler retune need distinct
   public causes or a smaller stable registry plus implementation detail.
6. **Default retention:** the default byte/age ceilings for desktop capture and
   whether packet-body capture is absent or merely disabled in the first bundle
   version.
7. **Position join:** the maximum accepted host-clock error for presenting a
   capture on a GPX segment, and how a corrected clock mapping versions prior
   joins.

The settled implementation uses a nonzero random boot token or visible
observation disablement, `u64` sequence and uptime without wrap, opaque capture
tags, metadata-only v1, local-carrier USB evidence, compact extensible quiet
causes, and explicitly started host capture with editable byte/age limits.
Hardware entropy and remote-carrier authentication remain later integration
gates. Do not turn entropy failure into a common zero boot identity.

The survey plan owns GPX join settings and clock-error admission. A collector
must measure its clock uncertainty before choosing its default join window;
that is not a prerequisite for the codec or offline fixture replay.

## Risks and contradictions

- Observation changes the system it measures. Even RAM writes and USB drain
  consume time. Owner-site timestamps and a cost receipt are required; host
  inference from existing frame events is insufficient.
- An overwrite ring preserves bounded operation by losing history. A visible
  `Gap` is part of the contract, not an error Signalman may hide or interpolate.
- `TxStarted` without `TxFinished` can mean reset, fault, or loss. The reducer
  leaves it open through the boot edge and does not invent airtime.
- Listening is exact-profile availability, not successful reception. A quiet
  interval without captures does not prove absence of transmissions.
- A receiver's capture does not prove a sender's identity. Protocol-level
  authentication remains with Retinue/Postilion; placement evidence may use a
  capture correlation without promoting it to a peer fact.
- Firmware persistence would compete with the behavior under observation and
  with flash endurance. It stays deferred until an unplugged field receipt
  proves the RAM-only boundary inadequate.
- Current V4 and T114 ownership paths differ. The common contract should expose
  that difference through causes and incomplete intervals rather than forcing
  both boards into a false common event loop.

## Progress

- **2026-09-06:** Sol planning pass completed; coordinator settled shared
  lifecycle and observation decisions and dispatched the bounded Luna codec
  foundation. O1 remains open until target checks and the host replay consumer
  also exist. No firmware or carrier has been changed by this planning pass.
- **2026-09-06, codec foundation:** `radio-hand::observation` now encodes bounded
  big-endian v1 Event and separate Gap records with CRC-32/IEEE. Maximum record
  size is 64 bytes, including up to 30 opaque bytes for an unknown event kind.
  Known reason values decode canonically; future values remain inspectable.
  Root review fixed the unknown empty-payload case and the future-event framing
  so opaque payload length comes from the outer record. Nine codec tests cover
  all eleven initial kinds, literal expected bytes in both directions, the
  maximum-size future event, a literal gap, malformed/truncated records,
  nonzero identity/sequence and range bounds. Eleven event literals were also
  independently checked with Python `struct` and `zlib.crc32`. This proves a
  codec, not source-clock monotonicity enforcement, RAM recording or device
  authentication. O1 remains partial without its host replay consumer.
- **2026-09-06, final validation:** all 212 focused `radio-hand` tests passed
  after review. Allocation-free library checks passed for ARM
  `thumbv7em-none-eabihf` and Xtensa `xtensa-esp32s3-none-elf`; firmware images,
  emission and physical acceptance remain unverified. Focused Clippy remains
  red on six warnings in unchanged commissioning and position-disclosure code;
  the new modules have no remaining reported warnings. Exact scope is recorded
  in the shared work-lane progress receipt.
