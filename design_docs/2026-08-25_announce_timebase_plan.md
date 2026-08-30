# Announce timebase plan

**Date:** 2026-08-25
**Status (2026-08-29):** in progress. Phases A, B, and C are implemented. Phase A is
re-receipted against RNS 1.5.2 after replacing P8's disconnected settle delay with a
connected passive drain, adding cross-arm contamination checks, and waiting on live receiver
acceptance before stage-one shutdown. The corrected 72-cell matrix and six-cell packet-loop
diagnostic are green. Phase D's software slice has separate A/B reservation stores on both
boards, reservation-backed native-node emission on T114, and a guarded downgrade policy.
Physical power-cut/on-air receipts, explicit rekey recovery, a rebuilt guard-aware firmware
package, and the V4's separate native-node successor remain open.
**Owns:** the announce `rand_hash` field, receive-side announce freshness, and the
firmware tier's durable monotonic timebase. The Peer lane owns black-box evidence, the Air
lane owns protocol and firmware code, and Assurance owns any central validation or
provenance registry change. Evidence supplied by one lane does not close another lane's
gate.

**Related authority:** [wire format reference](2026-07-13_rns_wire_format_reference.md)
(needs the corrections in §6 below), [current RNS 1.5.2 re-pin
receipt](2026-08-29_rns_152_repin_receipt.md), [historical RNS 1.5.0 re-pin
receipt](2026-08-23_rns_150_lxmf_111_repin_receipt.md), [live-gate flake
lane](2026-08-23_live_gate_flake_lane.md), [Prns harvest
brief](2026-08-09_prns_harvest_brief.md), [work lanes](2026-08-09_retinue_work_lanes.md),
and [permissive-source classification](2026-08-25_permissive_radio_protocol_compatibility_survey.md).

---

## 0. Decisions and execution order

The source survey is prior art and a hypothesis generator. It is not one undifferentiated
implementation authority. Per the compatibility survey, only `observed-wire`,
`official-doc`, and elected `clean-donor` material may shape Retinue code. Findings from a
`source-derived-peer` may motivate a probe, but the probe or a clean donor must carry the
decision. This corrects the broader authority implied by earlier wording in this plan.

### Decisions taken in review

1. **M1, structured caller input:** `announce::build` will take a typed announce blob whose
   constructors distinguish exact wire replay from minting `nonce(5) || timebase(5)`. The
   protocol core remains sans-I/O; entropy and time still come from its caller. A raw
   `[u8; 10]` remains available only as decoded or fixture wire material, not as the normal
   emission contract.
2. **Receive freshness is not packet-loop dedup.** The packet-hash window remains a bounded
   relay-loop mechanism. Announce admission gets separate per-destination state and gates
   address-book mutation, route mutation, local publication, and relay together.
3. **Firmware durability uses an ahead-of-use reservation.** The board persists a
   `reserved_through` ceiling before it emits any value in that range. After restart it
   starts above the durable ceiling. A periodic checkpoint of the last value used is
   insufficient and is rejected below.
4. **Every mint advances.** The five-byte field is encoded in whole-second units, but the
   ordinal must increase for every emission from a destination, including two calls in the
   same second, retries, and freshly minted owned-destination path responses.

### Phases

| phase | owner and write surface | target | done-conditions |
| --- | --- | --- | --- |
| **A. Black-box decisions** | Peer: `crates/retinue/oracle/` and captured local evidence | Run P1, P2, P3, and the receive matrix P8 against pinned stock RNS with persistent, per-destination state. | Exact RNS version, inputs, order, config lifetime, shared-global-state scope, and destination-table state are recorded. Poison cases use their own destination/config. Before a path-response measurement, seeded traffic is drained through a connected passive client until an observed quiet window, and every pre-request capture is checked against every seeded candidate across all arms. No implementation source is read. |
| **B. Typed emission** | Air: `crates/retinue/src/announce.rs`, host emission, `Node` caller seam, examples and owner tests | Introduce the structured wire type; make host and board callers supply five nonce bytes plus a monotonic ordinal. | Byte-order KATs pass; deterministic injected-clock tests cover same-second emission, backward host-clock movement, and 40-bit exhaustion; no emission site fills bytes 5..10 with entropy. The already-dirty `reqresp_interop.rs` is coordinated rather than overwritten. |
| **C. Receive freshness** | Air: shared bounded freshness model plus `Endpoint` and `Node` consumers | Apply the P8 acceptance matrix before every observable announce effect. | Host and `no_std` tests cover stale time, duplicate blob, changed context, better-hop copies, ratchet/app-data rollback, expiry, and bounded eviction. Retention scope is explicit; packet-loop dedup stays separate. |
| **D. Durable firmware reservation** | Air, with board-specific storage adapters and a declared quiet-write seam | Persist a reservation before radio use, fail closed on storage fault/exhaustion, and preserve identity across upgrade. | Torn writes, corrupt timebase with valid identity, first upgrade, explicit rekey, and downgrade posture are tested. Both boards prove reservation recovery across a power cut. T114 then emits above every possibly transmitted pre-cut value, and stock RNS accepts it and completes a link. V4 on-air proof belongs to its native-node successor because V4 currently ships modem and RNode only. Flash cadence and receive blanking meet a stated bound. |
| **E. Reconciliation and receipt** | Owning docs plus Assurance-owned registry work | Correct the wire reference, source boundary, index, and final status from measured behavior. | O-20 is closed by observed bytes; open equality or migration points move to a named plan; this plan has dated Findings and Progress; hardware and software claims remain separate. |

Phases B and C may share one protocol-core commit after Phase A answers the matrix. Phase D
does not ride along merely because the A/B settings record already exists; its write
authority, endurance, migration, and failure behavior are separate proof obligations.

