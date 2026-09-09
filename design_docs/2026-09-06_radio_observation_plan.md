# Radio observation and Signalman availability plan

**Status (2026-09-09): O1/O2 complete in software; O3 refusal and zero-flash-write checks now verified.**
T114 owner emission, bounded read-only direct-PHY collection and a finite
Signalman capture command now exist. T114 receive, TX, cursor loss, repeated
reads, pressured-session reconnect, power-cut history reset, retune refusal and
zero observation-induced NVMC mutations have physical evidence. Detailed CPU,
IRQ/FIFO and USB timing costs remain unmeasured. V4 emission, durable capture/export and the availability
view remain open. Existing counters and host events do not substitute for the
physical receipts in O3-O6.

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

Collection is a separate cursor-based, read-only diagnostic stream. The first
carrier fixes `max_records` at one; a request names `(boot_id, after_sequence)`.
The response returns bounded
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

The implementation keeps the recorder under
`crates/radio-hand/src/observation/recorder.rs` and the host model in
`apps/signalman/src/observation.rs`. A drain borrows the recorder, so its retained
snapshot cannot change during iteration. Its record limit includes the synthetic
gap, and the returned cursor advances to the last represented event sequence,
including a gap's final missing sequence. A one-record request can therefore
make progress without allocating a second ring-sized buffer. The borrow must be
short-lived by the eventual owner: it must not span an asynchronous USB write.

Replay must match the stop's assignment, work id or cause to its open start.
An unexplained sequence jump, contradictory occupancy, unknown future event,
disconnect or restart breaks continuity. Only matched complete intervals enter
duration totals; incomplete intervals and missing events remain separately
inspectable. Host receive time is retained without being substituted for a
missing device timestamp. Equal profile ids in different registries do not
establish equal radio settings.

The v1 host bundle is currently an in-memory schema, not a disk-format or
retention implementation. Its constructor accepts one opaque device association,
collector carrier label, immutable versioned exact-profile definitions and
caller-selected payload-byte/entry ceilings. It checks metadata before copying
and admits borrowed wire records before allocating their retained copies. Fields
are private and exposed read-only; disconnects also consume the capture budget.
Carrier provenance makes no board-authentication claim. Profile ids absent from
the supplied registry are refused; a changed registry requires a new bundle.
Known event kinds with future reason values remain inspectable.

Replay preserves source events, missing ranges and incomplete interval reasons.
Exact previously retained events are idempotent even when re-read after a boot
change; unseen backwards events and conflicting sequences are refused. Overlap
between an overwrite gap and already retained records counts only the still
missing tail. A capture contradicting the active profile or occupancy breaks its
interval. Complete duration totals use only matched starts/stops; missing records
and incomplete intervals are separate counts. Quiet duration retains its cause.
This proves source-event accounting, not physical reception or remote identity.

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
| Signalman model/export | `apps/signalman/src/observation.rs` and focused tests/fixtures; future persistence children | Adds host time, bundle and reducer; retention/export remain future work and do not alter `postilion::Event` semantics |
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
- **2026-09-06, recording and replay:** O1/O2 software acceptance completed.
  Terra implemented the borrowed RAM recorder; Luna supplied the independent
  mixed-boot literal fixture; the coordinator implemented and reviewed the
  bounded Signalman bundle/reducer and consumer tests. Nine recorder tests cover
  empty/zero-capacity rings, every cursor and page limit through repeated wraps
  at capacities one through three, explicit overflow, monotonic uptime,
  exhaustion and saturation. Fourteen host tests cover literal replay,
  recorder-to-wire-to-host pagination, gaps, reboot/disconnect, conflicting
  occupancy/ids, duplicate and backwards records, admission, future values and
  summaries. The final combined Signalman/radio-hand run passed 259 tests. Embedded
  library checks pass for ARM and Xtensa. Firmware images, live radio timing,
  USB drain cost and journal-write measurements remain O3/O4 receipts.
  Signalman all-target Clippy passes with `--no-deps -D warnings`. With
  dependency linting enabled it stops on the same six warnings in unchanged
  commissioning/portable-first-write and position-disclosure code. Formatting
  and diff whitespace checks pass; this does not reclassify repository-wide CI.

### 2026-09-07 O3 software slice

T114 borrows one `OwnerObservations` with a 32-record RAM ring and at most 16
exact PHY definitions per boot. A fresh nonzero hardware-RNG token names the
boot; initialization failure makes discovery report `Disabled` with boot zero.
Profile or work-id exhaustion disables recording. This first carrier reports a
generic disabled status, not a detailed failure reason. Existing profile entries
remain readable after recording disables so retained history stays interpretable.

