# Position disclosure

**Date:** 2026-09-01
**Status (2026-09-03):** in progress. PD0 ruled and closed. PD1 and PD2
implemented in `radio-hand::control::position_disclosure` with tests; closed in
software, unreceipted on any board. PD3's V4 UART/module/cable and parser path
are physically receipted, but no satellite position fix has been measured; PD3
and PD4 through PD6 remain open.
**Owns:** PD0 through PD6. Whether a node reports its position, to whom, at what
precision, and by what authority.
**Consumes:** FS2's command envelope (closed in software), FS6's bounded-table
discipline, `gazette`'s contact trust, `PublicConfigurationV1`.
**Does not own:** the GNSS hardware choice, the disclosure-tier ruling itself
(that belongs to the field-node security posture), or any OTA bearer.

## Why this exists

A carried node that reports where it was and what it heard is the same artefact
twice. It is the instrument that answers whether a power-capped leaf node is
useful in a given place, and it is the coverage map a customer needs to decide
where to put a second node. Building it as test scaffolding and rebuilding it
later as a feature would be doing it twice.

It is also the first feature in this repository whose default is a privacy
decision rather than an engineering one, which is why it gets a plan before it
gets code.

## The shape, and the constraint that fixes it

**An announce is a broadcast. Its contents cannot vary by recipient.** Anything
in `app_data` is disclosed to everyone in earshot. That single fact splits the
feature in two, and no amount of cryptography merges them.

**Broadcast tier.** One public value, read by anyone. Off, coarse, or precise,
owner's choice. Precise-to-everyone is a legitimate setting and not a degraded
one: a beacon that wants to be found, a public asset, an emergency deployment.

**Directed tier.** Per-identity precision, answered over a link to a named
destination. Unlimited granularity, auditable because you can record who asked,
and revocable per identity without touching anyone else.

The constraint that makes a per-recipient *announce* impossible is airtime, not
cryptography. See PD-F5 below: roughly 60 to 70 bytes of headroom, which fits
one group-encrypted blob and not one blob per trusted identity.

## The node needs a projection, not a directory

`gazette` authors the policy: rich contacts with `trust` and `freshness`, shared
across Knot, Moot and Signalman. The node must never need it.

What ships to the node is a **compiled, owner-signed table of address hash to
disclosure level**. No names, no handles, no network dependency, no directory
lookup. A headless placed node with no host therefore carries its owner's
decisions and enforces them alone, which is the point of the owner-claim model.

**Corrected 2026-09-02: the entries are stored blinded, not as plaintext
hashes.** The first draft said "sixteen bytes and a level per entry", which
would put trusted identities on flash in the clear, and the seizure paragraph
in the field-node security posture forbids contacts on flash. Each entry is a
keyed hash of the address hash under a node-local secret, so a flash dump
cannot enumerate who was granted. It can still confirm a candidate the seizer
already holds, because the secret is on the same flash; PD0 states that
residual in the seizure paragraph rather than mitigating it away. Lookup cost is
one keyed hash per directed request. See PD-F10.

## Authority

The ACL is a **command to a running node, so it rides the FS2 envelope under
config authority.** It needs no offline firmware key, no removable media, and no
publisher trust root. It inherits FS4's governing rule unchanged: an authority
that can be used remotely must not also be able to replace firmware.

The FS2 per-command counter is also the replay defence. A captured old ACL
cannot reinstate a revoked grant.

## Gates

### PD0. The disclosure model is ruled

Tiers named, defaults fixed, and the promise each tier makes written down. The
ruling belongs in the field-node security posture; this gate is done when that
document carries it and this plan cites it.

Must state explicitly what **coarse does not promise**. See PD-F6: coarse
position broadcast repeatedly and heard by several receivers logging RSSI is
trilaterable back toward precise. The guarantee degrades with broadcast rate and
receiver count, and the coverage-mapping feature in PD6 is exactly that
apparatus. Coarse is obscured against a single observation, not against
sustained multi-receiver observation, and the tier list must say so rather than
implying safety.

