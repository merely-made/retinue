# Position disclosure

**Date:** 2026-09-01
**Status:** plan. No code written. Nothing here is receipted.
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

NMEA over UART on boards that have a module. The only genuine code gap: `grep`
for `gnss|gps|nmea|ublox` across `crates/`, `firmware/` and `apps/` currently
returns nothing.

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