The Executive records confirmed RX arming, collected good/damaged frames,
transmit starts/completions, and refusals. Assignment zero denotes the current
unscheduled owner; capture tag zero does not correlate packet bodies. New driver
callbacks witness the existing successful standby and transmit commands without
adding SPI work. Retune, CAD, storage entry, and radio errors conservatively emit
`ContinuityLost` (v1 event tag 11) when an interval is open. This records lost
certainty, not a measured quiet interval. Signalman leaves its end unknown and
excludes it from complete duration totals. Sleep and V4 quiet-window emission
still need their respective owners.

Direct-PHY command `0x04` is a zero-delimited 46-byte command, with a request id
and hex-encoded bounded fields. Response `0x86` has a length and CRC-32/IEEE;
its maximum is 136 bytes, including at most one raw 64-byte event or gap.
Discovery `(boot=0, cursor=0)` reads bounds and profile count. Subsequent cursor
and profile requests name that boot. The pure reply builder borrows owner RAM
without access to the radio, clock, control runtime or store. Reading does not
advance the recorder or any durable counter. Other host links decline diagnostic
writes unless they implement a bounded transport.

T114 bounds each observation reply to five milliseconds. A timeout or partial
write retires that DTR session; radio collection continues while the host is
absent, and an observed DTR fall permits a fresh session. This deadline bounds
observation replies only: existing ordinary USB writes, including banners and
RX forwarding, still need a separate stalled-reader audit. Therefore this slice
does not close the broader slow-host acceptance condition. Physical USB timing,
IRQ/FIFO interference, CPU cost and ring lifetime remain unmeasured.

`signalman-observe PORT [MAX_PAGES]` opens an exclusive direct-PHY serial session,
reads the profile dictionary and drains toward the initial newest sequence.
It defaults to 64 pages with a 64 KiB payload budget. A later overwrite is
retained as the full source gap. It sends only wake/observation commands,
retires a timed-out or cancelled request, and lowers DTR when collection ends.
The JSON stdout receipt preserves raw bytes, host receive time, profile
encodings, cursor/loss totals and the unauthenticated local USB association.
This explicit finite receipt is not O5's durable segment/manifest and retention
implementation. The existing serial pump is not used because its automatic
status/configuration traffic would change the experiment. Native-node and RNode
collection remain unsupported.

Physical O3 acceptance must record exclusive bench ownership, USB hardware
identity, boot-selected direct-PHY personality, previous image/recovery route,
source revision, firmware image SHA-256 and flash receipt before the run. Compare
identical RF traffic with collection absent, normal, slow, and stopped; record
captures/misses, listening recovery, USB occupancy, ring overwrite, CPU cost,
and zero observation-induced journal writes/quiet-window entries. Include a
reboot, cursor loss, DTR retirement/reconnect and ordinary RX/TX coexistence.
No board was flashed or serial port opened for this software slice.

Validation for this slice:

- `cargo test -p radio-hand -p selvage -p tulle -p signalman --features
  radio-hand/control-retinue,tulle/serial-async --locked --offline -j2
  --target-dir C:\t\retinue-20260907-signalman-collect --quiet` passed **357 tests**.
  This includes the real owner-to-carrier-to-Signalman lifecycle fixture,
  overwrite/cursor/profile checks and explicit cancellation retirement. An
  earlier millisecond-deadline test race was replaced with poll-and-drop
  cancellation before the final passing run.
- `cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf
  --locked --offline -j2 --target-dir C:\t\retinue-20260907-observation --quiet`
  passed. ELF SHA-256:
  `5c7e004d1a136dc2ee21d72b6524eff14d0a9959d66efff292560c2be3f1031d`.
  This is a linked software image, not a flash or on-air receipt.
- After `C:\Users\mark_\export-esp.ps1`, `cargo +esp check -p
  tulle-heltec-v4-phy --release --target xtensa-esp32s3-none-elf
  -Zbuild-std=core --locked --offline -j2 --target-dir
  C:\t\retinue-20260907-observation --quiet` passed. The unchanged
  `last_sleep_us` and `radio_wake_registrations` dead-code warnings remain.
  This verifies shared consumer compilation, not a linked V4 image.
- `cargo clippy -p signalman -p tulle -p selvage --all-targets --features
  tulle/serial-async --no-deps --locked --offline -j2 --target-dir
  C:\t\retinue-20260907-observation --quiet -- -D warnings` passed.
  Modified Rust files pass formatting checks and `git diff --check` passes.
  The capture executable's `--help` path passes without opening a port.

### 2026-09-08 stalled USB reader continuation

All T114 USB writes now share cancellation-safe session retirement. The entire
ordinary write, including any terminating zero-length packet, has a 25 ms
scheduling deadline; observation replies retain their 5 ms deadline. Failure or
cancellation leaves retirement latched before any further bytes can be sent.
The normal radio loop continues while waiting for a real DTR fall. Failed
session startup also retires, and the radio-failed status-only loop waits for
DTR to fall rather than immediately retrying an unread endpoint.

