# Wall node and carrier-agnostic management implementation plan

**Date:** 2026-08-30
**Status (2026-09-01):** active plan. WN0 is partial: the contract was
deliberately revised to `RHC0` v2 before shipment, with `transaction_sequence`,
but target execution of its shared vectors remains unreceipted. WN1 is partial:
the portable async durable runtime, `RHC0` v2 and `RHD1` durable version 3
authority, outer-counter persistence, mutation-sequence watermark, safe
one-result retry eviction, public configuration/recovery validation, a
host-proven quiet-window contract, the bounded V4 receive-phase cancellation
fix, an always-compiled fixed-size public-identity validator, a portable
first-owner durable-state constructor, and the portable full-state first-write
staging/arbitration model exist. The current worktree also has a physical USB
claim-only slice before SplitHost/RNode/RX/power arm, plus a host-side
`postilion::control::first_owner` controller and `signalman-first-owner`
literal USB bench entry. Its fixed allocation-free
Inspect/Claim/Resume/Abandon codec verifies an Ed25519 proof over version,
domain, opaque board `NodeId`, fresh 32-byte true-RNG nonce, and full canonical
`OwnerClaim`; the claim establishes one implicit Owner. The separate writable
pending A/B pair remains at `0x3F6000`/`0x3F7000`. Staging is read back; resume
writes ordinary control before pending cleanup and repairs corrupt control with
outer-record sequencing preserved, including `u32::MAX`; abandon is permitted
only while ordinary control is blank. Blank+blank with no witnessed mutation
retains ordinary modem/RNode compatibility. Unresolved pending, corrupt,
storage, or runtime uncertainty is status-only outside the bounded claim-only
path. WN1 remains Partial: independent V4 review is GO after two P1 fixes; the
host-side first-owner controller and literal USB bench entry now exist; and the
Retinue runtime, board key vault/sealing, and target or on-device proof remain
open. WN2 through WN8 are Open.
**Supersedes:** the LB1 through LB6 ladder in the archived
[Bluetooth capability scoping brief](archive_docs/2026-08-30/2026-08-11_bluetooth_capability_scoping.md).
It narrows, rather than replaces, the
[listener executive](2026-08-10_listener_executive_and_protocol_leases.md),
[Signalman management plan](2026-08-15_signalman_management_surface_implementation_plan.md),
and [Linkboy flashing plan](2026-08-08_linkboy_public_flashing_plan.md).

This is a separate implementation plan because the feature crosses the
embedded node, board control, Reticulum interfaces, safe configuration,
firmware activation, and Signalman. Appending it to Signalman's presentation
plan would give UI code authority over firmware. Appending it to the old BLE
brief would preserve the false premise that Bluetooth is the feature rather
than one management carrier. The retired brief moved to the archive in the
same change, so this does not create a second active plan for the same work.

## Goal

The first appliance is one Heltec WiFi LoRa 32 V4, powered from the wall:

1. A laptop or phone discovers it locally over BLE. USB is the local fallback.
2. The owner claims it, selects a regulatory region and LoRa profile, supplies
   WiFi credentials, and enables Reticulum transport.
3. The controller disconnects. The V4 continues routing Reticulum traffic
   between its SX1262 LoRa interface and its WiFi Reticulum interface.
4. Later administration can arrive over BLE, USB, local IP, or an authenticated
   Reticulum Link. A tower node remains manageable while any routed management
   path survives.
5. A risky change is provisional. The owner may confirm it through any
   surviving carrier. If confirmation does not arrive, or power fails while
   the change is provisional, the node returns to its last known-good
   configuration.

The second appliance shape adds several resident protocol adapters under the
radio executive. It keeps Reticulum, MeshCore, Meshtastic-compatible, and other
destination namespaces distinct. It may route Reticulum through a foreign mesh
as an explicit bearer, or let Signalman originate a new message in another
protocol. It does not synthesize one cross-protocol address space.

## Verdict

Build Retinue as the standalone V4 option and make Signalman a thin controller
for the same board protocol. Reuse Prns platform work where its public seams
fit, especially mobile Bluetooth, but retain one control contract and one
board authority in this repository.

The fastest useful trunk is Reticulum over native LoRa plus WiFi, with USB,
IP, and Reticulum management. BLE follows as bootstrap and nearby recovery.
Safe configuration is part of that trunk, not polish. Firmware selection and
resident multi-protocol scheduling are later, independent capabilities.

## What can satisfy the wall-node conditions

This table is a repository-grounded baseline, not a fresh upstream capability
audit. The current pins and provenance rulings live in the
[compatibility survey](2026-08-25_permissive_radio_protocol_compatibility_survey.md).

| Option | One V4 alone | LoRa plus WiFi transport | Laptop or phone setup | Safe remote management | Resident foreign personalities |
| --- | --- | --- | --- | --- | --- |
| Stock RNS 1.5.2 plus a V4 in RNode mode | No. RNS and Transport live on an attached host; the V4 is the radio modem. | Yes, when a small Linux or other host supplies the WiFi and RNS interfaces. | Host administration can do the job, but the V4 is not the WiFi-provisioned appliance. | Host-specific; this transaction and rollback contract is absent. | No. RNode is an exclusive host-controlled radio mode. |
| Prns | Plausible, but unreceipted on the current V4 and current RNS 1.5.2 baseline. | Its embedded and host interface family is the closest external fit. | Strongest reusable mobile and Bluetooth work in the surveyed set. | The Retinue control transaction, recovery-set rule, and Linkboy authority still need to be added. | Not Retinue's resident Tucket/Sennet controller. |
| microReticulum 0.5.0 | An embedded transport is possible. | Requires a board integration and current interop proof. | No accepted Signalman-style provisioning path in this program. | No accepted cross-carrier transaction and rollback contract. | Its global transport ownership conflicts with the resident executive; its recorded compatibility target is old. |
| Current Retinue firmware | Not on the V4. The current V4 exposes modem and RNode channels; native node answers unavailable. | Native LoRa transport is proven on the T114. Retinue WiFi and BLE firmware are absent. | USB text commands only. | Absent. | Scan-plan foundations exist, but the current image still boots one Channel. |
| Target Retinue appliance | Yes. | Native LoRa and WiFi interfaces under one transport node. | Same control protocol over BLE and USB, with a laptop and at least one phone face. | Yes, over IP or Reticulum as well as local carriers, with provisional apply and rollback. | Later WN7 profile, gated by the listener executive and measured radio physics. |

A stock RNS host plus a V4 RNode remains the practical companion-computer
alternative. It can meet the networking need today if the one-board condition
is relaxed. It is also a useful independent acceptance peer. It is not the
standalone V4 product.

## Corrections to the existing vocabulary

### Retinue is not the radio executive

`radio-hand` owns the resident board executive, radio schedule, leases, and
board control state. `retinue` is the Reticulum protocol implementation and
the RNS adapter/router running under that executive. This corrects the older
listener document's shorthand.