**Done when:** tiers, defaults, and the coarse caveat are ruled and cited, and
the absent-identity default is chosen explicitly rather than by convention.

### PD1. The ACL record

Canonical, versioned, bounded. Address hash to disclosure level.

- **Bounded and refusing, not evicting.** Follow `AnnounceIngressPolicy`: explicit
  capacity, refuse at the limit, count refusals. Silently evicting a kin entry
  to make room is a privacy failure that presents as a bug. The refusal must
  also surface **at projection time**, where the owner compiles the table, not
  only as an on-node counter discovered after a grant silently failed to take.
- **No table is a defined state.** A freshly commissioned node with no ACL yet
  pushed answers every directed request at the broadcast tier. It never invents
  a default grant, and it never refuses to answer at all, because a node that
  goes silent on first boot is indistinguishable from a broken one.
- **Absent-identity default is a field, not a convention.** Whitelist and
  blacklist semantics are both defensible; the case that dominates in practice is
  the identity nobody has ever seen, and it must be answerable by reading the
  record.
- **Illegal states rejected at construction**, as `ReticulumTransportPolicy`
  already does with `max_hops`.

**Done when:** the record encodes and decodes canonically, round-trips under
test, rejects non-canonical forms, and refuses past capacity with a counter.

### PD2. Activation semantics

**Monotonic forward only. No autonomous rollback.**

`update.rs` models `ActivationMode` with an atomic trial boot and autonomous
rollback, which is correct for firmware and **wrong for an ACL**: a node that
reverts to yesterday's table re-grants whoever was just revoked. This is a new
mode, not a reuse.

**What makes no-rollback safe, stated so nobody has to rediscover it:** the
ACL governs disclosure only. It never governs who may command the node. Command
authority is the owner's FS2 key, independent of any table, so a bad ACL push
is always correctable by the next command. If a future change ever let this
table gate command authority, a bad push with no rollback would lock the owner
out permanently. That combination is forbidden, and PD2 fails if it appears.

**Done when:** an ACL write is accepted only with a higher counter, a replayed
or older table is refused and counted, no path exists by which the node reverts
to a previous ACL, and a test proves the owner can still push a corrected table
after pushing one that refuses every identity including the owner's own.

### PD3. GNSS driver

NMEA over UART on boards that have a module. `radio_hand::gnss` owns the
host-testable parser; the V4 firmware owns its RX-only UART1 wiring and module
control pins.

Fix quality must be represented, not assumed. No fix, stale fix and valid fix
are three different states and the disclosure logic must see which it has.

**Done when:** a board with a module reports position and fix quality through
the existing status path, and reports absence rather than a stale value when the
fix is lost.

### PD4. Position schema

What lands in `app_data`, in what units, at what precision per tier. Coarse must
be coarse *at the source*, quantised before it is encoded, never full precision
truncated at render time.

**Done when:** the encoding is fixed, byte-counted against PD-F5's headroom, and
a coarse value is demonstrably underivable from what is transmitted.

### PD5. The two carriers

Broadcast field in a `PublicConfigurationV2`, following the transport-policy
precedent in the same record. Directed request and response over a link,
consulting the PD1 table.

**Done when:** a node answers the same asker differently by tier, and the
broadcast value is independent of any directed grant.

### PD6. Coverage capture

The stationary node logs received frames with `rssi_dbm`, `snr_db`, reported
position and timestamp. This is the walk-test instrument and the customer-facing
coverage map, and it is one artefact.

The placed node's own position is part of the record. Without it the RSSI
column has no spatial anchor and cannot be read against distance, which is the
whole reason to log it rather than presence alone.

**Done when:** a single CSV from one placed node, carrying that node's position
in its header, reconstructs a carried node's route with signal strength, and
drop and pickup are visible as gaps with a last known position on each edge.

## Findings, verified 2026-09-01