---

## 1. What is settled, and how

Two surveys on 2026-08-24 and 2026-08-25 read fourteen independent Reticulum
implementations at source, licence verified by opening the file in every case. The
survey findings below are useful corroboration, but source-derived peers do not authorize
Retinue code. The emitted-byte observations and persistent stock-RNS probes carry the wire
claims; elected clean-donor evidence may carry an implementation technique with provenance.

**The 10-byte announce field at payload offset 74..84 is not opaque.** It splits: bytes
0..5 a per-emission random nonce, bytes 5..10 a 40-bit **big-endian count of whole
seconds**.

Confirmed three independent ways, none of which required reading the Python reference:

1. **Stock RNS's own emitted bytes.** Committed captures decode to their own capture
   wall clock: `1786506815` is 2026-08-12 03:53:35 against directory
   `peer-20260812T035333Z`, and `1787537807` is 2026-08-24 02:16:47 against
   `peer-20260824T021646Z`. The field delta of 1,030,992 s is 11.93 days, matching the
   directory gap exactly.
2. **Fixtures already in this tree.** `announce_plain.bin` at offset 93 gives
   `f2d091f887 006a55a78a`, and `0x006A55A78A` is 1,783,998,346, which is
   2026-07-14T03:05Z, its capture date. `announce_ratchet_appdata.bin` is `006a55a78b`,
   exactly one second later.
3. **RNS's own persisted routing state.** The msgpack `destination_table` RNS writes in
   every gate run is
   `[dest(16), timestamp f64, received_from(16), hops u8, expires f64, random_blobs
   [bin10 ...], iface_hash(32), packet_hash(32)]`. `random_blobs` is a **list**, which is
   RNS's own output confirming it retains this field per destination for comparison.

**The survey says the field is a monotonic counter, not a clock.** Eight implementations that implement
announce acceptance were read. **Not one performs a calendar check, a skew check, a
plausibility window, or any comparison against the local clock.** Every one compares
ordinally, per destination, against blobs stored for that destination. Three of them have
a real wall clock available and still decline to use the field as one. Prns names the
type `MonotonicTimebase` rather than `UnixTimestamp` for this reason.

**Working hypothesis: a monotonically increasing 40-bit counter interoperates.** A real
epoch second is not required by any receiver surveyed. P3 must confirm that stock RNS does
not range-check it before Phase B or D relies on that result.

**The asymmetry that makes the fix direction matter.** Monotonicity is enforced per
destination against that destination's own prior emissions. There is no cross-node
timestamp comparison anywhere in the protocol.

- **Starting low is harmless.** microReticulum emits values in the thousands and it never
  bites.
- **Starting high is poison.** Once a value above what the node can subsequently count up
  to is latched, the node can never beat its own high-water mark again.

**The source tally predicts a strictly-greater acceptance boundary, seven to one.** P1 and
P8 decide Retinue's rule; the tally does not.

---

## 2. Defects, ranked

### D1. All ten bytes are CSPRNG. Deployment-affecting.

Three emission sites:

| site | what it emits |
| --- | --- |
| `crates/retinue/src/endpoint.rs:4568` `rand_hash()` | 10 bytes `getrandom`, via `Endpoint::announce` (2973) and `path_response` (2144) |
| `crates/radio-hand/src/channel/node.rs:501` | 10 bytes `exec.random`, fresh every Beat |
| `crates/retinue/examples/*` (about 10 files) | low-order LE nanoseconds, **worse than random** |

`crates/retinue/src/announce.rs:189` documents the field as "a fresh random one per
announce", which is the wrong contract and is where the defect is written down.

**Observed in our own artefacts.** Announces Python RNS cached during retinue's own
peering runs decode as:

```
peer-20260812T035508Z:  0c713c0192 f2ca180000  ->  1,042,772,656,128
peer-20260824T021646Z:  dcaecd302a 9cce180000  ->    673,450,065,920
```

Retinue's advertised emission time **regressed by 369 billion over twelve days**.

**Severity, stated accurately.** An earlier framing of this as "poisons a peer for seven
days" overstated it. Because retinue draws fresh randomness per announce, the Nth is
accepted with probability about 1/N, a running maximum of uniform draws, so roughly ln(N)
of N announces land. First contact always succeeds and route expiry heals. It **degrades
rather than breaks**. The two real harms are sharper than the framing suggested:

- A genuinely better, lower-hop path is discarded most of the time by the `hops <=
  existing` gate, so route optimisation toward a retinue node is broken.
- Where a rejected announce also suppresses onward retransmission, announces stop
  propagating past the first hop.

The nanosecond variant in the examples is worse than random rather than better: taking
the little-endian low ten bytes and having them read back big-endian over bytes 5..10 puts
the fastest-moving byte in the most significant decoded position. Measured on committed
captures, it plateaus for 18.3 minutes and regresses roughly every 78 hours.

### D2. No monotonic timebase exists on the firmware tier at all. Deployment-affecting.

`SystemTime` and `UNIX_EPOCH` appear nowhere in `crates/retinue/src`,
`crates/radio-hand/src` or `crates/outrider/src`.
`crates/retinue/src/announce_admission.rs:118` states the discipline: "Times are monotonic
milliseconds relative to the endpoint's creation." That resets to zero on every restart.

**Even with D1 fixed, a firmware node would regress on every reboot** without a persisted
high-water mark. This is not a consequence of D1; it is a separate missing capability.

### D3. Path table has no emission-time gate and no replay set. Deployment-affecting.

`endpoint.rs:2164`: `keep_existing = e.hops <= hops && now.duration_since(e.learned) <
PATH_TTL`. `node.rs:657`: `if route.hops <= hops { return }`. Hop count plus retinue's own
local clock. Nothing in `crates/` parses, compares or remembers `rand_hash`; its only
consumer workspace-wide is a test.