### Management carrier and Reticulum interface are separate facts

BLE may expose a control GATT service, a Reticulum Bluetooth Auto interface, a
HostLink byte pipe, or a foreign app service. Those are independent facets.
Likewise WiFi may carry the management protocol, a Reticulum TCP or Auto
interface, or both. A connected bearer does not imply a particular protocol
session.

### Firmware selection and resident multiplexing are orthogonal

The board capability classes are:

- **Simple:** one installed firmware image and one resident capability set.
- **Switchable:** several verified images or slots, with one selected for a
  trial or durable boot.
- **Multiplex:** one running image contains several resident protocol adapters
  scheduled by `radio-hand`.
- **Hybrid:** a switchable board may select among images, one or more of which
  are multiplex images.

One SX1262 still receives under one exact PHY profile at an instant.
Multiplexing buys scheduled coverage with a measured miss rate. Several radios
or several boards buy continuous coverage.

### Selvage does not become a boot manager

The current `selvage` crate is dependency-free LoRa PHY profile and wire
vocabulary shared by host and firmware. Image authorization, flash slots,
trial boot, and rollback do not belong there. Linkboy retains package
authority; a portable update journal owns activation decisions; board firmware
executes them. If “Selvage-capable board” remains product language, it means
the board consumes the shared radio-profile seam, not that `selvage` writes
its flash.

### Bearer, protocol route, and semantic gateway

- A native adapter routes a destination in its origin protocol and namespace.
- A foreign-mesh bearer encloses an opaque Reticulum frame. Reticulum still
  addresses and routes end to end above the foreign mesh's own routing.
- A semantic gateway terminates one protocol message and originates another
  under owner policy. It records both identities and the disclosure boundary.

Only the second is “Reticulum over another protocol.” The third is a trusted
application feature and stays outside Tulle, Retinue Transport, and the radio
executive.

WN0 is carrier-agnostic semantic control, not a claim that every transport is
protocol-compatible. A compatible carrier conveys the authenticated Retinue
command envelope and its bounded WN0 payload. The `VerifiedCommand` to control
conversion is a **handoff**, never a bridge. Reserve bridge or gateway for
terminate-and-reoriginate cross-protocol behavior; a foreign-mesh bearer is
only opaque Reticulum carriage.

Signalman therefore addresses a foreign destination as the typed pair
`{ protocol, native destination }`. A contact may retain several such
endpoints. Selecting a MeshCore endpoint asks the MeshCore provider to
originate there; it does not make a Reticulum destination hash resolve inside
MeshCore. If an incoming Reticulum message is deliberately re-originated to
that endpoint, the action is an owner-configured semantic gateway and receives
its own receipt.

## Runtime shapes

The same V4 firmware may expose these roles when capacity and radio policy
permit:

| Role | Runtime owner | Relationship to other roles |
| --- | --- | --- |
| Standalone Reticulum node | `retinue::Node` under the board executive | First trunk. Transport may use LoRa and WiFi concurrently. |
| Resident adapter controller | `radio-hand` executive plus Retinue/Tucket/Sennet adapters | WN7. One radio is scheduled; adapters never own it. |
| RNode compatibility mode | host-driven `radio-hand::rnode` | Exclusive mode. It suspends resident listening until the host releases it. |
| Selected foreign firmware | boot/update substrate | Replaces the running Retinue image; Linkboy and the catalog must label this explicitly. |

Firmware selection must not be represented as a radio personality. A resident
adapter must not claim the safety properties of an independently rollbackable
image.

## Control contract

### One semantic protocol

Use the existing signed command envelope as the authority layer, with one
bounded semantic payload inside it. The following layouts are illustrative,
not compile-ready Rust:

```text
signed outer command, already owned by retinue::command:
  command version
  target class and 16-byte control node id
  controller key id
  monotonic replay counter
  WN0 opcode
  bounded payload
  signature

inner control request:
  semantic version
  transaction id
  expected known-good config generation
  operation
  bounded arguments

response:
  node id
  transaction id
  known-good config generation
  optional effective or candidate generation
  disposition
  bounded result or refusal
  provisional deadline and commit token, when applicable
```

The request never carries a claimed controller role or a second authorization
proof. The node derives the role from the grant stored for the verified outer
command key. The outer target and response node id are the same opaque 16-byte
control identity, stored in `AddressHash` by the existing command code; that
does not make it a Reticulum destination.

The first operations are capability/status, WiFi scan, owner claim, staged
configuration, provisional apply, commit, revert, reboot, and recovery status.
Firmware stage/activate and resident-adapter policy are later privileged
operations using the same envelope.

The semantic wire contract belongs in the `no_std`, allocation-free
`radio-hand::control` module. `retinue::command` remains the signed outer
envelope. Extract a neutral crate only if a real non-Retinue consumer proves
the current dependency boundary wrong.

### Carrier adapters

- **USB:** the existing HostLink/session shape carries framed control messages.
- **BLE:** a dedicated GATT service carries the same frames and status events.
  ATT fragmentation and bonding stay inside the BLE adapter.
- **IP:** an authenticated, encrypted control listener carries length-bounded
  frames. Plain TCP is insufficient for credential-bearing operations.
  It is distinct from a Reticulum TCP interface.
- **Reticulum:** a registered management destination accepts an authenticated
  Link and control requests. Large responses or package bytes use Resources;
  the control operation itself remains the same transaction.

Carrier authentication may strengthen the exchange, but it never replaces the
signed outer command, transaction identity, generation checks, or the
board-local controller grant. A factory claim path must supply an explicit
physical-presence proof rather than pretending an unowned key was preauthorized.

A command signature supplies authenticity and integrity, not confidentiality.
Any operation carrying WiFi credentials or another secret requires an encrypted
carrier session, or a separately sealed payload bound to the node, controller,
transaction, and operation. Raw LoRa packets, announces, and unauthenticated TCP
must refuse secret-bearing operations. This carrier fact stays in dispatch
context rather than becoming a caller-asserted field in the semantic request.

### Safe configuration state

```text
known-good generation g
        |
        | stage(tx, expected=g)
        v
staged old=g, candidate=g+1
        |
        | persist rollback record, then apply
        v
provisional candidate
        |                         |
        | commit(tx, controller)  | timeout, explicit revert, failed health,
        | from any carrier        | or reboot before confirmation
        v                         v
known-good g+1               restore known-good g
```

Rules:

1. The rollback record becomes durable before a risky setting changes.
2. A commit names the exact node, controller, transaction, candidate
   generation, and commit token. Exact command-byte replay is refused by the
   outer counter. The same semantic transaction under a fresh counter returns
   its cached prior result without repeating the side effect.
3. Confirmation may arrive through any carrier authorized for that controller.
4. Reboot while provisional restores the old configuration before opening
   ordinary network services.
