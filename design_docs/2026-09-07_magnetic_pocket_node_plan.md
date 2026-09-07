# Magnetic pocket node implementation plan

**Date:** 2026-09-07
**Status (2026-09-07):** active preparation. Product profiles are ruled below;
PN0 through PN8 remain open. The stack review fixes the contract owners and the
first phone probe targets the owner's M4 and iPhone. Unsigned device and
simulator builds pass; simulator launch and local receipt retention are checked.
Physical installation awaits an NFC-enabled provisioning profile. A platform
scan probe is not an authenticated dock, pocket-node firmware, magnetic charging
receipt, or qualified enclosure.

## Purpose and relationship to existing plans

Build a battery-powered Retinue node that attaches at the magnetic charging
position on a phone but remains a complete network participant when the phone is
absent. The phone is a temporary Signalman control and synchronization surface.
Its NFC or Bluetooth session is never part of the node's routing liveness.

This is separate from the
[wall-node plan](2026-08-30_wall_node_management_plan.md), whose first appliance
is a wall-powered V4 with LoRa and WiFi. It consumes that plan's signed `RHC0`
control, durable configuration, recovery, and carrier-independence decisions.
It consumes the [Retinue Small plan](2026-07-31_retinue_small_plan.md) for a
bounded native node, the
[listener-executive plan](2026-08-10_listener_executive_and_protocol_leases.md)
for radio ownership, and the
[observation plan](2026-09-06_radio_observation_plan.md) for truthful gaps and
collection. It does not reorder their active implementation slices.

The [compact signed feed and local control plan](2026-08-25_compact_signed_feed_and_local_control_plan.md)
owns LC0's local command/property/stream decision and AT0's bounded secure
attachment proof. PN1 reconciles with those families before freezing another
wire grammar. The [hardware-family plan](2026-09-07_radio_hand_hardware_family_plan.md)
owns shared board capability and power boundaries; PN retains pocket-specific
geometry and acceptance. Mere's existing integration is at
`mere/ports/signalman`, with Personae/Castellan credentials and host persistence;
it is not a dependency of firmware.

"Magnetic" describes the working attachment and charging geometry. A shipped
product may be called MagSafe or Qi2 only when the applicable Apple and Wireless
Power Consortium requirements, licenses, and qualification have been met.

## Verdict

Develop two variants from one node and dock contract:

| Working profile | Local carriers | Physical intent | Product promise |
| --- | --- | --- | --- |
| **NFC model** | dynamic NFC plus USB-C recovery | thinnest acceptable phone spacing; receiver-only magnetic charging | Bluetooth is absent as a usable capability. The node routes, receives, transmits, queues, and records while detached from Signalman. |
| **Bluetooth model** | the same NFC and USB-C paths plus optional BLE | more antenna clearance and a modestly larger battery envelope if measurements justify it | BLE makes long synchronization and live inspection easier. It does not keep Retinue alive and may be disabled without reducing network capability. |

Receiver-only charging is the trunk: a compatible charger replenishes the
node. When the node sits between the phone and a charger, the phone is not
promised charging power. A Qi transmitter normally serves one receiver, and the
node is that receiver in this mode.

Charging the phone through the node is a conditional feasibility gate. It
requires a receiver on the charger-facing side, a separate transmitter on the
phone-facing side, managed power conversion, foreign-object handling, and
thermal control. It is not a direction bit on an ordinary Qi receiver. If that
bridge cannot pass PN7, both variants ship receiver-only charging.

The Bluetooth model's extra volume is assigned in this order:

1. phone-to-antenna separation and a sub-GHz antenna keepout;
2. NFC, charging-coil, ferrite, and 2.4 GHz coexistence spacing;
3. battery capacity that fits without entering either antenna keepout.

A larger metal-pouch battery is not an RF remedy. The Bluetooth model may claim
better attached reception only when the selected geometry proves it.

## Product invariants