The earlier ordinary-write stall limitation is addressed in software. These
deadlines are implementation bounds, not measurements of radio cost or proof
that a desktop reader stopping actually backpressures the USB endpoint.
Two focused tests exercise partial-error/cancellation retirement, refusal to
poll a continuation, successful reuse and explicit reconnect. All 177 default
radio-hand library tests and the locked/offline T114 release build pass using
`C:\t\retinue-20260907-observation`. Physical acceptance remains open.

The first physical flash of `8931e81` returned with identity slot A sequence 84,
US915 and modem mode preserved. Its first observation capture exposed an
attachment-induced re-arm: modem startup unconditionally requested RX even
though the long-lived owner was already listening. Startup now resets only the
command parser; boot, transmit, profile changes and faults retain their existing
RX-preparation responsibility. Both board initializers start with preparation
owed. CDC reads also check DTR every 50 ms while idle, since application closure
does not disable USB. Reconnect tests hold DTR low for at least 150 ms; shorter
pulses are not guaranteed to be observed by this polling design. These corrections
are part of the physical probe, not a completed O3 cost receipt.

### 2026-09-08 physical result

[Raw receipt](2026-09-08_t114_observation_receipt.json) contains source and image
hashes, the flash transcript, timestamped serial bytes, parsed records, three
identical attachment captures and the final Signalman capture. The reproducible
runner is `python -B testing/o3_usb_bench.py --t114 COM10 --v4 COM6 --output
<receipt.json>`. It sends synthetic packets and explicitly applies the T114's
exact profile to the V4's runtime settings within the same serial session.
It does not change persisted settings or the region selection.

The user confirmed all boards available. T114 COM10 (`1915:521f`) was flashed
through Linkboy's expert serial-DFU route, rediscovering COM4 after transition.
Both writes reported activation and programming complete. Final firmware is
`5ccf59852d7ba25b0bdf5ff0f2710dcd9bf441eb`, application binary SHA-256
`d223b6d4c9681765049a1cd5e8739a656953c5a0344b8a275e17f8d743009ba4`,
covering `0x26000..0x70e62`, below the journal at `0xe6000`. The recovery v51
UF2 digest was verified before mutation; it is a retained recovery image, not a
backup of the prior running image. The T114 retained identity slot A sequence
84, US915 and modem mode, with crash count zero. V4 COM6 was the RF peer; its
firmware was not flashed. COM7 was unused.

Measured outcomes on the final aligned run:

- Three separate collection sessions returned identical raw records and boot,
  with newest sequence still one: attachment stopped manufacturing radio gaps.
- Six acknowledged V4 transmissions produced six T114 captures with its host
  closed. Six more produced six captures with read-only drains interleaved
  between packets. This small stationary sample does not establish saturated
  simultaneous RF/USB performance or packet-body identity.
- Forty undrained transmissions produced an explicit gap for sequences 1–27
  followed by the 32 retained events. The subsequent T114 transmit produced
  paired listening stop, TX start, TX finish and return-to-listen at sequences
  60–63. Final Signalman replay retained the enlarged 1–31 gap and two
  incomplete intervals, rather than inventing complete coverage.
- Bounded unread-request pressure accepted 173 host writes and yielded 12,312
  bytes when read back. In the separate liveness check, a further write in that
  same session timed out; DTR-low reconnect recovered discovery on the same
  boot. This demonstrates session retirement/recovery, not the actual timing of
  the device's 5 ms deadline, or RF capture continuity during pressure.
- Final radio counters were 54 good captures, zero RX errors/damaged frames,
  two successful T114 transmissions and zero TX errors. The totals include two
  alignment packets and an earlier T114 TX probe, as the receipt records.

Earlier attempts received zero RF packets despite acknowledged V4 transmissions.
The status banner was insufficient evidence of a matching runtime PHY. Applying
the exact dictionary entry within the V4 test session fixed reception; the
runner now rejects missing captures instead of treating command success as an
RF pass. The final profile was 906.875 MHz, SF11/BW250 kHz, coding denominator
five, preamble 16 and sync `0x2b`; its complete encoded definition is retained.

O3 remains partial: a planted physical refusal, power-cycle/loss leg, independent
journal-write/quiet-window measurements, CPU and IRQ/FIFO cost, and concurrent
RF under actual endpoint pressure are outstanding. The latest V4 compile
recheck was blocked by the shared Cargo package-cache lock and stopped without
a compilation result; it is not a new passing receipt. The T114 release build
and 177 default radio-hand library tests passed. The physical runner, its strict
capture/lifecycle conditions, formatting and diff checks passed.

### 2026-09-08 concurrent RF and reader pressure