5. A transaction that would remove every owner-approved recovery path is
   refused. Destroying the last path requires a physical-presence recovery or
   factory-reset action.
6. The owner can configure which paths count as remote recovery. BLE presence
   alone does not satisfy a tower node's recovery policy.
7. WiFi credentials are write-only. Status exposes network identity and
   outcome, not the secret. They enter only through a confidential dispatch
   context and are redacted from debug output before WN1 persistence begins.
8. Candidate generations are allocated with checked monotonic increments and
   are not reused after rollback. Exhaustion is a refusal, never wraparound.
9. Commit tokens are fixed-width, controller- and transaction-bound secrets.
   They do not replace a signed command and never appear in status, logs, or
   audit receipts.
10. Durable semantic identity stores a keyed tag over the canonical verified
    request, never plaintext arguments or an unkeyed credential digest. Its
    board-only key is stable across reboot and unavailable to carriers.
11. Retry retention is bounded and explicit. Evicting a cached result must make
    an older transaction refuse as expired or unknown; it must never make the
    old side effect executable again. A one-result cache is therefore only a
    WN1 foundation, not closure of durable semantic retry.

The state machine is independent from firmware activation. Both use the same
principle of staged state, exact confirmation, and rollback, but they keep
separate journals and privileges.

## Ownership and authorization

Factory state advertises only a bounded claim service. Ownership establishment
requires physical presence through a reset/claim window, USB, or an equivalent
board gesture. The first owner installs a controller public key and recovery
policy. BLE pairing is useful transport protection but is not ownership.

Privileges are separate:

- observer: status and public capabilities;
- operator: ordinary configuration and provisional commit;
- updater: verified package staging and trial activation;
- owner: controller roster, identity rotation, recovery policy, destructive
  reset.

All mutable requests are authenticated, replay-bounded, rate-limited, and
recorded in a bounded audit ring without secrets. Reticulum management uses a
Link to the node's management destination and binds the proven remote identity
to a controller grant. Other carriers provide equivalent application-layer
proof.

## Interface scope

The V4 first trunk includes:

- SX1262 direct-PHY LoRa as a native Reticulum interface;
- WiFi TCP as the first embedded IP Reticulum interface;
- WiFi Auto only after multicast discovery and lifecycle are proven on the
  embedded stack;
- USB, BLE, IP, and Reticulum control carriers;
- optional Bluetooth Auto as an independent Reticulum interface after the
  control GATT service is stable.

I2P, Pipe, and OS-owned modem interfaces remain on a companion host. They still
reach the appliance through an ordinary Reticulum interface. “All Reticulum
modalities” means the node composes the modalities its hardware and runtime
can actually own; it does not embed an operating system inside the V4.

## Authority table

| Owner | Owns here | Must not absorb |
| --- | --- | --- |
| `retinue` | Reticulum packet/link/resource semantics, native node, transport and RNS management destination carriage | WiFi credentials, BLE lifecycle, firmware trust, foreign address translation |
| `radio-hand` | board control vocabulary and state machine, durable settings decisions, radio executive, adapter leases | package catalog trust, product UI, protocol-specific routing |
| board firmware crates | ESP/nRF network and BLE stacks, flash partitions, hardware RNG, physical-presence and reboot execution | cross-board policy or package catalog authority |
| `selvage` | shared LoRa profile and direct-PHY wire values | image slots, signatures, boot selection |
| `tulle` | shared radio/interface mechanics and host-facing radio transport | Reticulum routes or semantic gateways |
| Postilion | host station session, bounded snapshots, controller adapter and carrier status | board policy, graph identity, flash authorization |
| Signalman | owner choices, controller roster UX, secret entry, recovery-path policy, status and refusals | radio ownership, Reticulum internals, Linkboy plan construction |
| Linkboy | catalog trust, package verification, immutable update plans, activation policy and receipts | management carrier preference or radio scheduling |
| Prns | optional platform provider/donor and independent peer at a pinned revision | Retinue board authority or unreceipted compatibility claims |

## Phases and gates

Each WN gate is one feature phase with its own validation boundary.

### WN0. Freeze the shared control and capability contract

**Writes:** a radio-free `radio-hand::control` candidate, host/firmware golden
vectors, capability vocabulary, and the minimal Postilion adapter surface.

Define node/controller/transaction identities, transaction sequences, config generations, roles,
operations, refusal codes, recovery-path facts, board class, installed image
slots, resident adapters, radios, and carrier capabilities. Keep every field
bounded and versioned. Recognize a repeated controller/transaction/request as
a duplicate before generation comparison, refuse conflicting reuse, and apply
generation comparison only to mutations. WN1 owns cached-result replay and
durability; WN0 must not claim exact-once execution by itself.

**Done when:** one host test and one firmware-target test consume the same
encoded fixtures; unknown versions and oversized fields fail closed; the
authority table matches Cargo dependencies; and no UI or carrier-specific
field enters the semantic contract. Until the firmware fixture consumer lands,
cross-target compilation is evidence of portability, not closure.

Before shipment, `RHC0` deliberately moved to v2 and added
`transaction_sequence`, so the durable retirement rule is explicit rather than
an inferred transaction-id convention. The target fixture consumer is still
not an execution receipt; WN0 remains partial until that proof is recorded.

### WN1. Implement durable configuration transactions

**Writes:** `radio-hand` control state, an A/B or journaled configuration
store, board-store traits, and a deterministic power-cut model.

Extend the existing torn-write-safe storage pattern rather than creating an
in-place settings write. Persist owner grants, non-secret configuration,
sealed credentials, known-good state, and one provisional transaction with
explicit size budgets. Exercise every cut between erase, write, apply,
confirmation, and cleanup.

**Done when:** model tests prove last-known-good recovery at every cut point;
cross-carrier commit works; replay, wrong controller, wrong node, stale
generation, expired transaction, and changes that fail the owner `RecoveryPolicy`
or its required recovery quorum are refused;
same-transaction retries return the cached response after a fresh signed outer
counter without repeating work; that result survives reboot for durable
transactions; and secret values never appear in status, logs, or receipts.

**Current partial:** the portable async durable runtime now owns `RHC0` v2 and
`RHD1` durable version 3 authority. Version 2 durable bodies fail closed. It
persists outer counters, advances a mutation-sequence retirement watermark, and
retains one safely evicted terminal result. Its transition report distinguishes
**changed** work from a **replayed** prior result. `Ready` is valid only when
the journal is for the expected node and the applied state is known-good;
otherwise recovery is not a normal boot. A poisoned journal means discarding
and rebuilding the in-memory verifier from durable grants and counters.