The source survey says a transport node answering a path request for a cached foreign
destination replays the original announce payload, changing routing header material rather
than the signed blob. P8 confirmed that this creates an ordinary same-blob arrival over a
different path, but it also showed that stock RNS does **not** admit that blob again, even
when the candidate path is better and the packet-loop hash has been removed.

Retinue does **not** currently implement that cache. `Shared::path_response` answers only
for a locally registered destination and mints a new announce; the executor-neutral `Node`
does not answer cached foreign path requests either. Incoming stale path responses from an
external transport can still expose the missing receive gate, but the plan must not
describe byte-preserving foreign responses as current Retinue behavior.

**The existing packet-hash window is related but not the answer.** Retinue's packet hash
masks hops, header type, and transport ID (`packet.rs:224`), while context remains part of
the hash. Moving `announce_is_new` before `learn_path` would still be wrong: it would make
context affect admission even though P8 found identical decisions for ordinary announces
and real path responses. Keep that window for relay-loop suppression. Freshness needs
parsed, per-destination blob/timebase state applied before the address book, route, local
announce publication, or relay changes.

### D4. Accepted equal- or worse-hop announces do not replace the route. Compatibility.

`endpoint.rs:2192` and `node.rs:657` both keep the incumbent on `hops <= hops`. P8 shows
that stock RNS replaces the route for every accepted newer blob at better, equal, and worse
hops. It also replaces an expired incumbent for the measured stale/worse branch. Phase C
therefore cannot put the freshness comparator in front of the existing shortest-live
filter: the accepted candidate becomes the route incumbent. A peer moving to a different
equal-length path, or deliberately advertising a newer longer path after topology changes,
must not remain pinned to the old interface.

---

## 3. The emit fix

The wire shape is settled. M1 is taken in §0; the remaining runtime and migration choices
are gated rather than left implicit.

- Bytes 0..5 from the CSPRNG, unchanged.
- Bytes 5..10 a big-endian whole-second count from a monotonic source.
- **Quantise to seconds before truncating.** 40 bits of seconds is about 34,865 years; 40
  bits of microseconds is 12.7 days. reticulum-zig truncates a microsecond counter and
  wraps in under a fortnight. One line, fatal if wrong.
- **Stay under 2^32 while in uptime mode.** Naturally true for a boot-relative counter. It
  matters because at least one Go port reads the 40-bit field into a `uint32` and silently
  discards the top byte, so a value with bits 32..39 set compares differently on that peer
  than on a Python one. Avoid triggering it; do not rely on it.

**Decision M1: `announce::build` takes a structured value.**
The sans-io contract is correct and not in question: retinue has a `no_std` firmware tier
and a tokio host tier, one implementation serves both, and a core that reached for a clock
or an RNG could not compile for the boards. It also buys byte-exact fixture pinning.
Sans-io says the caller supplies entropy and time; it says nothing about the parameter
being structureless. The type carries the split so callers cannot get it wrong. Exact
decoded bytes remain constructible for fixture replay, while a mint constructor requires
five nonce bytes and a checked 40-bit ordinal.

**Decision M2: what the `no_std` tier puts there.** A value from a durable ahead-of-use
reservation, described in §4. Boot-relative uptime without that reservation is not an
implementation option.

**Decision M3: remediation and downgrade.** No peer-state cleanup appears to be owed. Every peer that has heard
retinue is a throwaway RNS instance in a temp directory: 45 of 54 oracle scripts use
`tempfile.mkdtemp` and the rest delegate to those that do, no `destination_table` exists
outside per-run capture directories, and retinue has never been pointed at a public
Reticulum node or a testnet. The poisoning is real and demonstrated but did not survive
its test run. **The bill comes due the first time retinue announces to something
persistent.** P2 verifies the claim against an isolated persistent destination.

Firmware rollback is still owed. Once a board has emitted from a durable reservation, an
older image that ignores the appended state and resumes ten random bytes can regress or
jump beyond the reserved range. Phase D must state how Linkboy/catalog rollback policy and
the raw-owner recovery route expose or refuse that downgrade before persistent deployment.

---

## 4. The firmware timebase

### What anyone actually implements

| strategy | implemented by | status |
| --- | --- | --- |
| persisted high-water mark in flash | Prns | shipped, dual-region, tested against interrupted writes |
| persisted cumulative-uptime counter | microReticulum | shipped on ESP32, three defects below |
| raw monotonic uptime, no persistence | reticulum-zig | shipped, wraps in 12.7 days, regresses every boot |
| refuse to announce until firmware supplies time | LXMF-rs | contract shipped, **seam never wired** |
| acquire from a heard announce | nobody | zero of eleven |
| NTP, GPS, or an RTC driver | nobody | zero of eleven, grep-confirmed |
| time from a host or peer handshake | nobody | zero of eleven |

The design instinct that the announce timestamp should ride whatever time-acquisition
story the node already has, rather than being special-cased, is **unprecedented in the
surveyed corpus**. LXMF-rs is the only implementation that even provides the seam
(`RnsError::TimeSourceUnavailable`, refusing to announce until `set_time_override` is
called), and nothing in its embedded crates, runtime or C ABI ever calls it.

### What retinue has to work with

Both boards have RTC peripherals. `firmware/t114-phy` runs `embassy-nrf` with
`time-driver-rtc1`; `firmware/heltec-v4-phy` has `esp_hal` TimerGroup and hands the RTC to
`power::arm`. What they lack is not a clock but **epoch knowledge at boot**, which per §1
the wire does not require.

### Recommendation