1. `radio-hand` remains the resident board and radio executive. Retinue owns
   RNS identity, routing, links, announces, and protocol obligations under it.
2. Signalman owns operator intent, synchronization, durable host capture, and
   truthful delivery wording. It does not become the node's durable authority.
3. Detaching the phone, ending a Core NFC session, disabling Bluetooth, or
   killing Signalman does not stop ordinary node work.
4. Every dock transfer is bounded, cursor-based, idempotent, and resumable after
   loss at any byte boundary.
5. NFC proximity is presence evidence, not controller authorization or
   confidentiality. Existing signed controller grants and replay protection
   remain authoritative.
6. Routine status, observation, and inbox reads cause zero flash writes and zero
   radio quiet windows. Durable mutations continue through the one board-owned
   quiet-window path.
7. Charging impairment is a reported node state. Signalman never presents a
   charging interval as ordinary full-quality listening unless measurements
   support that claim.
8. USB-C remains available for recovery, firmware installation, and diagnostics
   on both variants.
9. Device routing authority, controller authority, and message-sender authority
   have separate lifetimes. A dock session ending clears its transient access;
   it does not clear the device's durable participation policy. Delegated sending
   authority may expire independently, with an explicit refusal and receipt.

## Physical and RF architecture

The magnet ring, battery pouch, charging coil, ferrite, phone chassis, and many
cases are electrically significant structures. The sub-GHz antenna must not be
placed inside that central stack. The opening geometry puts it in an
RF-transparent top or side spine, preferably extending beyond the phone's metal
silhouette. Candidate structures include a tuned flex antenna in the spine and
a short deployable antenna. A centered PCB trace is only a negative-control
layout.

The Bluetooth model uses its added thickness to lift that spine away from the
phone. An RF switch or second sub-GHz antenna is admitted only after a one-
antenna coupon establishes a remaining orientation-specific null and the second
path improves held-out measurements. Feature count is not evidence of diversity.

The NFC antenna and Qi receiver coil are separate tuned structures. The NFC
loop sits where representative phones can couple to it while magnetically
attached; it is tuned in the complete stack, including magnets, ferrite,
battery, enclosure, phone, and case. The design includes the NFC-tag protection
network required for exposure to a wireless-power field from the first coupon,
because adding it later changes the antenna tune.

The Bluetooth antenna, when fitted, has its own keepout and supply filtering.
Coexistence work measures conducted and radiated effects from BLE advertising,
BLE transfer, the charging converter, NFC polling, and LoRa RX/TX. The frequency
separation between BLE and LoRa does not waive co-site, harmonic, ground-current,
or power-noise testing.

## Dock protocol

An NFC NDEF URI is the wake and launch affordance. Meaningful synchronization
uses an active tag-reader session against an MCU-connected dynamic tag such as
the ST25DV-I2C family. Its 256-byte Fast Transfer Mode mailbox is a transport
constraint, not an application message size.

`DockFrameV1` is an illustrative candidate, not a second frozen local-control
protocol. LC0/AT0 and PN1 must assign four distinct responsibilities: mailbox
fragmentation, secure-session records, application stream framing, and signed
mutations. The bounded transport fragment fits in one mailbox exchange; its
payload budget includes authentication overhead where that layer requires it.
Handshake fragmentation and data fragmentation have explicit byte/time bounds.

```text
illustrative, not compile-ready:
  magic and version
  node id and fresh boot token
  stream kind
  transfer id
  cursor or chunk ordinal
  flags and bounded payload length
  payload
  integrity check
```

It multiplexes six bounded streams:

- `Control`: existing signed `RHC0` requests and their response bytes, protected
  by the authenticated session; current `RHC0` replies are not device-signed;
- `Snapshots`: bounded read-only status, separate from signed journaled `Status`;
- `Observations`: cursor drains from the `radio-hand` recorder, including gaps;
- `Inbox`: received application records available to this controller;
- `Outbox`: durable outgoing intents accepted by the node;
- `Receipts`: node acceptance, handed-to-radio, link, propagation, delivery, or
  terminal refusal facts when actually observed.