`PublicConfigurationV1` is an exact 21-byte public body: schema version,
region, enabled management-carrier mask, Reticulum relay flags and hop limit,
and a canonical Selvage PHY command. It deliberately excludes credentials,
identity, board channel selection, and protocol-personality state. The requested
PHY power remains durable; the board `ConfigApplier` is the regulatory boundary
and must derive the region- and hardware-clamped effective profile before it
reaches the radio.

`RecoveryPolicy` separately requires its active physical-presence and
authenticated-remote carrier quorums. Firmware constructs the trusted,
runtime-owned `BoardRecoveryFacts`; a candidate cannot claim those facts from
its carrier mask. An Owner may change configuration only subject to that policy.
An Operator may change Reticulum PHY and transport only; region, carrier, and
sealed-credential changes are Owner-only, while Observer and Updater changes
are refused.

The portable quiet witness is now host-model evidence: every live persist or
apply holds one borrow-scoped board `QuietWindow`/`QuietGuard` through slot I/O
and application. `LiveOutcome` reports whether operation may resume or needs a
hardware reset; `ResetRequired` makes the runtime reset-pending and blocks
further work. Boot runs before radio activation and therefore has no live
witness. This proves ordering in the model, not target execution.

The V4 current worktree now has one `V4RadioOwner` and a bounded
`QuietWindow`/`V4QuietGuard` source seam. A completed-boundary preflight refusal
is retryable before stopping starts. Once entry starts, `ResetOnDrop` covers
entry cancellation, guarded work, and finish failure; the guard lends the
control A/B store and `ConfigApplier`, settles again, restores RX, and returns
`Resumed`. The driver enters standby, clears chip IRQ flags, then proves the
physical DIO1 line low before quiet work or RX re-arm. This is source
implementation, not embedded execution proof.

That V4 `ConfigApplier` deliberately accepts only `ManagementCarrier::Usb`, the
literal ESP32-S3 USB Serial/JTAG carrier, empty sealed credentials, and a
non-relay Reticulum transport. It aligns the configured region before applying
through `Executive`, returns typed refusals for unsupported policy, and resets
directly if a driver fault makes hardware state uncertain.

The current worktree's boot-only recovery seam runs before SplitHost, RNode,
RX, or power arm. Its one-shot pre-radio `V4BootOwner` consumes an explicit
`HardwareResetToken` and uses `StaticCell`-backed scratch outside `main`'s
async task stack. From persisted board identity it derives the opaque control
`NodeId` and separate `SemanticTagKey` through distinct versioned HMAC-SHA256
domains. This is stable control/retry-tag material, not credential sealing. USB
exposes only local physical-recovery facts. `Blank` creates neither durable
state nor authority. If settings are unavailable, boot preserves recovery modem
behavior. Valid control completes the existing normal boot recovery and then
continues ordinary service; a persisted RNode selection remains its exclusive
compatibility personality. Blank control plus blank pending likewise continues
ordinary modem/RNode compatibility. Only pending, corrupt, unsafe, read, or
runtime-failure outcomes are status-only. The control runtime is dropped after
boot. The physical USB claim-only session is offered only after GPIO0 has been
released, newly pressed, and held for three seconds within a 20-second
post-boot window. It is literal ESP32-S3 USB Serial/JTAG, not UART. It permits
at most 45 seconds, eight complete KISS requests, and three parse failures;
wall-time is checked before reads and responses, while read errors or zero
length end safely. Terminal resume or abandon replies precede software reset.
This is still not a live long-term control carrier, verifier-restore dispatch,
native Reticulum node, BLE/WiFi/IP/Reticulum management, or secrets/vault.

`ManagementCarrier::Usb` is not a generic local-wired fact. The default
`host-usb` image advertises its one physical USB recovery fact, while
`host-uart-low-power` advertises an empty fact set. A USB-recovery commissioned
journal therefore fails status-only in the latter image, though `Blank` still
boots. All three locked Xtensa configurations compile; that is compile evidence
only.

Exactly one V4 host transport is compile-enforced. A mixed default plus
`host-uart-low-power` build fails with the intended diagnostic, and USB
claim/recovery code is cfg-closed out of UART. Before Resume can mutate either
journal, the USB path reads the exact pending A/B pair, decodes it with
`load_first_write_state`, and applies the USB-only, empty-credentials,
non-relay feasibility gate. Rejection preserves both pending and control state
and leaves the portable recovery shape intact. Optional Xtensa
`cargo test --no-run` cannot build because the core-only image has no `test`
crate; that existing harness limit is not a required gate or a claim that V4
cfg unit tests ran.

The portable commissioning model now contains `OwnerClaim` with exactly one
fixed-size Retinue public identity, public configuration, and recovery policy;
its first-owner role is implicit. It carries no secrets, credentials, multiple
owners, counters, or transaction artifacts. The public-identity validator is
always compiled, allocation-free, and exact: 64 bytes total, arbitrary X25519
public bytes in the first half, and Ed25519 `VerifyingKey` syntax in the
second half. Durable decode now enforces that validator directly. Only the
verified-command/verifier integration remains `control-retinue`-gated.
`DurableState::from_owner_claim` requires trusted `BoardRecoveryFacts`, reuses
the ordinary durable constructor, creates exactly one Owner grant with both
counters, generation, and watermark at zero, empty sealed credentials, and no
provisional transaction or cached receipt. It refuses board-incompatible
recovery before a persistable state exists.

The portable first-write path is also no longer feature-gated. It stages and
arbitrates the complete canonical initial `RHD1` durable state, not a digest or
parallel claim format, and rechecks exact initial invariants plus trusted board
recovery facts before staging, load, or arbitration. Focused commissioning
tests pass 6, `first_write_staging` passes 8, and the focused durable-model
tests pass 4. Identity conformance now covers 322 full-size comparisons plus a
truncation boundary check.

The V4 now has writable staged first-write A/B storage at `0x3F6000` and
`0x3F7000` before ordinary service.
This is separate from the ordinary control rollback journal at `0x3F4000` and
`0x3F5000`. Valid control wins stale or corrupt pending and continues through
the existing normal boot runtime. Blank control plus blank pending creates no
authority and preserves ordinary modem/RNode compatibility when no witnessed
mutation occurs. Valid pending may resume or abandon through the bounded
claim-only session. Corrupt, unsafe, or failed storage evidence is status-only
before SplitHost/RNode/RX/power arm.

T114 still needs an owner/stop-and-drop refactor. WN1 remains Partial. Open
work is phone UI,
physical on-device GPIO/USB/flash/power-cut/reset proof, package rebuild/flash,
live long-term control carrier/verifier-restore dispatch, BLE/WiFi/IP/Reticulum
management, native V4 Reticulum node/transport, credentials/vault, and
headed/on-air proof. The focused `portable_first_write` receipt passes 12;
base `radio-hand` passes 161; `radio-hand --features control-retinue` passes
170. Rustfmt and three serial locked Xtensa core-only checks pass for default,
`host-uart-low-power`, and `host-uart-low-power+rf-sleep-proof`. The V4 keeps
only `radio-hand` `features = ["radio"]`, with no Retinue runtime,
`control-retinue`, allocator, or `-Zbuild-std=alloc`; its feasibility boundary
is literal USB, empty credentials, and non-relay. Independent portable review
is GO; independent V4 review is GO. This is source, host, and compile evidence
only.