- **PD-F1. `gazette` already carries per-contact trust.**
  `ports/gazette/src/ledger.rs:22-28` defines
  `Contact { id, name, handle, trust, freshness }`, with values `"vouched"` and
  `"known"` in its own tests. `trust` is a `String`. A disclosure decision keyed
  on a stringly-typed field will default wrong on a typo, so PD1 must map it to
  a closed set at projection time rather than compare strings on the node.
- **PD-F2. `retinue::address_book` is not a contact list and should not become
  one.** It is a bounded cache fed by `ingest(announce)` with `max_peers` and a
  `refused` counter. It answers "who have I heard on the air", which is a
  different set from "who do I trust".
- **PD-F3. An ACL entry is cheap.** `ADDRESS_HASH_LEN` is 16
  (`crates/retinue/src/hash.rs:24`). Sixty-four entries is 1,088 bytes against a
  firmware that already carries `destination_capacity: 4_096`
  (`crates/retinue/src/announce_admission.rs:51`).
- **PD-F4. The bounded-table precedent exists.** `AnnounceIngressPolicy`
  (`announce_admission.rs:15`) carries explicit capacities and counters and
  refuses at the limit.
- **PD-F5. Announce headroom is roughly 60 to 70 bytes.** Structure is
  `[ratchet(32)] || signature(64) || app_data(*)` (`announce.rs:9`). A Retinue
  announce measured 185 bytes unstuffed in the 2026-08-23 peer matrix, against a
  boot profile of SF11 at 250 kHz (`selvage/src/lib.rs:327`) capping a LoRa
  payload near 255. One group-encrypted blob fits; one blob per identity does
  not.
- **PD-F6. Coarse degrades under observation.** Repeated coarse broadcasts heard
  by several receivers logging RSSI are trilaterable. The apparatus that defeats
  coarse is the same one PD6 builds.
- **PD-F7. Authority is already separated.** FS4 defines config authority (warm,
  FS2 envelope, cannot replace firmware), firmware authority (offline) and
  publisher authority (packages). The ACL is config authority. FS2 is closed in
  software.
- **PD-F8. No GNSS support exists.** Zero matches for `gnss|gps|nmea|ublox`
  across `crates/`, `firmware/` and `apps/`.
- **PD-F9. Revocation is eventually consistent.** A node unreachable since a
  change still honours the old grant. Inherent to any offline ACL. A validity
  horizon on the projection would make a stale table degrade rather than persist,
  and is an open question rather than a decision.
- **PD-F10. The seizure paragraph forbids what PD1 first proposed.** The
  field-node security posture's design-target paragraph states that "contacts
  ... live in signalman on the host, never on flash." A plaintext table of
  trusted address hashes on the node is a contact graph on flash and violates
  it. Found 2026-09-02 while drafting PD0 into that document; missed by the
  2026-09-01 review pass, which read the FS gate definitions and not the
  seizure paragraph above them. Resolved by blinded storage, with the residual
  membership-test cost added to the paragraph as an inventory item.
- **PD-F11. The host cannot blind, so the record has two encodings.** The
  owner compiles the table on the host from gazette, but the blinding secret is
  node-local and the host does not hold it. So the wire record
  (`PositionAclV1`) carries plaintext sixteen-byte hashes under the FS2
  envelope, and the node blinds at write time into the stored table
  (`BlindedPositionAcl`), which holds HMAC-SHA256 tags truncated to sixteen
  bytes, domain-separated with `retinue.position-acl/v1`. Flash never holds the
  plaintext. Found 2026-09-02 while implementing PD1; `hmac` and `sha2` were
  already dependencies of `radio-hand`, so no crate was added.