A new session begins with capabilities, boot token, current generations, queue
watermarks, and per-stream cursors. Signalman supplies its last admitted
cursors. Duplicate chunks return the same result. A changed boot token and an
overwritten cursor produce explicit restart and gap facts. Signalman may say
`saved on phone`, `accepted by node`, `handed to radio`, or a stronger observed
state; it must not collapse those into `sent`.

### Read-only access and authenticated results

The session proves the device key and an authorized controller key, binds their
proofs to the fresh transcript, and sets per-stream read/write permissions.
Inbox content and BLE connection material require confidentiality. A boot token,
tag UID, checksum, or caller-supplied node id alone is not device authentication.
AT0 supplies the host/device transcript and measured embedded resource ceiling;
Mere's Tokio Noise adapter is a host reference, not firmware-ready code.
Notochord's carrier-observed identity and secure-channel proof remain distinct.

Current `ControlRuntime::observe_status` journals every accepted signed outer
counter through a quiet window. Routine reads therefore use a separate bounded
snapshot and observation path with session freshness and volatile sequencing.
They neither consume that durable command counter nor mutate a drain cursor on
the board. Owner revocation is checked when admitting access and at bounded
session checkpoints. Reconnect and reboot require a new authenticated session.
Authority-sensitive signed `Status` remains explicitly journaled.

### Durable acceptance and the two stores

Signalman's existing `MessageId`, `MessageEvent`, and `MessageBook` remain the
host message vocabulary. The host persists an outgoing intent before offering
it to a node, and persists the admitted node result before advancing its local
cursor. The phone store and the board custody store have separate recovery
proofs; a host persistence receipt cannot establish board durability.

An outgoing intent has an immutable id independent of dock transfer id, session,
carrier, and boot. The board atomically persists the accepted intent and its
duplicate-suppression/result state before returning `accepted by node`. Retrying
that intent with the same body after a lost reply or reboot cannot enqueue it
again; the same id with different content is refused. PN1 fixes retention,
expiry, capacity refusal, and the retired-id boundary so eviction never silently
restores permission to execute an old intent. This is local acceptance, not an
exactly-once guarantee for RF transmission or remote delivery.

Inbox reads are nondestructive. Acknowledgement, deletion, cancellation, and
ownership transfer are separate authorized mutations, with controller scope
and power-cut behavior specified. Observation RAM loss continues to produce
gaps; it does not inherit the durable message queue's guarantees. Firmware
storage remains board-owned and separate from the control/settings journal.

`ManagementCarrier` currently assigns tags only to USB, BLE, IP, and Reticulum.
PN1 adds NFC through an explicit compatible protocol-version change and updates
all known-bit masks, durable decoders, literal vectors, capability limits, and
recovery rules together. The dock envelope does not smuggle NFC in as USB or
BLE.

## Bluetooth model

BLE carries the same dock streams through a bounded GATT adapter. ATT
fragmentation, connection state, bonding, and platform discovery remain inside
that adapter. BLE link security may add privacy and confidentiality, but signed
Retinue control remains mandatory.

The owner-visible Bluetooth setting has three modes:

- `Off`: controller and advertising disabled;
- `Tap to open`: an authenticated NFC interaction opens a bounded BLE window
  for faster synchronization;
- `Nearby`: reconnectable advertising for owners who prefer convenience and
  accept its power and tracking costs.

`Tap to open` is the default candidate. Advertising exposes neither a stable
Retinue identity nor a stable node id. The phone receives the ephemeral
connection material over the NFC session. Permission denial, bond deletion,
Bluetooth-off, timeout, and disconnect are ordinary resumable states.