### WN2. Make the V4 a standalone Reticulum transport node

**Writes:** the V4 firmware allocator and native-node assembly, `retinue::Node`
integration, durable announce reservation use, and bounded diagnostics.

Port the proven T114 node shape without copying its board loop. Give the V4 a
fixed and measured allocation budget, retain modem/RNode recovery, set
transport policy explicitly, and make every interface id and queue bound
visible in status.

**Done when:** after controller disconnect and power cycle, the V4 forwards a
three-party RNS 1.5.2 path and payload over direct PHY in both directions;
identity and announce freshness survive reboot; allocator peak and refusal
behavior are recorded; and modem/RNode recovery still works.

### WN3. Add WiFi as a Reticulum interface

**Writes:** V4 WiFi lifecycle, sealed credentials, embedded TCP interface
adapter, interface configuration, and coexistence diagnostics.

Begin with configured TCP client/server behavior. Add Auto discovery as its own
receipt. Keep the control listener and Reticulum interface separate even when
they share the network stack. Measure ESP32-S3 WiFi/BLE/SX1262 memory,
interrupt, power, and packet behavior rather than assuming coexistence.

**Done when:** one V4 transports Reticulum between an RNS 1.5.2 WiFi peer and a
LoRa peer while the controller is absent; restart rejoins without exposing
credentials; loss and return of either interface do not wedge the other; loop
suppression and path choice are observed; and capacity figures are recorded.

### WN4. Carry management over USB, IP, and Reticulum

**Writes:** carrier adapters, a Reticulum management destination assembly,
Postilion client support, authorization bindings, and tower-node receipts.

Every carrier dispatches the WN0 frames into the WN1 state machine. The
Reticulum carrier uses authenticated Links and Resources without inventing a
second command grammar. Carrier changes are ordinary provisional
configuration.

**Done when:** equivalent request/response fixtures pass over USB, TCP, and a
Reticulum Link; a controller changes a remote node through one path and commits
through another; an unconfirmed WiFi, LoRa, or route-policy change reverts; and
a two-site operator can fully inspect and repair the node while one routed path
survives. Credential-bearing requests pass over encrypted carriers and are
refused on raw broadcast or plain TCP paths.

### WN5. Add BLE bootstrap and a phone face

**Writes:** V4 BLE stack integration, control GATT adapter, ownership claim,
WiFi scan/provision operations, Signalman controller client, and one mobile
host integration.

Start on the V4 because it is the target appliance. Use the Prns backend or a
narrower upstream seam when that preserves one platform BLE owner; otherwise
record why a small local adapter is required. Bluetooth Auto and HostLink are
separate facets. The old T114 SoftDevice work becomes a later portability
receipt rather than the opening risk.

**Done when:** a factory-reset V4 is claimed and joined to WiFi from both a
laptop and at least one real phone; join success or failure returns over the
still-open BLE session; the phone can later reconnect locally; removing BLE
does not stop the node; bonding deletion, Bluetooth-off, permission denial,
disconnect, and restart have explicit states; and BLE plus WiFi plus LoRa
capacity is measured on the exact firmware.

### WN6. Add verified image selection without confusing it with adapters

**Writes:** a portable form of Linkboy's release identity/update journal, V4
partition and boot-health integration, control operations, and power-cut
receipts.

Linkboy still authenticates catalogs and constructs immutable update plans.
Any delivery bearer may fill an inactive slot. Only a rollback-capable
activator may trial it without outside recovery. The V4 reports simple,
switchable, multiplex, or hybrid capabilities from measured flash and RAM, not
from board-family assumptions.

**Done when:** a Linkboy-authorized package reaches the inactive V4 slot over
two different carriers; the exact candidate trial-boots; health confirmation
commits it; silence, wrong application identity, timeout, and power loss return
to the confirmed image; downgrade/replay is refused; and ROM-loader cable
recovery remains documented and physically proven.

### WN7. Replace boot channels with resident adapter policy

**Depends on:** LE1 through LE5 and the exact-wire gates for every admitted
foreign adapter.

**Writes:** the `radio-hand` resident adapter registry, lease dispatch,
participation policy, capability/status facts, and Signalman controls.

Retinue becomes one adapter under the executive. Tucket and Sennet retain their
own routing and address spaces. RNode remains an explicit exclusive mode.
Signalman chooses monitored protocols, participation level, required listening
floor, and owner preference; it does not command unmeasured rapid flipping.

**Done when:** one multiplex image schedules at least Reticulum plus one
foreign exact-wire adapter; every radio handoff returns to the declared scan
plan; malformed input cannot extend a lease; per-profile off-time and miss
rates are measured; a second-radio or second-board configuration demonstrates
continuous coverage where one SX1262 cannot; and the UI labels bearer,
exclusive mode, native route, and semantic gateway distinctly.

### WN8. Qualify the appliance profiles

Qualification is split so multiprotocol work cannot block the useful wall
node.

**WN8a, standalone:** factory reset, laptop and phone claim, WiFi join,
LoRa-to-WiFi transport with the controller gone, restart, remote status,
cross-carrier safe change, timed rollback, and tower-distance repair all pass
on one exact V4 artifact.

**WN8b, switchable:** WN6's trial, confirmation, rollback, power-cut, and cable
recovery pass on that artifact and partition table.

**WN8c, multiplex:** WN7's exact-wire, lease, measured coverage, and
multi-radio/flock receipts pass without weakening WN8a or WN8b.

**Done when:** each shipped capability claim names the profile that earned it.
An artifact may ship WN8a without claiming WN8b or WN8c.

## Dependency order

```text
WN0 -> WN1
  |      |
  v      v
WN2 -> WN3 -> WN4 -> WN5 -> WN8a
                   |
DIST7 + WN4 ------> WN6 -> WN8b
LE1-LE5 + WN0 ---> WN7 -> WN8c
```

The first implementation trunk is WN0 through WN5 plus WN8a. WN6 can proceed
after the management carrier and current DIST7 foundation meet. WN7 waits for
the listener executive and foreign exact-wire membership; it does not hold the
standalone transport node hostage.

## Findings

- **2026-08-30:** `crates/retinue/src/command.rs` already owns the
  transport-independent signed authority envelope: node or fleet target,
  controller key id, monotonic replay counter, opcode, bounded payload, and
  signature. WN0 therefore adds an inner semantic payload rather than a second
  identity, role, proof, or replay system.
