# Compact signed feed and local control plan

**Date:** 2026-08-25. **Status:** plan; CF0/CF1 implemented and locally
verified; later families remain unstarted.

**Authority:** the
[permissive protocol survey](2026-08-25_permissive_radio_protocol_compatibility_survey.md)
owns the external pins and licence ledger. The
[listener executive plan](2026-08-10_listener_executive_and_protocol_leases.md)
owns radio arbitration and its open LE1/LE2 gates. Mere's
`design_docs/mere_docs/implementation_strategy/2026-07-24_low_power_managed_network_plan.md`
owns Personae/Notochord session admission and Stickleback remains Mere's
replicated-space authority.

## Finding

There are three useful mechanisms here, not one new universal protocol:

1. an exact, compact signed-feed implementation derived from tinySSB's pinned
   MIT wire;
2. a privileged local radio-control protocol, using ULCP's device/host custody
   split and command/property/stream discipline; and
3. a live secure attachment that binds a host identity to one local session.

The feed is durable and connectionless. Noise secures one live attachment.
Local control asks the executive to act. They compose, but none substitutes for
the others.

The first safe code is the signed-feed verifier. A radio personality cannot
land honestly until LE1 and LE2 replace the current boot-selected channel with
the resident executive and revocable adapter leases. A local-control cutover
must account for the existing direct-PHY host tools and receipts. Noise needs a
measured embedded proof before it becomes a firmware dependency.

## Rulings

- Implement the pinned tinySSB feed exactly before extracting a generic feed
  abstraction. `tinyssb-core` is a working package name, not a claimed crate
  name.
- Keep that leaf `#![no_std]`, allocation-free, MIT-licensed with upstream
  attribution, and dependent on no Retinue or Mere crate. Hosting it in the
  Retinue workspace must not make Mere pull MPL code.
- The tinySSB message id and original signed frames remain the foreign object's
  identity. A Mere wrapper or gateway receipt must never be presented as the
  original author's signature.
- Stickleback does not depend on tinySSB. A Mere domain or probe supplies the
  foreign-record grammar and admission policy through Stickleback's existing
  extension points.
- Retinue's existing signed in-band command verifier remains the command
  authority. Feed entries are durable facts, not an alternate command path.
- Personae owns the host's identity, derived keys, delegation and revocation.
  The radio owns its device key and stores only the host public identity and
  narrowly provisioned symmetric material needed for configured offline work.
- Chatelaine is currently vocabulary for secret items, not a firmware key
  store. Castellan may later provision or exercise material from the host;
  Retinue firmware still enforces non-exportability, host replacement and
  sleep-time behavior.
- Mere's existing Noise behavior and Notochord transcript binding are prior
  implementation seams. Retinue must not invent a second identity meaning for
  the same handshake, but its embedded implementation cannot depend on Mere's
  Tokio transport crate.
- Durable feed entries remain independently verifiable after a Noise session
  disappears. Persisting Noise transport state is not delayed decoding.

## Ownership map

| Layer | Owner | Responsibility |
| --- | --- | --- |
| Personae/Notochord | Mere host | identity, derived-key attestation, delegation, revocation, owner admission |
| secure attach | local-control host and radio | authenticate one attachment, bind identities to its transcript, derive session keys |
| local control | Retinue host and firmware | commands, authoritative properties, raw streams, attach epoch and bounded transactions |
| feed core | permissive leaf crate | exact frame parsing, signature and chain verification, side-chain integrity |
| radio personality | Retinue adapter | GOset/WANT/CHNK/DATA state and bounded lease requests |
| foreign-source domain | Mere consumer | admission, storage, receipts and projections over unchanged tinySSB objects |
| Stickleback | Mere | replicated-space processing, policy-before-insert, retention and host sync |

## CF family: exact signed-feed core

### CF0. Pin and fixture wall