The NFC model must support an honest product claim. A prototype may reuse the
T114/nRF52840 path while leaving its 2.4 GHz stack disabled, but a production
unit called "no Bluetooth" uses a main MCU without 2.4 GHz, or a common main
MCU with an optional BLE companion fitted only to the Bluetooth model. A
firmware setting on fully fitted Bluetooth hardware is described as
Bluetooth-disabled, not Bluetooth-free. The T114 is therefore a protocol and
power prototype rather than the production proof of this claim.

## Charging and power states

The trunk contains a Qi receiver, ferrite and alignment structure, protected
single-cell charging and power-path management, fuel measurement, an NTC at the
cell, and temperature sensing near the receiver coil. Component selection stays
open until PN0 fixes input power, charge-current, battery, coil, and enclosure
limits. An integrated receiver/charger such as TI's active BQ51050B class proves
that the receiver-only topology is conventional; it is not selected here as the
production part or as a Qi2 claim.

The executive exposes independent power facts, not one mutually exclusive enum.
Charge target (`none`, `node`, PN7-only `phone`), thermal restriction, reserve
status, dock pause, and measured radio impairment may coexist. Signalman derives
these labels while retaining every active fact:

| State | Required behavior |
| --- | --- |
| `Field` | measured ordinary listening and configured transmit policy |
| `Charging node` | node is the wireless-power receiver; observe converter noise, coil and battery temperature, charge current, and any radio derating |
| `Charging phone` | PN7-only bridge state; preserve a configured node reserve and report the stronger thermal/RF impairment |
| `Thermal hold` | stop or reduce charging at hardware-safe thresholds; preserve only radio work proven safe at that temperature |
| `Battery reserve` | shed optional display, BLE, and high-cost work according to owner settings while retaining the declared emergency radio floor |

Charge reserve, Bluetooth mode, ordinary charging current below the hardware
ceiling, and optional-work shedding are settings. Cell safety limits,
regulatory transmit limits, and component absolute limits are enforced floors
or ceilings rather than owner overrides.

NFC and Qi operation in the same stack is not assumed concurrent. PN4 either
proves the intended NFC mailbox exchange while Qi is active or implements a
deterministic, bounded charge pause for the dock session. A pause must not erase
the fact that the device was charging or bypass thermal and foreign-object
protection.

PN4 must name and prove a pause trigger that works before a successful mailbox
request: for example protected field detection or a local physical action. A
request over the impaired NFC channel alone cannot be that trigger. Fix the
maximum pause, automatic resume, hysteresis, and repeated-trigger budget before
the coexistence test. Hardware protection overrides charging requests; thermal
and reserve restrictions jointly clamp the radio policy rather than overwriting
one another. The observation record retains charging exposure during a pause.

## Security and recovery

The NDEF launcher contains public routing information and a fresh challenge,
never a durable secret. Initial ownership uses the existing proof-bound claim
shape plus one of two explicit roots: a per-unit one-time claim secret delivered
outside the NFC field, or USB physical recovery. Merely placing an unclaimed
phone against the node is insufficient.

Secret-bearing settings use a confidential carrier or a payload sealed to the
node and signed transaction. Short range does not make plaintext credentials
safe. The node retains at least one owner-approved recovery path through every
configuration transition. NFC may count as local physical presence only after
its exact protection, challenge, and enclosure assumptions are recorded;
receiver-only charging by itself proves nothing about who is present.

## Ownership and proposed files