- **2026-08-30:** `crates/radio-hand/src/control.rs` is a small facade over
  `control/{model,codec,admission}.rs`; every file stays below the repository's
  600-line ceiling. Together they define allocation-free 256-byte request and
  response ceilings, checked generations, opaque verified-controller
  admission, typed capability facts, fail-closed codecs, and shared vectors.
  The admission helper recognizes duplicates but deliberately stores no prior
  response and is not durable; those are WN1.
- **2026-08-30:** `crates/postilion/src/control.rs` signs the inner request with
  `retinue::command`, leaves the signer, node target, and counter caller-owned,
  and refuses a response whose node id or transaction does not match the
  request. Postilion now depends on default, radio-free `radio-hand`; the
  reverse edge does not exist.
- **2026-08-30:** Debug output for inner requests, responses, provisional
  commit tokens, the volatile admission cache, and the outer signed
  `retinue::command::Command` now reports metadata and lengths without payload
  bytes. This reduces accidental logging, but it does not make the signed wire
  confidential; secret-bearing operations still require encrypted carriage.
- **2026-08-30:** the shared golden vectors are consumed by `radio-hand` and
  Postilion host tests. Both T114 and V4 firmware entry points now run a
  fixed-buffer boot check that decodes and re-encodes those same request and
  response bytes. Both targets compile that consumer, but neither check has an
  on-device execution receipt. WN0 remains partial.
- **2026-08-30:** `crates/radio-hand/src/settings.rs` described its persisted
  64-byte private identity in the opposite component order from the actual
  `crates/retinue/src/identity.rs` encoding. The comment now says X25519 secret
  then Ed25519 seed; persisted bytes and behavior did not change. WN0 exposes
  only the 16-byte control node id and no private identity bytes.
- **2026-08-31:** `crates/radio-hand/src/control/durable.rs` now supplies the
  portable async durable runtime and bounded `RHD1` durable version 3 authority;
  a version 2 body is refused. It keeps known-good state separate from one
  provisional change, persists outer counters, advances the mutation-sequence
  retirement watermark, stores owner grants, and retains one safely evicted
  terminal result. `PublicConfigurationV1` is the strict canonical 21-byte
  public configuration codec, while its requested power is not the board's
  effective power. Its transition report distinguishes changed work from
  replayed work. This is WN1 partial, not a firmware apply receipt.
- **2026-08-30:** `retinue::command::VerifiedCommand` is an opaque witness
  produced only after target, allowlist, counter, and signature verification.
  The handoff restores its outer counter from durable state and must rebuild a
  verifier from durable grants and counters after poison, rather than carrying
  a possibly stale in-memory verifier forward. Semantic retry identity is over
  the canonical verified request; request arguments and key bytes are not
  journaled or printed by `Debug`.
- **2026-08-30:** durable semantic validation binds each `ControllerId` to the
  SHA-256 public identity it names. Under `control-retinue`, unparsable public
  identity bytes fail before boot can apply anything; the raw grant constructor
  is crate-private. `Blank` means only that both A/B facts are erased. It never
  grants commissioning authority. A caller must persist an accepted outer
  counter even when a verified command is then refused as malformed, wrong
  opcode, or non-node targeted, and an Observer authenticates but cannot mutate.
- **2026-08-30:** both firmware crates now implement separate control A/B flash
  backends with erase, program, readback, decode verification, and fail-closed
  load. The T114 build script and linker map prove the four adjacent reserved
  sectors. The V4 constants prove sector alignment and fit below 4 MiB, but its
  generic ESP linker script exposes no authoritative physical application-end
  symbol. Package metadata currently owns the `0x3f0000` image ceiling, so a
  build-time overlap guard remains open rather than inferred.
- **2026-08-31, final software-only firmware/package check:** the documented
  release cross-builds passed for T114 (`thumbv7em-none-eabihf`) and V4
  (`xtensa-esp32s3-none-elf`, `-Zbuild-std=core`). Both targets compile the
  fixed-buffer control fixture and their separate stores, but WN1 is still
  fixture/store-only: neither target loads, saves, or runs the control runtime.
  `heltec-v4-current.toml` writes `0x0..0x3F0000` and preserves
  `0x3F0000..0x400000`, covering its settings, reservation, ordinary control
  pair, and writable pending first-write pair. The remaining preserved tail is
  unallocated: it is not a pending claim and no credential-vault range has yet
  been chosen. Its package has no native-node declaration.
  The immutable T114 v51 payload writes only `0x26000..0x69400`, but its
  native-node preserved/guard range begins at `0xE8000`; it therefore does not
  contractually cover the new `0xE6000..0xE8000` control pair. This is compile
  and existing-package inspection evidence only: no WN1 package rebuild,
  flash, physical reset, or on-air proof occurred.
- **2026-08-30:** the first V4 cross-target check after adding HMAC exposed
  SHA-256's default `alloc` feature under the image's `-Zbuild-std=core`
  contract. `radio-hand` now disables that default feature; the exact Xtensa
  check passes without adding an allocator to the radio-only image.
- **2026-08-31:** `RecoveryPolicy` contains independent physical-presence and
  authenticated-remote carrier quorums. `BoardRecoveryFacts` is a runtime-owned
  firmware trust seam, so a candidate mask cannot assert physical presence or
  authenticated remote access. Owner changes remain subject to both; Operator
  changes are limited to Reticulum PHY and transport, and Observer/Updater
  mutation is refused. This replaces the weaker `preserves_recovery` claim.
- **2026-08-31:** the portable `QuietWindow`/`QuietGuard` model holds one
  borrow-scoped quiet witness over live journal I/O and application.
  `ActiveQuietGuard` aborts dropped work after entry; a board-owned entry future
  must itself stop work or latch reset if dropped before it returns a guard. Its
  `LiveOutcome` distinguishes resumed operation from `ResetRequired`, which
  blocks that runtime instance. Its unsafe replacement constructor is reserved
  for the board startup owner once after actual hardware reset. Boot is pre-radio
  and has no live witness. The focused result remains host-model ordering
  evidence, not firmware execution. T114 still needs owner/stop-and-drop work.
  The current V4 worktree now supplies one source-level live owner and bounded
  quiet guard. It is now called only by pre-radio boot, never a control transport;
  its control A/B store is not used by `ControlRuntime`.
- **2026-08-31:** the bounded V4 receive-cancellation slice is implemented in
  the current worktree and was accepted by independent review. V4 RNode now
  does prepare/arm, selects host
  input against `wait_for_irq`, and calls `rx_collect` only after the radio
  wins. The direct V4 and `rf-sleep-proof` paths use the same wait-then-collect
  boundary. V4 low-power DIO1 registration now has RAII drop cleanup and a
  same-poll high handshake, relying narrowly on exclusive GPIO14 and
  SX1262-level-latched DIO1; its pure model is 2/2. The final locked Xtensa
  `rustup run esp cargo check` matrix passed for default,
  `host-uart-low-power`, and `host-uart-low-power+rf-sleep-proof`, with
  `-Zbuild-std=core` and `-j1`. This closes a software receive race only: it
  is not a flash quiet boundary, board owner or `QuietWindow`,
  `ControlRuntime` wiring, or a physical, light-sleep, or on-air receipt.
  RNode remains an exclusive compatibility mode, not a resident lease. WN1
  remains Partial and WN2 remains Open.