The earlier checkpoint recommendation was wrong. If flash holds `100` and the board emits
through `700` before power loss, restoring at `101` still regresses by 599 values. Rounding
and adding one changes the unit; it does not make an old checkpoint cover values emitted
after it. Periodically rewriting `SettingsStore` also violates the current rule that flash
writes happen before radio startup or immediately before reset, and a ten-minute erase
cadence would exhaust ordinary sector endurance.

The corrected shape is:

1. **Whole-second wire units, strict ordinal minting.** Quantise a real clock before
   encoding it, but mint `max(source_seconds, last_emitted + 1)`. A board without epoch
   knowledge advances the reserved ordinal once per emission. Same-second retries and path
   responses must not reuse the prior ordinal.
2. **Persist a reservation before use.** Durable state records `reserved_through`. Boot or
   a declared quiet-write operation atomically advances that ceiling, verifies it, and only
   then allows emission from the newly reserved range. A reboot starts above the durable
   ceiling, even if every value in the old range was heard before the crash.
3. **Fail closed.** If reservation write/readback fails, its state is corrupt while the
   identity survives, the 40-bit range is exhausted, or a live node consumes its range
   without an authorized quiet window, it emits no announce and exposes a concrete fault.
   It does not fall back to uptime, zero, randomness, or a guessed epoch. Explicit rekey is
   a recovery action, not an automatic response to timebase damage.
4. **Storage is a separate design target.** The existing A/B settings record supplies a
   torn-write pattern, not automatic authority for runtime erases. Phase D chooses and
   proves a dedicated reservation journal or a bounded boot-time reservation scheme,
   including capacity, configurable lease size, flash wear, receive blanking, and renewal.
5. **Migration is part of correctness.** First upgrade from a legacy identity with no
   reservation, corrupt reservation with a valid identity, factory reset/rekey, and
   downgrade to an image that ignores the field each get an explicit outcome and test.
6. **A real epoch remains separate.** If later adopted, its representation is tagged so a
   board can move from ordinal to epoch-derived values but cannot silently move backward.
   Ratchet expiry, link-request freshness, and cross-node log correlation need their own
   wall-clock decision; this field does not supply one.

### Phase D software shape implemented 2026-08-26

The reservation is not appended to `Settings`. It has its own `RHR0` v1 body inside the
existing generic A/B record framing: versioned body magic, one reserved zero byte, and an
inclusive 40-bit big-endian `reserved_through`. An adapter may treat the pair as
uncommissioned only when both outer slots are erased. A valid older slot survives a torn
newer write. No valid slot plus any nonblank/corrupt data is a fault, not permission to
recommission.

Boot plans `new_ceiling = prior_ceiling + lease_size`, writes the inactive slot, re-reads the
authoritative pair, and constructs `TimebaseGenerator::firmware_lease` only after exact body
verification. The default is 65,536 logical ordinals and remains caller-configurable. T114
advances by exactly one for each attempted announce; heartbeat uptime is scheduling input,
not ordinal input. At the current ten-minute cadence, an uninterrupted default lease covers
about 455 days. Reboot abandons the unused part and reserves above the old durable ceiling,
so ordinary runtime performs no flash writes and has zero reservation-induced receive
blanking. Lease exhaustion refuses announces until a reset/authorized reservation.

Storage is board-owned and identity-independent:

| board | reservation pair | settings pair | current emission use |
| --- | --- | --- | --- |
| T114 | `0xE8000..0xEA000` | `0xEA000..0xEC000` | Reserved and verified before radio initialization when native node is requested. Fault selects modem recovery and reports `timebase=fault`. |
| Heltec V4 | `0x3F2000..0x3F4000` | `0x3F0000..0x3F2000` | Storage-only `timebase` / `timebase reserve [lease]` probe, committed immediately before reset. V4 has no native-node personality and makes no emission claim. |

Downgrade has two barriers. Native-node settings now persist guarded channel byte `3`; old
firmware knows only legacy byte `1`, treats `3` as unknown, and selects modem. New firmware
reads byte `1` only as a migration state and verifies a settings rewrite to byte `3` before
reserving or using the radio. An active native channel reports
`state=node-timebase-v1`. Linkboy carries that observed state into its immutable plan and
refuses packages without a matching persistent-state declaration and a real preserved range.
The retained v47/v51 artifacts correctly remain undeclared because their immutable binaries
predate Phase D, even though their manifests now preserve all four pages.

This closes the software storage, torn-write, first-upgrade, exhaustion, and ordinary
downgrade paths. Still open: explicit rekey recovery tests; a rebuilt, immutable
guard-declaring package; physical cuts before/during/after reservation; T114 stock-RNS
announce/link after the cut; and a future V4 native-node implementation before any V4
on-air claim.

---

## 5. The receive fix

**Source-survey tally at equal-or-better hop count:**

| strictly greater | accepts equal | no comparison |
| --- | --- | --- |
| microReticulum, reticulum-kt, reticulum-swift, Quad4 Go, go-reticulum, LXMF-rs, Prns | reticulum-zig | ReticulumKit, rns.js, one Go fork |

Seven to one, and the one appears broken in the same function: its stricter term is a
subset of its looser term, so the hop comparison is dead code and hop count plays no role
in acceptance. This is corroboration, not implementation authority; P1 and P8 settle the
stock behavior Retinue follows.

**Two field post-mortems corroborate independently of any source reading.** reticulum-kt
commit `3e22e7e` *added* the emission-time gate after a deployed network showed phones
holding stale path-response entries at four hops that fresh one-hop announces could not
overwrite. reticulum-swift carries a comment from someone who hit the mirror failure:
without a proper timestamp "the relay's deduplication logic will reject our announces".

P8 established these branches against stock RNS 1.5.0 and revalidated them against RNS
1.5.2. “Worse” below means a larger calibrated hop count; the expired/worse result is not
a typo.