| Concern | Owner and likely path | Stop line |
| --- | --- | --- |
| Dock framing, cursors, stream limits, and NFC carrier tag | `crates/radio-hand/src/dock.rs` and `control/**` | allocation-free semantics; excludes I2C peripherals, phone APIs, and durable host storage |
| Host dock exchange and retry orchestration | `crates/postilion/src/control/**` plus a bounded dock adapter | consumes LC0/AT0 and the common model; excludes UI and credential custody |
| Dynamic-tag peripheral and board power events | first T114 proof behind a narrow adapter, then a new production firmware target | excludes Signalman policy and RNS routing internals |
| Qi receiver, battery, temperature, and charge-state facts | production board target under the unique `radio-hand` owner | reuses the existing interruption and flash authority |
| Embedded message delivery and durable acceptance | Retinue/Outrider portable seams plus a board custody store | Outrider's host delivery/propagation modules are not an embedded implementation; no NFC, GATT, or host database dependency |
| Signalman synchronization model | `apps/signalman/src/dock.rs` and focused fixtures | remains independent of Core NFC, Android reader, and BLE platform code |
| Controller credentials and commissioning integration | `mere/ports/signalman`, Personae and Castellan | firmware receives scoped proofs and provisioned material, never the vault or full Notochord evaluator |
| Host message persistence | existing Signalman message model with a host adapter; desktop precedent in `apps/signalman-desktop/src/messages.rs` | commit-before-expose behavior is reusable; board custody needs its own backend and receipt |
| Mobile platform adapter | a disposable platform probe first; final host location ruled from the actual Mere/Signalman mobile consumer | excludes message-log, controller-authority, and queue-truth ownership |
| Presentation and lifecycle | Mere's Cambium/rootstock with a native mobile event and NFC adapter; Genet engine contracts below | Cambium moved to Mere on 2026-09-03; desktop compilation does not establish mobile readiness |
| Bluetooth adapter | board-specific BLE stack below the common dock model; reuse WN5 and the archived donor research | carries the common contract rather than BLE-only semantics |
| Enclosure, antenna and charging evidence | versioned CAD/BOM inputs plus `validation/results/` raw captures and a dated receipt | measurements do not rewrite protocol authority |

## Phases and done-conditions

### PN0. Freeze claims, controls, and test matrix

Choose the main-MCU plus optional-BLE-companion architecture, target endurance
range, supported regional antenna variants, cell safety envelope, charge input
class, and maximum attached dimensions. Record representative and held-out
phones, cases, chargers, orientations, and the centered-antenna control. Fix
acceptance metrics before selecting the winning coupon.
Include an absolute packet-capture floor, maximum attached link-budget penalty,
permitted orientation coverage, useful dock payload/throughput, and endurance
under a named workload; beating the centered negative control is insufficient.

**Done when:** both product profiles have distinct capability manifests; every
claimed operating state maps to a measurement; the power budget names RX, TX,
idle, NFC, BLE, charge, converter, and sleep loads; and the RF/thermal test
matrix can reject either variant without changing its thresholds after seeing
results.

### PN1. Freeze and model the dock contract

Implement and fuzz the bounded dock framing and cursor reducer without phone or
radio hardware. First reconcile LC0 framing and AT0 secure attachment, freeze
identity roles, and specify phone-store versus board-store transactions.
Version the NFC management carrier honestly. Use literal mailbox-sized fixtures
shared by firmware and Signalman; include handshake and authentication overhead.

**Done when:** interrupted reads and writes at every byte/chunk boundary resume
without duplicating a durable intent; stale boot tokens, retired cursors,
overwritten records, queue capacity, unknown stream kinds, wrong nodes, replayed
control, and protocol-version skew have explicit results; routine status, inbox,
and observation reads (including authentication and retries) perform no durable
write or quiet window; wrong-device/replayed-session proofs fail; per-stream
permissions and confidential content are enforced; and all prior `RHC0` vectors
either remain byte exact or have a recorded version transition. Inject power
loss before/after intent and dedup persistence, after acceptance but before its
reply, and during host result persistence. Reboot plus retry must preserve the
declared custody result without duplicating a durable intent.

### PN2. Prove NFC on representative phones

Connect a protected dynamic-tag breakout to the current T114 native-node proof
platform. Build the smallest iOS Core NFC and Android reader adapters around the
same Signalman model. Exercise NDEF launch separately from active mailbox
synchronization.