- **2026-08-31:** a separate board key vault remains preferable to
  identity-derived long-term sealing. `sealed_credentials` is bounded opaque
  storage, not a sealing implementation. V4 confidentiality still depends on
  secure boot and flash encryption, and T114 credentials remain host-owned.
  The V4 staged first-write A/B pair now occupies `0x3F6000..0x3F8000`; a
  separate credential-vault range remains open and still needs a package/image
  overlap guard once chosen. The V4 Retinue runtime and target/on-device
  power-cut and boot proofs remain open.
- **2026-08-30:** `crates/radio-hand/src/channel.rs` constructs exactly one
  `Personality` for a boot. `profiles.rs` and `executive.rs` already hold
  scan-plan inputs and CAD/capture counters. The target scheduler is partly
  typed, but the current runtime is not multiplex.
- **2026-08-30:** `firmware/heltec-v4-phy/src/channels.rs:41-55` explicitly
  reports `channel node` unavailable. The T114 native node links
  `retinue::Node`; the V4 currently carries no allocator. WN2 is a real
  runtime/allocator gate, not a setting.
- **2026-08-30:** `crates/retinue/src/node.rs:239-268,382` shows that
  `retinue::Node` is executor-neutral and has an explicit transit setting.
  That is the correct core for the V4; the embedded WiFi adapter is missing.
- **2026-08-30:** `firmware/heltec-v4-phy/Cargo.toml` and its source contain
  neither a BLE stack nor a WiFi interface implementation. The old LB brief
  researched candidate stacks but never became an active decision.
- **2026-08-30:** `crates/selvage/src/lib.rs` owns direct-PHY wire constants
  and `PhyProfile`; it has no flash, package, or boot-slot seam.
- **2026-08-30:** `apps/linkboy/src/update.rs:17-160` already distinguishes
  `DualSlotRollback` from `ExternalRecoveryOnly`, and models stage, trial,
  exact confirmation, and rollback. WN6 should make that authority portable,
  not reimplement it under a bearer.
- **2026-08-30:** `apps/signalman/src/management.rs` projects read-only device
  facts. The S0 through S9 plan owns product presentation and actions, but no
  on-device control protocol exists. WN0 through WN5 supply that seam.
- **2026-08-30:** the
  [RNS 1.5.2 re-pin](2026-08-29_rns_152_repin_receipt.md) is green in the
  measured local-TCP, wire, Resource, route-freshness, LXMF, and pinned-Prns
  scopes. Physical RNode captures, RF forwarding, interface-discovery
  metadata, and public-network operation were not remeasured by that receipt.
- **2026-08-31, current worktree:** the portable `OwnerClaim` model accepts
  exactly one syntactically validated Retinue public identity, public
  configuration, and recovery policy. The validator is now always compiled,
  allocation-free, and exact: 64 bytes total, arbitrary X25519 public bytes in
  the first half, and Ed25519 `VerifyingKey` syntax in the second half.
  `DurableState::from_owner_claim` takes trusted `BoardRecoveryFacts`, reuses
  ordinary durable construction, creates one Owner grant at zero
  counters/generation/watermark, and rejects incompatible recovery before
  persistence. The portable full-state first-write path stages the complete
  canonical initial `DurableState`, not a digest or parallel claim. Six focused
  commissioning tests, `first_write_staging`'s 8 tests, the focused durable
  model's 4 tests, and identity conformance across 322 full-size comparisons
  plus truncation pass. Only verified-command/verifier integration remains
  `control-retinue`-gated. This is not a carrier or on-device commissioning
  receipt.
- **2026-08-31:** `firmware/heltec-v4-phy/src/commissioning_store.rs`,
  `control_boot.rs`, and `main.rs` establish the separate V4 first-write A/B
  pair at `0x3F6000`/`0x3F7000`, distinct from ordinary control rollback at
  `0x3F4000`/`0x3F5000`. The later physical USB slice makes it writable, while
  valid control still wins stale or corrupt pending and blank+blank preserves
  ordinary modem/RNode compatibility. V4 remains radio-only, with no Retinue
  runtime, `control-retinue`, allocator, or `-Zbuild-std=alloc`; independent
  V4 review is GO. This is source, host, and compile evidence only.
- **2026-08-31, documentation reconciliation:**
  `crates/radio-hand/src/control/public_identity.rs` validates all 64 public
  identity bytes without allocation, and
  `crates/radio-hand/src/control/durable/model/commissioning.rs` carries the
  canonical initial `RHD1` state through first-write arbitration. The V4
  `control_boot.rs` reads control A/B and pending A/B before service, while
  `commissioning_store.rs` owns writable staged slots at `0x3F6000`/`0x3F7000`. These
  facts keep WN1 Partial and WN2-WN8 Open; the receipts are host/source/compile
  evidence, not embedded execution or physical flash/reset/power-cut/GPIO/USB/
  light-sleep/on-air proof.
- **2026-08-31, physical USB first-write slice:**
  `crates/radio-hand/src/control/durable/model/portable_first_write.rs` owns
  the fixed Inspect/Claim/Resume/Abandon codec and full-state staging.
  `firmware/heltec-v4-phy/src/control_boot.rs` gates a literal USB Serial/JTAG
  KISS session on sustained post-boot GPIO0 presence before SplitHost/RNode/RX/
  power, and checks V4 feasibility before resume changes control storage.
  Portable review and independent V4 review are GO.
- **2026-08-31, V4 re-review:** the source now compile-enforces exactly one
  host transport, and the mixed default plus `host-uart-low-power` build fails
  with its intended diagnostic. USB claim/recovery is cfg-closed out of UART.
  Resume reads and decodes the exact pending A/B pair before the V4
  USB-only/empty-credentials/non-relay gate; rejection leaves both journals
  unchanged. Optional Xtensa `cargo test --no-run` remains unavailable because
  a core-only image has no `test` crate, an existing harness limit rather than
  a required V4 gate.

## Progress

- **2026-08-30:** captured the carrier-agnostic management direction,
  separated firmware selection from resident multiplexing, made BLE bootstrap
  rather than permanent authority, created WN0 through WN8, and archived the
  pre-decision LB brief. Implementation and hardware receipts were open.
- **2026-08-30:** implemented the WN0 semantic codec and capability vocabulary,
  reused the FS2 signed command envelope, added the minimal Postilion signer and
  response decoder, and passed host tests plus T114 and V4 cross-target checks.
  WN0 stays partial because its source-level firmware boot consumers do not yet
  have a target-execution receipt and WN1 still owns durable response replay.
  A later receipt may insert the exact locked validation and target-check
  evidence here; this status line does not treat compilation as target execution.