- **PD-F12. The V4 GNSS socket is fully mapped and already powered.** Heltec's
  V4 carries an SH1.25 8-pin GNSS socket for its L76K module. Its factory and
  LoRaWanGPSTime sketches use `Serial1.begin(9600, SERIAL_8N1, 39, 38)`, where
  Espressif orders the last pins RX then TX: module TX enters ESP RX GPIO39 and
  ESP TX is GPIO38. The read-only driver owns only GPIO39, so it cannot drive
  the module's RX net. GPIO34 enables the VGNSS-controlled GNSS rail active low;
  reset GPIO42 and standby GPIO40 are held high to run and keep the module awake.
  Vext GPIO36 instead powers the OLED/external rail and is unrelated to GNSS.
  Verified 2026-09-03 against Heltec's V4 documentation and sketches; the July
  embedded-Rust record had deferred GPS explicitly.
- **PD-F13. `status` is not live state, so PD3 needed its own probe line.** The
  board's `status` reply is two prebuilt byte strings, online and identity. The
  `timebase` handler at the same site formats live state into a reply, and the
  `gnss` line now mirrors it: `absent`, `nofix`, or `fix` with integer
  coordinates, plus parser counters (accepted, dropped, successful bytes read)
  and a saturating UART-error total. A nonzero error total exposes line activity
  even when no successful bytes arrive, including likely framing errors from a
  wrong baud; zero successful bytes alone is not a silence or wiring verdict.
- **PD-F14. The parser lives in `radio-hand`, not the firmware, on purpose.**
  `radio_hand::gnss::NmeaParser` is `no_std` and host-testable; the V4 task in
  `firmware/heltec-v4-phy/src/gnss.rs` only owns the UART and pins and feeds
  bytes. Every sentence shape is therefore under `cargo test` rather than only
  provable on metal. Under the default `host-usb` feature `main.rs` has no UART
  import in scope, so the GNSS block uses fully qualified `esp_hal::uart` paths
  rather than adding an import that would duplicate the `host-uart-low-power`
  one.
- **PD-F15. The V4 release build is not reproducible byte for byte.** A rebuild
  after adding a single `#[allow(dead_code)]` produced a different ELF hash.
  Any on-metal claim therefore names the exact image flashed, and the final
  receipt is taken against a fresh build of the committed source, not against
  whatever happened to be on the board during development.

## Open questions

- Whether a validity horizon belongs on the projection, and what a node does
  when it expires: fall back to the broadcast tier, or refuse all directed
  answers.
- Whether the group-encrypted announce blob is worth its revocation cost.
  Removing one identity means rekeying the whole set. It buys passive precision
  for kin without a round trip, and nothing else.
- Whether coarse quantisation is fixed or owner-selectable, and if selectable,
  whether the selected grain is itself disclosed.
- How the projection is authored. `gazette` is a mere port and a headless node
  has no host, so the compile step lives with Signalman or the resident, and that
  boundary is not yet drawn.

## Sequencing note

PD0 and PD1 are unblocked and cheap. PD3 touches `firmware/heltec-v4-phy` and
`firmware/t114-phy`, both of which had substantial uncommitted work from other
sessions on 2026-09-01, so it should be scheduled against that lane rather than
started opportunistically.

## Progress

- **2026-09-01.** Plan written. No code. Findings PD-F1 through PD-F9 verified
  against the tree on this date.
- **2026-09-01, review pass.** One citation corrected (PD-F3 cited
  `announce_admission.rs:52`; `destination_capacity` is at line 51). Three
  safety arguments that were implied are now stated: PD2 records *why*
  no-rollback is safe (the ACL never governs command authority) and adds a
  lockout test to its done-condition; PD1 requires capacity refusal to surface
  at projection time and defines the no-table state; PD6 requires the placed
  node's own position in the record. No gate changed status.
- **2026-09-02. PD0 drafted, not done.** The tier ruling is written into the
  field-node security posture as a feature target, with the seizure paragraph
  amended and a new open question. It awaits Mark's approval before commit.
  Drafting it surfaced PD-F10, which corrected PD1's storage model from
  plaintext to blinded. PD0 closes when the ruling is approved and this plan's
  citation of it stands.