| loaded route | candidate blob | timebase | hops | stock outcome |
| --- | --- | --- | --- | --- |
| live | new | newer | any | admit |
| live | new | equal or older | any | no observable admission |
| live | exact same | equal | any | no observable admission |
| expired | new | newer | any | admit |
| expired | new | equal or older | worse | admit |
| expired | new | equal or older | better or equal | no observable admission |
| expired | exact same | equal | any | no observable admission |

Ordinary and real path-response contexts produced the same result in every row. A separate
six-cell run moved the observed packet-hash list aside while preserving the destination
table; exact-same blobs still never changed the route, including the live/better and
expired/better rows. Packet-loop dedup therefore cannot stand in for announce freshness,
and a same-blob better-path carve-out is not stock behavior.

**Recommendation:** implement this rule once in a bounded per-destination freshness model
shared in semantics by `Endpoint` and `Node`. Within the one-incumbent P8 shape, the compact
form is `new_blob && (timebase > incumbent_timebase || (expired && candidate_hops >
incumbent_hops))`. Admission occurs before `AddressBook::ingest`,
`learn_path`/`learn_route`, `PeerAnnounce` publication, and relay so every effect follows the
same stock-compatible decision. Any retained-history guarantee is stated as **within the
configured bound and lifetime**; no finite board can promise to reject one blob forever.

### Phase C implementation scope

The shared always-available core owns one row per destination:

- the current accepted `AnnounceBlob`, whose last five bytes supply the incumbent timebase;
- the incumbent hop count and caller-supplied monotonic acceptance tick;
- a bounded oldest-first history of accepted full ten-byte blobs.

First sighting is admitted. A candidate whose full blob remains anywhere in history is a
replay and is refused. Every other candidate follows the compact P8 rule above. Context,
interface, transport ID, packet hash, ratchet, and app data are deliberately absent from the
decision. On acceptance the candidate becomes the incumbent, including the expired/worse
branch where its timebase may move backwards. Historical blobs remain so a later replay of
the displaced higher value does not regain authority merely because the incumbent moved.

Evaluation and recording are separate operations. `Endpoint` holds one freshness guard
across evaluation, address-book capacity outcome, recording, route replacement, local
publication, and relay scheduling; concurrent held-release tasks therefore cannot both
admit against one incumbent or publish out of decision order. `Node` performs the same
sequence in its single-threaded ingest turn. A full address book refuses the announce
without recording its blob, since nothing else learned or published it.

Route usability and freshness retention are separate clocks. The existing route lifetime
moves the row from live to expired for the P8 decision, while a longer configurable
retention lifetime keeps the freshness tombstone after the usable route is physically
removed. Destination and per-destination blob capacities are explicit. Retention-expired
rows are removed first; remaining pressure evicts the oldest accepted row, and blob pressure
evicts the oldest retained blob. Rejected traffic never refreshes eviction age. Host defaults
remain aligned with its 4,096-peer tier; the board default remains aligned with `PEERS` and
uses a smaller per-destination history. The 64-bit payload-only accounting receipt is 8,760
bytes for a 32-destination, eight-blob board profile and 1,900,600 bytes for the host's
4,096-by-16 profile. That includes table and `Vec` headers plus retained elements, but not
allocator metadata or spare capacity; Phase D's board integration still owes a target heap
receipt. Callers can choose narrower bounds.

Phase C receive history is runtime state. Reconstructing an `Endpoint` or `Node`, configured
retention expiry, and bounded eviction are the declared points at which an old blob may
become a first sighting again. Durable receive replay state would need a separate storage,
wear, corruption, and migration design; it does not ride on Phase D's outbound ordinal
reservation.

Tests have three layers: the pure core enumerates the 72 expected decision-table cells from
the measured P8 dimensions, plus multi-step historical replay and tiny-capacity eviction
cases. Context is deliberately repeated as a non-input there; signed `Endpoint` packets
carry the ordinary/path-response context proof. `Endpoint` proves zero address-book, route,
publication, sequence, or relay mutation on rejection, including a real held-release task
ordered behind newer direct ingress. `Node` proves the same through its bounded action list
and `no_std` build. Both consumers prove that an accepted newer equal/worse route replaces
the incumbent, an expired stale worse candidate is admitted, an expired stale better/equal
candidate is refused, and packet-loop dedup remains a later independent relay mechanism.

---

## 6. Wire reference corrections

Against [the wire format reference](2026-07-13_rns_wire_format_reference.md). Layout
corrections may land from committed bytes; receive-policy corrections wait for Phase A.

1. **Close O-20** and rewrite §3.3.4. The 10 bytes are 5 random plus 5 big-endian whole
   seconds. Confirmable from fixtures already in the tree, per §1.
2. **Raise O-20's impact from "Low".** The current rationale, that the field is opaque to a
   verifier and only affects dedup and freshness, is wrong: it is opaque to a *verifier* but
   decisive to a *transport node's path table*, and it gates onward retransmission. It is
   the difference between being routable and not.
3. **Split the two payload tables** (the field table and the offset table) into nonce(5) and
   timestamp(5, BE seconds) rows, sourced to the captures rather than to Beechat. Note that
   Beechat's `SHA256(...)[0..10]` line describes its **pre-`d4fc67d`** behaviour and is now
   historical: that commit, "fix: announces include timestamp in random blob", is an
   independent author discovering this defect by interop testing.
4. **Expand §3.3.5** from a note that retinue has no dedup or freshness check into the named
   gap, with the measured P8 matrix and bounded-state rule from §5 written out.
5. **"Expires routes at exactly 7.0 days" is the surveyed stock default-mode figure only.**
   The source survey reports ACCESS_POINT 24 h, ROAMING 6 h, else 7 days. Do not map that
   onto Retinue by assertion: Retinue host and board routes currently default to 30 minutes,
   and the remote stock interface mode is a separately observed/configured fact.