- **2026-08-31:** extended the WN1 portable durable core to `RHD1` durable
  version 3, strict public configuration/recovery validation, role-limited
  configuration changes, and the authenticated `VerifiedCommand` handoff. The
  host model now proves borrow-scoped quiet-window ordering, post-entry abort,
  and entry-cancellation reset latching. `ResetPending` is instance-scoped;
  replacement is an unsafe startup-owner acknowledgement of actual reset.
  Changed-versus-replayed durable semantics are covered. WN1 remains partial:
  T114/V4 have separate A/B stores but no safe live quiet witness or
  control-runtime wiring; the V4 Retinue runtime, board vault/sealing,
  candidate-image overlap guard, and target/on-device power-cut/boot execution
  receipts are still open.
- **2026-08-31:** the current implementation includes the bounded V4
  receive-cancellation fix. RNode and both direct receive variants now select
  or poll only `wait_for_irq` after
  prepare/arm and collect the frame without racing that SPI work. The DIO1
  low-power registration path cleans up on future drop and performs its high
  handshake in the same poll. Software checks passed for all three V4 feature
  variants on the locked Xtensa, build-std-core, `-j1` path. WN1 remains
  partial because there is still no V4 board owner/`QuietWindow`, control
  runtime wiring, flash quiet boundary, or physical/light-sleep/on-air proof;
  WN2 remains open.
- **2026-08-31, current worktree:** completed the V4 source-only live-owner
  seam. One `V4RadioOwner` supplies a bounded `QuietWindow`/`V4QuietGuard`:
  pre-entry busy refusal is retryable; after standby, cancellation, drop, finish
  failure, or an uncertain hardware apply reset rather than falsely resume.
  The guard clears chip IRQ flags and proves DIO1 low through `lora-phy`, lends
  the control A/B store and narrow `ConfigApplier`, then restores RX and returns
  `Resumed`. Reset-only preview behavior was rejected because it could not
  establish resumed live service. The source seam is unreachable from V4 boot or
  a control carrier and has no embedded, physical, or on-air receipt, so WN1
  remains Partial and WN2 Open.
- **2026-08-31, current worktree:** implemented the V4 boot-only durable
  recovery seam before SplitHost/RNode/RX/power arm. A one-shot pre-radio
  `V4BootOwner` consumes `HardwareResetToken`, uses `StaticCell` scratch outside
  `main`'s async task stack, and derives stable opaque `NodeId` and separate
  `SemanticTagKey` values from persisted board identity with distinct versioned
  HMAC-SHA256 domains. This is not credential sealing. USB supplies only local
  physical-recovery facts; `Blank` creates nothing and authorizes nobody;
  unavailable settings preserve recovery modem behavior; nonblank boot/runtime
  uncertainty is status-only and cannot enter modem/RNode; and the runtime is
  dropped after boot. RNode stays exclusive after successful or blank boot.
  The host runtime suite passed 26 tests, and locked Xtensa checks passed for
  default, `host-uart-low-power`, and `host-uart-low-power+rf-sleep-proof`.
  This remains source/compile evidence: there is no physical/on-device/reset/
  power-cut/on-air receipt. A live `QuietWindow` is still unreachable from a
  verified carrier. WN1 remains Partial and WN2 Open.
- **2026-08-31, current worktree:** implemented the portable first-owner
  durable-state model, the always-compiled fixed-size public-identity
  validator, and the portable full-state first-write staging/arbitration model.
  `OwnerClaim` has exactly one 64-byte Retinue public identity, public
  configuration, and recovery policy; first-owner role is implicit. It contains
  no secrets, credentials, multiple owners, counters, or transaction artifacts.
  `DurableState::from_owner_claim` requires trusted `BoardRecoveryFacts`,
  produces one zeroed Owner grant through the ordinary constructor, and refuses
  incompatible recovery before a persistable state. Six focused commissioning
  tests, `first_write_staging`'s 8 tests, the focused durable-model 4-test
  slice, and identity conformance across 322 full-size comparisons plus
  truncation pass. That portable groundwork was later connected to the V4
  physical USB claim-only path; verifier restore/live carrier dispatch and an
  on-device receipt remain open.
- **2026-08-31:** introduced the V4 staged first-write pair at
  `0x3F6000`/`0x3F7000` and the four-read pre-radio arbitration step before
  SplitHost/RNode/RX/power arm. Valid control beats stale or corrupt pending
  and continues the existing normal boot runtime; blank control plus blank
  pending creates no authority and preserves ordinary modem/RNode
  compatibility. The later physical USB slice can resume or abandon valid
  pending state; corrupt or unsafe evidence remains status-only. The V4 still links only
  `radio-hand` `features = ["radio"]`; there is no V4 Retinue runtime,
  `control-retinue`, allocator, or `-Zbuild-std=alloc`. The three locked V4
  Xtensa core-only variants and a fresh default independent check passed.
- **2026-08-31, documentation reconciliation:** recorded the always-compiled
  identity and portable first-write receipts, plus the initial pre-radio
  arbitration, before the later physical USB claim-only implementation.
- **2026-08-31, physical USB first-write slice:** implemented the bounded
  pre-radio physical-presence USB claim-only path, including full-proof claim,
  stage/readback, resume, and blank-control-only abandon. It replies before a
  terminal resume/abandon reset and leaves blank+blank boards ordinary-
  compatible when no witnessed mutation occurs. WN1 remains Partial and
  WN2-WN8 Open: independent V4 review is GO, and all evidence is source/host/compile
  only.
- **2026-08-31, V4 re-review:** recorded the two P1 fixes: one compile-enforced
  host transport, and a pre-mutation V4 resume feasibility check over decoded
  pending A/B state. This does not add V4 cfg unit-test execution, embedded
  proof, a live carrier, or any of the remaining WN1 gaps.
- **2026-09-01, host first-owner controller and bench client:** implemented the
  carrier-neutral `postilion::control::first_owner` controller and the
  separate `signalman-first-owner` literal USB bench entry. Claim always begins
  with a fresh inspect, signs the exact `OwnerClaim` transcript locally, and
  treats lost terminal replies as recovery-required instead of retrying. The
  USB adapter keeps DTR and RTS deasserted, bounds KISS deframing by
  `INSPECT_RESPONSE_LEN`, and distinguishes timeout, EOF, malformed, and wrong-
  kind replies. Targeted host receipts passed: `cargo test -p postilion
  first_owner -j1` ran 11 tests green, and `cargo check --manifest-path
  apps/signalman/Cargo.toml --bin signalman-first-owner -j1` passed. WN1
  remains Partial: phone UI, physical proof, live carriers, and the native V4
  Reticulum runtime remain open.