The first platform-only slice uses the owner's M4 and paired iPhone 14 Pro Max.
Keep it in [the disposable NFC probe](../apps/pocket-nfc-probe/README.md), outside
the Rust dependency graph. It establishes signing, install, launch, foreground
NDEF reading, ISO15693 tag detection, cancellation, and bounded local evidence.
It neither freezes `DockFrameV1` nor claims authentication or dynamic mailbox
exchange. Core NFC launch requires a supported universal link and user action;
the probe initially uses an explicit in-app scan, and the universal-link/domain
association is a separate unproved leg. A manual app opening is not NDEF launch.

Before full enclosure optimization, position the protected dynamic tag in a
representative magnetic stack and demonstrate sustained mailbox exchange at the
intended attachment location, without shifting the accessory to a separate tap
position. Record case, orientation, lock/unlock, already-attached app launch,
session timeout/cancel, and unavailable reader states. A loose-tag tap is only a
platform receipt. Android and held-out devices remain required for PN2 closure.

**Done when:** at least one current iPhone and one current Android phone can
launch, authenticate, exchange every dock stream, lose the session, and resume
from cursors; a repeated attach/detach block records success rate, useful
throughput, session duration, case, orientation, battery cost, and every failure
class; and removing the phone throughout the block leaves the node's radio loop
and queues active.

### PN3. Select the RF and mechanical stack

Build instrumentable coupons varying antenna spine, phone separation, magnet
and keeper construction, NFC loop, charging coil/ferrite, battery position, and
enclosure. Measure antenna tune and efficiency where available, then run counted
on-air RX/TX blocks detached, attached, cased, pocketed, and oriented against
the centered control. Repeat the selected geometry on held-out phones and cases.

**Done when:** the selected NFC model meets PN0's absolute usability floors and
beats the centered control across the declared attached-use matrix; its band
tune, link-budget penalty, packet
capture, current, and orientation nulls are recorded; the Bluetooth model's
extra geometry demonstrates a repeatable improvement before receiving an RF
premium claim; and neither battery variant enters the selected antenna or NFC
keepout.

### PN4. Qualify receiver-only charging

Integrate the Qi receiver, power path, protected NFC loop, cell and sensors.
Exercise detached charging and node-between-phone-and-charger charging against
the charger/phone/case matrix. Run full low-to-full and maintenance cycles,
misalignment, foreign-object, thermal, NFC-active, LoRa RX/TX, brownout, and
power-removal cases.

**Done when:** the node charges safely from every declared charger; unsupported
stacks refuse cleanly; Signalman and local indication distinguish node charging
from phone charging; the node maintains its declared reduced or ordinary radio
profile with measured loss; NFC either works at the admitted rate or invokes
the bounded charge-pause policy; charge and temperature limits survive reset;
the pause starts while NFC is impaired and resumes within its declared bound;
combined thermal/reserve states preserve the stricter limits; and USB-C recovery
remains usable.

### PN5. Close autonomous pocket-node operation

Integrate the bounded Retinue node, durable queues, dock drains, observations,
power states, and the unique radio owner in the production-shaped target. Use
USB only for fixture control and final evidence collection.
Explicitly implement and measure the portable delivery subset needed for the
declared message workload; the existing Outrider `no_std` codec/stamp subset
does not establish an autonomous delivery or propagation worker.

**Done when:** after claim and configuration, the node boots and participates
with every phone and host absent; receives and accepts bounded outbound work
during later docks; exposes truthful queue overflow and observation gaps;
survives phone-session churn, battery reserve, charge entry/exit, reset, and
power removal; and completes a provenance-bound on-air routing/message receipt
after Signalman has disconnected.

### PN6. Add the Bluetooth model

Fit the selected BLE path and carry `DockFrameV1` through one GATT service.
Implement `Off`, `Tap to open`, and `Nearby` without changing node authority.
Measure the real battery and coexistence cost before selecting the larger cell.