6. **Add `PATHFINDER_M = 128` with its asymmetry:** admit at `hops <= 128`, retransmit only
   at `hops < 128`. Invisible from the manual.
7. **Clarify path-response ownership.** A transport answering for a cached foreign
   destination must preserve the signed payload if P8 confirms that stock behavior; an
   owner answering for its own destination may mint a fresh announce. Retinue currently
   implements only the latter and has no foreign announce cache.
8. **Add `LOCAL_REBROADCASTS_MAX = 2`**, suppression keyed on `packet.hops - 1 ==
   entry.hops`.
9. **Add the RNS destination-table format** from §1, as retinue's first direct observation
   of RNS's routing state.
10. **Add an ecosystem-hazard note, not a spec fact:** one source-surveyed Go port truncates
    the 40-bit timebase to `uint32`. Stock P6 cannot establish whether it is the only port
    with that defect.

---

## 7. Probes

Black-box observation of stock RNS output is always available and is how the field layout
was confirmed. It is the default instrument.

| # | question | instrument | gates |
| --- | --- | --- | --- |
| **P1** | Does stock RNS accept an equal-timestamp announce at equal hops? | Two announces from one destination in the same wall-clock second, different nonces, against a **persistent** RNS config. Read `destination_table` between each; `random_blobs` grows only on acceptance. | §5. Seven implementations predict rejection and **nobody has measured it.** |
| **P2** | Does a poisoned high-water mark actually lock a corrected retinue out? | Announce once with an absurdly high timestamp half, then with a correct one; check whether `destination_table` grew a second blob. | the severity claim in D1 |
| **P3** | Is a received timebase range-checked anywhere? | From one destination, emit timebase `1` (1970) and `2^39`; read `destination_table`. | **§4 entirely.** If a boot-relative counter is rejected, the whole firmware recommendation collapses. |
| **P4** | How many blobs does RNS retain? | 70 announces at 1 Hz into a persistent instance, count the array. | the list-versus-scalar half of §5 |
| **P5** | Does stock RNS gate link requests on wall-clock freshness? | Capture a stock LINKREQUEST, replay the old request after a controlled delay/restart, and observe acceptance or proof. Merely finding a msgpack timestamp does not test the gate. | whether a clockless board can do anything beyond announcing |
| **P6** | What does stock RNS do with bits 32..39 set? | Same instrument as P3 with `2^33`. | stock's full-40-bit behavior; says nothing about whether the surveyed Go truncation is unique |
| **P7** | Path-request addressing: to the target hash, or to a well-known `rnstransport.path.request` destination? | Capture what `rnpath <hash>` emits. | one implementation disagrees with our reading |
| **P8** | What exact route/freshness combinations does stock accept? | In one persistent receiver, use a distinct destination per cell while varying `{older,equal,newer}` timebase, `{same,new}` nonce, `{better,equal,worse}` hops, ordinary/path-response context, and live/expired incumbent. Record the shared-runtime scope, forwarded frames, and destination table. | §5 receive algorithm, including whether a same-blob better path is useful and which state mutations must be gated |

P1 and P3 gate emission. P8 gates receive behavior. P2 measures remediation severity. P5
belongs to the later wall-clock decision, not this implementation trunk.

P8 is answered by the current RNS 1.5.2 and historical RNS 1.5.0 receipts in §11. The
expired arm is a loaded-state perturbation with a byte-identical MessagePack round trip
before changing selected expiry values; natural elapsed expiry remains a distinct lifecycle
measurement.

**Every stateful probe needs a persistent RNS config directory, and poison probes need
isolated destinations/configs.** Every current gate uses
`tempfile.mkdtemp`, which is why all nine committed runs contain exactly one entry with
exactly one blob: **the suite only ever exercises the first-sighting arm, which accepts
unconditionally.** It would pass with the field set to all zeroes. That blindness is
structural and should be recorded in the validation section of the wire reference.

---

## 8. Clean-room rule: microReticulum comment blocks

**microReticulum's Apache-2.0 sources contain large volumes of verbatim Reticulum Python
reference source in comments**: 23 `/*p ... */` blocks, 153 `//p` lines, 41 `//z` lines,
with no NOTICE file and no attribution to the Python author anywhere.

**Standing rule: microReticulum is a `source-derived-peer`, not a Retinue donor.** Its
permissively licensed source may be read for survey classification, but it may not shape
Retinue implementation. The `/*p`, `//p` and `//z` blocks are additionally forbidden even
inside survey work: they appear to reproduce restricted Python reference source, and a
repository licence cannot relicense pasted code.

One surveyor read two such blocks before recognising the pattern and disclosed it. Nothing
was carried into a recommendation and no retinue code is affected: retinue's IFAC came from
captured bytes and published documentation.

This makes microReticulum a **peer and probe lead, not implementation authority**. The
broader evidence-class rule already lives in the permissive compatibility survey. The
specific comment-block hazard belongs in `crates/retinue/oracle/README.md` beside the RNS
black-box discipline. Outrider's provenance already excludes all third-party protocol
implementation source, so duplicating a microReticulum-specific carve-out there would add
noise rather than a stronger boundary.

---

## 9. Done-conditions

- No emission site in the workspace writes randomness into bytes 5..10. Deterministic tests
  decode emissions and prove byte order, same-second strict advance, backward-clock
  handling, and exhaustion without sleeps.
- A firmware node durably reserves before use and survives reboot/power loss without
  emitting at or below any value it might already have transmitted. Verified on both
  boards against persistent stock RNS, not inferred from a host test.