- **2026-09-02. PD0 closed.** Mark approved the ruling as written, including
  the fresh-commission default of off, whitelist semantics as the
  absent-identity default, and blinded-on-flash storage with its stated
  membership-test residual over the host-only alternative. The ruling lives in
  the field-node security posture as feature target PD0; this plan cites it
  and that citation stands.
- **2026-09-02. PD1 and PD2 implemented in software.**
  `crates/radio-hand/src/control/position_disclosure.rs`, registered in
  `control.rs` beside `public_configuration`. Nine tests cover canonical
  round-trip and sort-on-construction, rejection of every non-canonical form
  (length, version, absent-policy byte, tier byte, unsorted, duplicate),
  capacity refusal at both construction and decode, the no-table state, the
  absent-identity default and its `Fixed` alternative, that the stored form
  holds no plaintext hash and a wrong secret does not resolve a grant, PD2
  monotonic acceptance with replay refused and counted, and the PD2 lockout
  test: revoke, replay does not re-grant, refuse everyone including the owner,
  then a higher-sequence correction lands. The full `radio-hand` suite is 170
  tests green with the module registered. **One PD1 caveat, stated rather than
  glossed:** capacity refusal is an `Err` from a stateless decode and is not
  counted on the table; the table's counter counts sequence refusals only.
  Counting capacity refusals belongs to WN's inbound runtime, which receives
  the command, and projection-time surfacing belongs to the host compiler,
  neither of which exists yet. Nothing has run on a board.
- **2026-09-02. PD3 implemented; on-metal proof blocked on module presence.**
  Parser in `radio_hand::gnss` with seven host tests, including one that guards
  the hand-typed fixture checksums (it caught all three being wrong).
  `GnssState` and `GnssFix` added to `radio_face::LocalStatus`, defaulting to
  `Absent`; both firmware crates build for their real targets, the V4 for
  `xtensa-esp32s3-none-elf` and the T114 for `thumbv7em-none-eabihf`. The V4
  task owns UART1 on GPIO38/39 with enable, reset and standby held, and the
  latest state is folded into every status publish at `ui::publish`. A `gnss`
  probe line answers over USB with state plus parser counters. Flashed to the
  COM7 V4 as image `68476581…`: the board boots, identity and region intact,
  and the probe answers `gnss=absent accepted=0 dropped=0 bytes=0`. That old
  image had UART1's directions reversed and discarded `RxError`, so its zero
  successful bytes cannot rule out baud mismatch, electrical activity, or a
  seated module. Whether an L76K is physically seated in COM7's socket, or on
  the COM6 board instead, remains an on-metal question.
- **2026-09-03. PD3 UART correction and diagnostic refinement.** Heltec's
  official V4 sketches establish UART1 RX GPIO39 / TX GPIO38, correcting the
  previous reversed mapping. The firmware now owns RX GPIO39 only and leaves
  GPIO38 undriven. The probe adds a saturating total `errors` counter to its
  absent/nofix replies, so `bytes=0 errors>0` records reception failures rather
  than a silent line. This is source-level only: it has not been flashed or
  measured on a board.
- **2026-09-03. PD3 V4 physical UART/parser receipt.** The corrected locked
  release was inspected and planned without warnings, then flashed through
  Linkboy to the COM6 Heltec V4.2 using a verified `espflash` 4.5.0 helper.
  The board returned `gnss=nofix accepted=40 dropped=5 bytes=1885 errors=0`
  immediately after the transfer, and `gnss=nofix accepted=22 dropped=5
  bytes=1120 errors=0` after readback/reset. Those nonzero accepted sentences
  and zero UART errors prove the corrected UART/module/cable and parser path;
  they do not establish a satellite position fix. The board was indoors and no
  `gnss=fix` was measured. PD3 therefore remains open pending an outdoor fix
  and loss-of-fix observation. Exact flash, preservation, probe, and control
  facts are in [the receipt](2026-09-03_v4_gnss_rx_fix_receipt.json).