**Done when:** NFC can open a bounded private BLE window; the full dock corpus
replays over NFC and BLE to the same Signalman state; stable identity is absent
from advertisements; permission, bond, timeout, restart, and disconnect states
resume correctly; disabling BLE leaves all Retinue behavior intact; and counted
LoRa RX/TX plus power measurements cover advertising, transfer, charging, and
idle. The larger battery is admitted only if it meets PN0 endurance without
weakening PN3's antenna result.

### PN7. Decide the phone-charging bridge

Build this as an isolated electrical/mechanical coupon after PN4, not into the
trunk PCB. Compare receiver-only operation with a two-coil receiver-to-
transmitter bridge. The candidate policy charges the node to an owner-set
reserve before offering phone power, stops on thermal or foreign-object faults,
and keeps the user-visible charge target explicit.

**Done when:** efficiency, delivered phone power, node reserve behavior, coil
and battery temperatures, charger negotiation, phone cases, NFC, BLE, LoRa
impairment, thickness, and fault handling all meet the predeclared PN0 limits.
If any safety or product limit fails, record receiver-only as the final
architecture and remove the bridge from the product BOM rather than retaining
an ambiguous dormant circuit.

### PN8. Qualify the two products

Freeze production boards, antennas, cells, firmware manifests, enclosures,
recovery instructions, and supported charging claims. Complete applicable
radio, battery, Qi/Qi2, Apple accessory, transport, and regional compliance work
against exact production samples.

**Done when:** the NFC model proves its Bluetooth capability claim; the
Bluetooth model proves its optional-carrier and attached-RF claims; advertised
runtime and charging behavior are reproduced from measured profiles; every
degraded state appears in Signalman and the observation record; firmware can be
recovered over USB-C; and the release receipt binds hardware revision, BOM,
antenna, cell, enclosure, firmware hash, region, phone/case/charger matrix, and
raw RF/power/thermal evidence.

## Findings

- **2026-09-07, stack review:** `postilion/src/control/verified.rs` documents
  unsigned board replies; `radio-hand/src/control/runtime.rs::observe_status`
  persists replay counters. PN1 needs authenticated replies and a separate
  write-free snapshot path, not a renamed existing `Status` operation.
- **2026-09-07, stack review:** `apps/signalman/src/message.rs` already owns
  message ids/events/replay. The desktop `messages.rs::MessageStore::append`
  persists before exposure. `outrider/src/lib.rs` gates delivery and propagation
  on `std`; board custody and the portable worker remain new implementation.
- **2026-09-07, stack review:** `mere/ports/signalman` bridges credentials;
  its expiring sited-station runtime is not the pocket device's routing lifetime.
  `mere/crates/murm/transport/src/noise.rs` has a host transcript but uses Tokio
  and a 65,535-byte scratch buffer. LC0/AT0 remain the extraction/proof owners.
- **2026-09-07, M4 preflight:** macOS 26.5.1, Xcode 26.6, a valid Apple
  Development signing identity, and a paired iPhone 14 Pro Max on iOS 26.6.1
  with Developer Mode enabled were observed over SSH. Device identifiers and
  signing material remain outside committed evidence. This proves access only.
- **2026-09-07, probe validation:** `apps/pocket-nfc-probe` builds for arm64
  iOS and the iOS simulator with Xcode 26.6 / iPhoneOS 26.5 SDK. Simulator
  launch, visible unavailable-reader state, atomic JSON receipt creation and
  reopening are checked; the receipt explicitly identifies simulator execution.
  Physical signing fails because Xcode has no configured account and its
  existing wildcard profile lacks NFC Tag Reading. Sign in through Xcode's
  Accounts settings and issue the probe-specific NFC profile before installing.
  NFC hardware availability is awaiting the owner; no tag or mailbox was read.
  Source hashes, build logs and simulator evidence are retained locally under
  `validation/results/pocket-nfc-probe-20260907/`, not as physical PN2 evidence.