Pin tinySSB at
[`39896b72c97b51159d46610c5f11ff7f5a279031`](https://github.com/ssbc/tinySSB/tree/39896b72c97b51159d46610c5f11ff7f5a279031).
Record every imported fixture's source path, upstream revision, SHA-256 and
evidence class. Exclude the Android tree and its LGPL Codec2 subtree.

Freeze the observed v0 facts used by the core: 120-byte packets, 32-byte feed
ids, 20-byte message ids, seven-byte DMX, big-endian sequence, the
`tinyssb-v0` domain, plain and side-chain entry types, Ed25519 signatures, and
100-byte side-chain payload plus 20-byte successor hash.

**Done when:** a fixture manifest identifies at least one valid plain entry,
one side-chain entry and chunk sequence, and independently corrupted variants;
no fixture depends on a prohibited source.

### CF1. Allocation-free verifier

Add the leaf crate with fixed representations for feed id, message id, main
frame, chunk frame and frontier. It verifies only the next entry for a supplied
frontier:

- derive and compare the expected DMX;
- verify the Ed25519 signature;
- enforce sequence and previous-message continuity;
- derive the next message id;
- return either complete inline content or a bounded side-chain requirement;
- verify every chunk hash and refuse a chain beyond the caller's declared
  bound.

The core owns no clock, entropy, persistence, retry, radio I/O or dynamic feed
table. It does not reuse Reticulum, Personae or device identities as tinySSB
feed identities.

**Done when:** pinned-source fixtures round-trip byte-exactly; bad DMX, signature,
predecessor, sequence, chunk pointer, declared length and capacity are refused;
the crate passes host tests and
`cargo check --no-default-features --target thumbv7em-none-eabihf`; dependency
and symbol inspection show no allocator.

#### CF0/CF1 implementation findings

- The permitted pinned ESP32 core carries the wire implementation but no raw
  packet corpus. `tinyssb-core` therefore records deterministic synthetic
  vectors, their checksums and their exact pinned MIT source inputs in its
  fixture manifest. The prior CF1 wording, "upstream fixtures", was
  contradictory with the available permitted source and is corrected above to
  "pinned-source fixtures".
- A fresh pinned v0 frontier uses the first twenty bytes of the feed id as its
  predecessor, not an all-zero message id. The leaf crate fixes that behavior
  and tests it directly.
- The verifier has fixed 120-byte frame values and caller-owned frontier and
  chunk cursor state. It deliberately adds no feed table, persistence, radio
  personality, local-control or Noise dependency.

#### CF0/CF1 progress

- Added the MIT, `#![no_std]`, allocation-free `tinyssb-core` workspace leaf
  with exact v0 main-frame and chunk verification.
- Added checksumed valid plain and side-chain fixtures plus corrupted DMX,
  signature and chunk variants. Their manifest excludes the Android and LGPL
  Codec2 trees and attributes only the pinned MIT ESP32 core.
- Host tests, allocation/symbol inspection and the embedded target check are
  recorded with this implementation's handoff; they do not advance CF2 or any
  radio, local-control or secure-attach family.

## MF family: Mere foreign-source composition

### MF0. Probe before platform API

Build the first consumer as a Mere probe or domain-owned example rather than a
new Stickleback primitive. Store an accepted object as the original 120-byte
entry, its complete side chain, feed id, sequence and tinySSB message id, plus a
separate local verification/import receipt.

The outer Stickleback operation identifies the importer and admission event.
It does not re-author the embedded feed entry. Projection may expose content
only after the original signature, predecessor chain and complete side chain
verify.

**Done when:** duplicate import is idempotent; a modified byte changes or
invalidates the foreign identity; incomplete content remains unavailable; two
Mere peers replicate the wrapper and recover the same original object and
verification result.

This proves tinySSB as a foreign signed source. It does not yet prove that a
native Mere operation should use tinySSB as its authoritative log.

## LC family: ULCP extraction

### LC0. Decide exact wire versus donor shape

Before changing firmware, compare the current direct-PHY USB grammar with the
pinned UMSH ULCP v3 minimum surface: bounded transaction ids,
command/property/stream framing, capability discovery, authoritative property
publication, radio clamps, raw frame metadata, reset reporting and HDLC-Lite.

Prefer exact ULCP framing if Retinue can honestly meet its minimum conformance
and gain host-tool interoperability. Otherwise name the Retinue protocol
separately and document every deliberate divergence. Do not call a merely
ULCP-shaped protocol ULCP.

The state split is mandatory either way:

- **device domain:** device identity, region and radio profiles, listener
  registry, participation, duty ledger and persisted board settings;
- **host domain:** one transient attached host public identity, filters,
  inbound queue policy and narrowly provisioned channel/pairwise material;
- **session state:** transaction ids, attachment epoch and live stream state.

Administrative attachment cannot claim the host domain. Replacing the tethered
host clears the host domain while preserving the device domain. Host private
keys are never provisioned, and provisioned secrets are never readable back.

**Done when:** the decision is fixture-backed; host and firmware parsers agree
under arbitrary fragmentation; clamped settings publish the value actually in
force; attach reset and host replacement clear exactly their owned state; the
existing direct-PHY consumer matrix passes through one atomic cutover rather
than a permanent parallel protocol.

## AT family: secure local attachment

### AT0. Embedded Noise feasibility and binding

Mere currently speaks `Noise_XX_25519_ChaChaPoly_BLAKE2b` through `snow` and
then exchanges an Ed25519 proof over the handshake hash. Notochord separately
binds verified Personae claims to carrier facts. Preserve those meanings.

Prototype a sans-I/O host/device transcript using `snow` with `std` disabled.
The radio proves its device identity. The host proves a Personae-derived local
control key and its fixed derived-key attestation. Pairing policy decides which
Personae master is allowed to attach; the radio does not embed the Personae
vault or the full Notochord policy evaluator.

Noise protects ordered local-control frames. It is not placed around stored
feed entries or radio personality frames. USB physical-possession-only mode,
Noise over USB, and BLE requirements remain explicit owner settings rather
than one hardcoded assumption.

**Done when:** host and `no_std` transcripts match; a proof replayed into a new
handshake fails; the wrong Personae master fails; reconnect creates a new
session epoch; malformed or stalled handshakes have fixed byte, allocation and
time bounds; T114 flash, static RAM and heap high-water deltas are recorded.

### AT1. Keep carrier and secure-channel facts distinct

Mere currently records `authenticated_initiator` as identity proved by the
carrier. Reticulum correctly supplies none there, while preserving its shared
link id. Knot's current publishing carrier then requires that outer carrier
identity to equal the inner Noise peer, which structurally rejects Reticulum
plus Noise.

Add a separate secure-channel fact containing the proven Noise peer and a
channel-binding hash. Notochord's transcript may bind both that fact and the
Reticulum shared-link id; it must not relabel a Noise identity as carrier
authentication. Local interface identity remains responder-owned policy
context rather than signed common transcript material.

**Done when:** p2panda plus Noise still detects outer/inner identity mismatch;
Reticulum plus Noise admits a correctly bound Personae subject despite the
carrier having no authenticated peer; replay on another Reticulum link fails;
Knot and Murm consume the same fact vocabulary.

## TP family: exact tinySSB radio personality

### TP0. Adapter after LE1 and LE2

After the listener executive and revocable lease contract land, implement the
exact GOset, WANT, CHNK and DATA replication state over CF1. Register tinySSB's
exact LoRa receive profile, including sync word `0x58`; it consumes measured
scan dwell like every other profile.

The adapter emits bounded actions and lease requests. A malformed packet,
oversized set, saturated DMX table or incomplete side chain cannot extend a
lease or grow state. Code may remain flash-resident while inactive tables are
constructed only for configured participation.

**Done when:** an untouched pinned tinySSB peer exchanges feeds, requests and
side chains in both directions; malformed traffic cannot prolong a lease; the
executive returns to the full scan plan by deadline; a two-radio RF receipt
records profile, airtime, miss rate, flash, static RAM, heap high-water and
durable-store consumption.

## MC family: a native Mere compact carriage

### MC0. Separate forcing-consumer decision

Establish the control case first: two root-signed p2panda operations, sequence
zero and sequence one with a backlink, retain identical header/body bytes and
operation ids across memory/IP and a Retinue Link or Resource loopback.

First test whether upstream `p2panda-core` can become `no_std + alloc` without
changing its wire. If that cannot land cleanly, a temporary permissive Mere
wire crate is acceptable only if desktop Stickleback consumes it too. A
firmware-only reimplementation would create two canonical encoders.

After MF0 and one real low-power Mere consumer exist, compare three choices:

1. carry unchanged signed p2panda operations inside compact side chains;
2. define a shadow-header codec whose decoded object retains the canonical
   p2panda operation id; or
3. admit tinySSB feeds as a distinct Mere source truth.

Measure signature/header duplication and airtime before choosing. Do not
silently turn tinySSB into a second generic replication authority beneath
Stickleback.

**Done when:** firmware and host produce the same p2panda bytes and ids; changed
signature, body, writer, sequence or backlink is refused; replay through two
carriers stores one operation; carrier, fragmentation, link and session facts
do not change object identity; one canonical object then survives LoRa to IP
without gateway re-signing being mistaken for authorship, and the second
consumer forces which compact shape is reusable.

## Personality storage and compression

The active executive plan explicitly keeps several protocol stacks resident in
flash. The 2026-08-25 T114 baseline measured:

- `.text`: 217,040 bytes;
- `.rodata`: 64,388 bytes;
- `.data`: 368 bytes;
- `.bss`: 79,948 bytes, including the fixed 48 KiB heap;
- application flash window: 802,816 bytes; and
- usable RAM window: 232 KiB.

The current image therefore has substantial flash headroom. Compressing code
would require a RAM overlay or boot-time image replacement because the nRF52840
executes ordinary code from flash. That is not justified by the present
figures. Record the section delta for every new adapter and keep inactive
runtime state cold; revisit compressed overlays only when measured flash
pressure, not protocol count, forces them.

## Licence and clean-room gate

- tinySSB core at the pin above is MIT; retain its notice and provenance.
- UMSH at `3bab31881190e0b689ee48a904ad99d5a8a25d65` is MIT OR Apache-2.0 for
  the inspected first-party core and protocol material; its unlicensed patched
  BSP dependency remains excluded.
- `snow` 0.10.0 is MIT OR Apache-2.0.
- GPL, AGPL, LGPL and differently licensed subtrees are reported and stopped.
- Python RNS and LXMF implementation source remains black-box. Only public
  prose and observed bytes may inform Retinue.

Every fixture and donor note carries `official-doc`, `observed-wire`,
`clean-donor`, `peer-output`, or `blocked-source` provenance.

## Findings

- **2026-08-25:** Stickleback already owns the host replicated-space mechanics;
  adding a generic feed runtime there would duplicate authority.
- **2026-08-25:** Mere already has a Noise XX session layer and a separate
  Notochord/Personae admission transcript. Retinue has neither in firmware.
- **2026-08-25:** Knot currently requires its carrier-authenticated peer to
  equal its inner Noise peer. Reticulum honestly has no carrier-authenticated
  peer, so Reticulum plus Noise needs a separate secure-channel fact rather
  than weakening that field's meaning.
- **2026-08-25:** `snow` supports `no_std` without its `std` feature but still
  uses `alloc`; the T114 proof must measure its fixed 48 KiB heap rather than
  assuming allocation-free operation.
- **2026-08-25:** p2panda operation ids already supply the canonical native
  Mere identity across carriers. `p2panda-core` 0.7.0 is permissive but is not
  currently a `no_std` crate, so its portability is a measured spike rather
  than an assumed firmware dependency.
- **2026-08-25:** LE1 and LE2 are open. The current firmware source still
  contains boot-selected `Channel` personalities, while the authoritative plan
  replaces them with resident adapter leases. CF0/CF1 can proceed without
  crossing that live migration.
- **2026-08-25:** the measured T114 image uses roughly 282 KiB of its roughly
  784 KiB application flash window. Personality compression is not currently
  forced.

## Progress

- **2026-08-25:** permissive sources and live Retinue/Mere seams audited; plan
  written. No implementation or wire migration started.