- Receive admission follows the measured P8 matrix and gates address book, route, local
  publication, and relay together. Replayed or stale material is rejected within an
  explicit per-destination retention bound; packet-loop dedup remains separate.
- A gate exists that exercises the non-first-sighting arm, meaning at least one gate uses a
  persistent RNS config across two announces.
- P1, P3, and P8 are answered before their dependent code lands.
- The wire reference carries the §6 corrections and O-20 is closed.
- The clean-room rule in §8 is recorded in the Retinue oracle README and agrees with the
  compatibility survey's evidence classes.
- Legacy upgrade, corrupt reservation with a valid identity, explicit rekey, reservation
  exhaustion, and firmware downgrade each have a named, tested outcome.

## 10. Explicitly out of scope

- Adopting a real epoch anywhere. Separate decision, §4.5.
- The equality carve-outs in §5. Deferred pending measurement.
- Ratchet expiry, link-request freshness, and anything else that wants a wall clock.
- The live-gate flake, which has [its own lane](2026-08-23_live_gate_flake_lane.md).
- Any change to Prns, microReticulum, or any other surveyed implementation. We read them;
  we do not carry patches to them.

## 11. Findings

- **2026-08-25, observed bytes:** committed stock-RNS captures and fixtures establish the
  5-byte nonce plus 5-byte big-endian whole-second layout. The stock acceptance branches
  remain Phase A questions rather than source-tally conclusions.
- **2026-08-25, black-box RNS 1.5.0 receipt:** the persistent clean-room probe answered P1,
  P2, and P3 with exact packet/blob inputs. P1 equal timestamp was rejected; P2 rejected
  the corrected timestamp after a `2^39` high-water announce; P3 accepted both `1` and
  `2^39`. Every first sighting persisted at one hop with a valid packet. The ignored local
  receipt is `validation/results/announce-timebase-final2/result.json`, SHA-256
  `639dda1d1d4f8ef9128a6a4f4ceeda00444524a62c1da234c7c167d5a6ab1ac1`.
- **2026-08-25, P8 baseline failure:** a separate stock TCP client sent a public-valid
  Type-1, wire-hop-zero announce to a stock TCP server transport. RNS cached the announce
  under `storage/cache/announces` but did not create a destination-table row. This is an
  invalid topology for the receive matrix, not negative freshness evidence. P8 still needs
  a real forwarded Type-2 transport path; no receive rule is inferred from this run. The
  ignored local diagnostic is `validation/results/route-freshness-direct-baseline/`; its
  `baseline.json` SHA-256 is
  `af744756f18bfa7ef46514bf66f7b9b73240cafb85f0b53248712064190876e0`.
- **2026-08-26, black-box RNS 1.5.0 P8 receipt:** natural two/three/four-transport chains
  calibrated better/equal/worse forwarded Type-2 hops against a persistent three-transport
  incumbent. Public `request_path` calls produced real context-`0x0b` responses; no context
  byte was synthesised. All 72 rows had public signature-valid forwarded Type-2 frames,
  valid hop relations, and no matching signature-valid frame with an unexpected header
  type or context.
  The measured rule is in §5. The ignored receipt is
  `validation/results/route-freshness-full-20260826T211647Z/result.json`, SHA-256
  `bcb83e38b9d840926f2ee3a7093a37877fa6f84e2d2c4ed1290c4290c2a17a38`.
- **2026-08-26, packet-loop isolation:** moving the observed `packet_hashlist.raw` aside
  while preserving and reloading the destination table moved all six incumbent route
  hashes out of the pre-candidate loop window and did not change any exact-same-blob
  outcome. All remained no-admission across live/expired and
  better/equal/worse. The ignored receipt is
  `validation/results/route-freshness-same-blob-diagnostic-20260826T212136Z/result.json`,
  SHA-256 `7b9680456492d7577b78fdd5b0007ad17934ebb966b50eb5549c2f2b83c269fc`.
- **2026-08-29, black-box RNS 1.5.2 P1/P2/P3 revalidation:** the persistent probe
  reproduced the 1.5.0 decisions. P1 rejected equal timebase, P2 rejected `2` after a
  `2^39` high-water announce, and P3 accepted `1` followed by `2^39`. The ignored receipt
  is `validation/results/announce-timebase-20260830T024551Z/result.json`, SHA-256
  `252c29b40dc4b972f1615935e434a8a1802cccf6f2db6ec641f4bae851b5fef0`.
- **2026-08-29, RNS 1.5.2 P8 harness correction and receipt:** the first re-pin run found
  path-response candidates before any request. Inspection showed the old 15-second settle
  elapsed while terminal transports had no downstream connection, so it could not drain
  egress. That run is setup evidence only. The corrected harness connects a passive drain
  to every terminal until an observed quiet window and scans every arm for every seeded
  candidate. The final full receipt has 72/72 valid rows, 72 signature-valid forwarded
  frames, and zero conflicting-frame cells at
  `validation/results/route-freshness-full-20260830T030952Z/result.json`, SHA-256
  `14601b688fe72e1763e8d022915c468d1f9b164715bc47aee726050b905aaf39`.
- **2026-08-29, RNS 1.5.2 packet-loop isolation:** stage-one shutdown now waits until the
  live stock receiver has accepted every incumbent rather than sleeping after socket
  delivery. With all six incumbent packet hashes then removed from the loaded loop window,
  all six exact-same-blob rows remained valid and showed no admission or route transition.
  The ignored receipt is
  `validation/results/route-freshness-same-blob-diagnostic-20260830T030802Z/result.json`,
  SHA-256 `d660ea18f6ce38d0029672d85829a257d25f53dd30ae2df9cec63fe2f6972550`.
- **2026-08-25, live code:** host `PathEntry` and firmware `Route` retain no announce blob
  or timebase, so neither can implement freshness without a new bounded state model.