- **2026-09-07, repository:** `radio-hand::control::ManagementCarrier` currently
  contains USB, BLE, IP, and Reticulum only. NFC therefore requires a real
  contract/version update rather than an adapter alias.
- **2026-09-07, repository:** O1/O2 provide the allocation-free observation
  codec, RAM recorder, gaps, and Signalman replay model. Firmware emission,
  collection carriers, and durable host storage remain open and must not be
  described as pocket-node capability.
- **2026-09-07, repository:** the current native-node evidence is T114-shaped;
  the complete resident scheduler has a radio-free foundation but no firmware
  consumer. This plan may prototype there but cannot claim a finished portable
  appliance from it.
- **2026-09-07, NFC:** ST documents dynamic NFC tags as MCU bridges and the
  ST25DV Fast Transfer Mode as a 256-byte RF/I2C mailbox. Apple documents NDEF
  background launch separately from app-owned reader sessions. The design uses
  launch plus a resumable foreground dock, not continuous background NFC.
- **2026-09-07, charging:** WPC receiver examples state that one transmitter
  serves one receiver at a time. ST separately documents protection for NFC
  tags exposed to wireless-power charging. Those facts make node-only receive
  the trunk and concurrent NFC/Qi a measured gate.
- **2026-09-07, Bluetooth:** Nordic documents the nRF52840 used by the T114
  proof class as containing both a 2.4 GHz transceiver and NFC-A. It is useful
  for prototype reuse, while the production NFC model needs a main MCU without
  2.4 GHz and the Bluetooth model adds a separate fitted companion.

## External references inspected 2026-09-07

- [Apple Core NFC](https://developer.apple.com/documentation/CoreNFC)
- [Apple background NFC tag reading](https://developer.apple.com/documentation/corenfc/adding-support-for-background-tag-reading)
- [Apple accessory design entry point](https://developer.apple.com/accessories/)
- [ST25 dynamic NFC tags and Fast Transfer Mode](https://www.st.com/en/nfc/st25-dynamic-nfc-tags.html)
- [ST AN5364: protecting ST25 tags from wireless power charging](https://www.st.com/resource/en/application_note/an5364-how-to-protect-st25-tags-from-wireless-power-charging-stmicroelectronics.pdf)
- [WPC Qi transmitter reference designs](https://www.wirelesspowerconsortium.com/media/1rof2nis/qi-v13-ptx-ref-designs.pdf)
- [WPC Qi2 Magnetic Power Profile announcement](https://www.wirelesspowerconsortium.com/media/w0ha5cbk/qi2-certification-rolls-out-news-release-11132023.pdf)
- [Nordic nRF52840 product page](https://www.nordicsemi.com/products/nrf52840)
- [TI BQ51050B receiver and battery charger](https://www.ti.com/product/BQ51050B)

## Progress

- **2026-09-07, review follow-through:** reconciled local-control and secure
  attachment ownership, three identity lifetimes, read-only snapshots, durable
  acceptance, independent power facts, absolute RF criteria, and the attached
  NFC gate. The owner authorized the M4 and phone probe. PN0-PN8 remain open.
- **2026-09-07, platform preparation:** added the disposable read-only iPhone
  probe, checked unsigned device/simulator builds and simulator receipt
  persistence. M4 copy is isolated at
  `~/Code/probes/retinue-pocket-nfc-20260907/pocket-nfc-probe`; the M4's existing
  Retinue checkout is untouched. Physical install/read awaits account/profile
  setup and a test tag. PN1 protocol implementation has not started.

- **2026-09-07:** founded the plan from the owner's decision to pursue an
  autonomous magnetic pocket node in NFC-only and Bluetooth variants, accept
  receiver-only node charging as the trunk, and keep phone-charging passthrough
  conditional. Inspected the live Retinue ownership, control, observation,
  native-node, listener, Signalman, and archived Bluetooth work before fixing
  the boundaries above.