The [concurrent receipt](2026-09-08_t114_observation_pressure_receipt.json)
uses unchanged firmware `5ccf598` and a separate bounded runner:
`python -B testing/o3_usb_pressure_rf.py --t114 COM10 --v4 COM6 --output
<receipt.json>`. It applies the exact T114 PHY to the V4 runtime, starts a
12-packet RF worker, and sends observation requests while keeping T114 replies
unread until that worker finishes. Both serial handles close on completion or
failure; no persisted configuration is changed.

All twelve acknowledged transmissions corresponded to twelve newly recorded
captures, sequences 64–75, without a new observation gap. The boot token stayed
`3072686413380256999`. The host unread workload lasted about 8.14 seconds;
173 observation writes were accepted, and 12,312 bytes were subsequently read.
A further write in that session timed out. DTR reconnect restored discovery,
and draining after the pre-run cursor recovered all twelve captures.

This adds a small concurrent host RF/USB workload receipt to the earlier
interleaved test. It does not resolve the exact device stall duration, individual
packet-body identity, CPU/IRQ/FIFO cost or independent journal-write count. The
runner records host monotonic timestamps rather than substituting them for
firmware execution timestamps. O3 remains partial for those measurements, the
planted refusal and the true power-cut leg. The pre-power-cut capture has been
saved, and physical removal of USB plus any battery has been requested.

The optional V4 compile retry again reached only `Blocking waiting for file
lock on package cache`; it was stopped without a compile result. Python syntax,
runner/receipt digest consistency, physical acceptance assertions and diff
whitespace checks pass for this follow-up. Firmware was not changed or reflashed.


### 2026-09-09 power-cut verification

Following the owner-reported full power removal and reconnection, read-only
Signalman collection succeeded. Boot ID changed from `3072686413380256999`
to `10685751629607856747`; the prior newest sequence 75 was replaced by a
fresh sequence-1 listening-start record. The old RAM history was absent and
the finite drain reached its target. The [existing receipt](2026-09-08_t114_observation_pressure_receipt.json)
now includes the post-power-cut capture. This closes the user-assisted
power-cut/volatile-history leg; the power removal itself was user-attested,
not independently instrumented. Refusal and cost/journal measurements remain
open. Do not request another manual power cycle for this receipt.

### 2026-09-09 refusal and flash-cost verification

The [physical receipt](2026-09-09_t114_observation_refusal_receipt.json) records
firmware `196dfd1`, its image hashes and serial-DFU transcript, the bounded
`testing/o3_usb_refusal.py` runner, raw USB traffic and final Signalman capture.
The T114 update and all checks used USB. V4 COM6 supplied the RF peer without a
firmware update. US915, the original PHY and crash count zero were verified.
The image ends at `0x71122`, below the reserved journal at `0xe6000`.

The executive now records rejected retunes as typed refusals, including missing
region, invalid profile and radio failure. The planted case requests a validly
encoded 869.525 MHz profile from the US915 owner: it returns
`CONFIG_OUT_OF_REGION` before touching the radio, and records
`Retune / InvalidProfile` with a fresh work id. This requests no transmission
and changes neither the current profile nor persistent settings.

Both physical runs passed. The extended run retained exactly one refusal,
returned the same raw 40-byte event on 64 rereads, and kept the radio counters
unchanged with RX arming still one. Flash erase and write attempt counters
remained zero before collection, after collection, and after RF recovery.
These counters are independent of the observation ring and increment immediately
before every settings, reservation and control-journal NVMC mutation, including
failed attempts. Saturation causes the runner to reject the measurement.

All three subsequent V4 packets produced captures without a listening break.
A T114 transmission then produced listening stop, TX start, matching TX finish
and return to listen at sequences 10–13. Its work id differed from the refusal.
Final Signalman collection reached cursor 13 on the same boot with zero missing
records. The earlier successful run and its raw traffic are also retained.

The current T114 modem path has no live `QuietWindow` implementation or call.
That source boundary, unchanged RX-arm counters and absence of a new continuity
break support zero collection-induced quiet entries in this path. They do not
measure electrical availability or IRQ/FIFO latency. The 64-read host elapsed
time includes the runner's serial timeout and must not be presented as device
CPU or USB occupancy. Detailed timing costs remain open; this receipt closes
the planted refusal and independent flash-mutation checks.

Validation: 177 radio-hand library tests, linked T114 release build, V4 release
check, Rust formatting, runner syntax and diff checks passed. The V4 check emits
only the existing `last_sleep_us` and `radio_wake_registrations` dead-code warnings.

Bench constraint: any future test requiring case opening or physical power-path
access uses an accessible alternative board. The sole assembled T114 is not
the disassembly fixture. A T114-specific electrical test that cannot use USB
waits for a suitable fixture; it does not create another manual task for the owner.