- **2026-08-25, live code:** `announce_is_new`/`seen_transit` are relay-loop windows applied
  after route learning. Moving them earlier would conflate loop suppression with route
  freshness and discard some useful multi-path evidence.
- **2026-08-25, live code:** Retinue path responses are freshly minted only for locally
  registered destinations. Retinue has no cache from which to replay a foreign announce.
- **2026-08-25, persistence review:** checkpoint-plus-one is not crash monotonic. Current
  board stores also restrict erases to pre-radio startup or immediate-reset paths, so a
  runtime high-water rewrite cannot be smuggled through `SettingsStore::save`.
- **2026-08-25, migration review:** the settings body is intentionally append-only and old
  firmware ignores later fields. That preserves identity but makes downgrade to a
  pre-timebase image a protocol-safety case that Phase D must expose or refuse.
- **2026-08-26, board audit:** T114 is the only current native-node target. Heltec V4 ships
  modem and RNode and explicitly refuses `channel node`. Phase D therefore separates the
  two-board storage/power-cut receipt from T114's on-air receipt; V4 native emission is a
  successor implementation, not a persistence side effect.
- **2026-08-26, package audit:** the retained T114 v47/v51 payloads predate durable leases.
  Their manifests may truthfully preserve the expanded state range, but cannot declare
  `node-timebase-v1` support until a new immutable artifact is built and hashed.
- **2026-08-26, CI regression on `eaff89e` (filed from the CI-repair lane):**
  `current_and_retained_ratchets_deliver_without_opening_a_link`
  (`crates/retinue/tests/endpoint_single.rs:71`) fails deterministically on GitHub CI with
  `rotated announce arrives: Elapsed(())` — the refreshed announce after `update_ratchets`
  misses its 2 s window on a loaded runner. Two independent CI executions failed
  identically (run 33031356328, first pass and rerun, check job; msrv fails the same test),
  while 10/10 local runs pass in ~1.1 s. The failure appeared with the receive-freshness
  enforcement and is the sole reason main CI is red at `8d47542` — every other job and
  every pre-existing failure is green there. Per the flake-lane rule, the 2 s constant
  should not simply be refitted under load; the hold/release mechanism on a slow runner is
  the thing to look at.

## 12. Progress

- **2026-08-25:** Reconciled the research brief with doc policy; added phases, lane
  boundaries, validation conditions, and this log. Chose a structured sans-I/O emission
  input, separated freshness from packet-loop dedup, replaced periodic high-water writes
  with ahead-of-use reservation, corrected current path-response and route-TTL claims, and
  made P8 a prerequisite for receive code.
- **2026-08-25:** Phase A is partially complete. P1/P2/P3 implementation and execution are
  complete in the pinned RNS 1.5.0 receipt above. P8's first attempted topology failed its
  baseline gate and was not retained as an implementation; the real transported matrix
  remains open.
- **2026-08-25:** The additive Phase B core primitive is implemented: a typed ten-byte
  announce blob, checked 40-bit decoding/minting, and a pure strict-advance generator with
  host and pre-reserved firmware ceilings. Caller migration remains behind the completed
  P1/P3 evidence and the separate firmware persistence design. The complete Retinue
  library gate passed 172 tests with `cargo test --locked --offline -p retinue --lib -j 1`,
  including all six new deterministic timebase tests.
- **2026-08-26:** Phase A is complete for the implementation trunk. The clean-room P8
  harness and packet-loop isolation diagnostic answered the receive matrix without reading
  RNS source. Phase C receive code remains unimplemented; natural elapsed-expiry behavior
  is explicitly outside the P8 receipt rather than being claimed from the loaded-state arm.
- **2026-08-26:** Phase C is implemented in one `no_std + alloc` freshness core and the
  `Endpoint`/`Node` consumers. The receive rule is applied before address-book, route,
  publication, and relay effects; refused address-book admission leaves no tombstone;
  accepted equal/worse routes replace the incumbent; packet-loop de-duplication remains a
  later relay concern. Runtime retention, row/blob bounds, policy reconfiguration, expiry
  and eviction counters, and process-lifetime reset are explicit. Exact-tree gates passed
  197 library tests, 167 feature-minimal library tests, `cargo check --no-default-features`,
  and checks of `radio-hand`, `postilion`, and `signalman`, all locked, offline, and `-j 1`.
- **2026-08-26:** Phase B caller migration is complete. Endpoint announces and owned path
  responses use per-destination host generators; `Node::announce` accepts only
  `AnnounceBlob`; `Node::poll(None)` runs maintenance while leaving a due announce due; and
  direct-emission examples mint the typed 5+5 form. Exact-tree gates passed 201 default
  library tests, 168 feature-minimal library tests, and both example builds.
- **2026-08-26:** Phase D's software slice is implemented as described in §4. Both board
  adapters use independent A/B reservation pairs and exact readback. T114's actual embedded
  target and both Heltec V4 ESP target configurations check successfully; the default
  `radio-hand` suite passes 63 tests, its node-feature test target checks, and Linkboy passes
  84 tests. Hardware power-cut/on-air receipts, explicit rekey, the rebuilt package, and V4
  native-node work remain open and are not inferred from these software gates.
- **2026-08-29:** Phase A was revalidated at the current RNS 1.5.2 pin. The P1/P2/P3
  decisions are unchanged. P8's corrected connected-drain harness passed all 72 route rows,
  and its receiver-event-gated packet-loop diagnostic passed all six exact-same-blob rows.
  The first contaminated re-pin attempt carries no route-semantic claim. The current-pin
  package, live-gate, Resource, Outrider, and peer evidence is collected in the
  [RNS 1.5.2 re-pin receipt](2026-08-29_rns_152_repin_receipt.md).
