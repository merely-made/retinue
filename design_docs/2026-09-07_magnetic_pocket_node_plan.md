# Magnetic pocket node implementation plan

**Date:** 2026-09-07
**Status (2026-09-07):** planned. Product and ownership decisions are settled
below; PN0 through PN8 are open. There is no NFC carrier, pocket-node firmware
target, magnetic charging receipt, phone integration, or qualified enclosure in
the current tree.

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

The proposed `DockFrameV1` is allocation-free and fits in one mailbox exchange:

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

It multiplexes five bounded streams:

- `Control`: the existing signed `RHC0` request and response bytes;
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

The executive exposes these power states:

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
| Dynamic-tag peripheral and board power events | first T114 proof behind a narrow adapter, then a new production firmware target | excludes Signalman policy and RNS routing internals |
| Qi receiver, battery, temperature, and charge-state facts | production board target under the unique `radio-hand` owner | reuses the existing interruption and flash authority |
| Retinue and Outrider queue integration | their existing bounded node/message seams, changed only when a concrete missing API is proved | remains independent of NFC, GATT, and charging policy |
| Signalman synchronization model | `apps/signalman/src/dock.rs` and focused fixtures | remains independent of Core NFC, Android reader, and BLE platform code |
| Mobile platform adapter | a disposable platform probe first; final host location ruled from the actual Mere/Signalman mobile consumer | excludes message-log, controller-authority, and queue-truth ownership |
| Bluetooth adapter | board-specific BLE stack below the common dock model; reuse WN5 and the archived donor research | carries the common contract rather than BLE-only semantics |
| Enclosure, antenna and charging evidence | versioned CAD/BOM inputs plus `validation/results/` raw captures and a dated receipt | measurements do not rewrite protocol authority |

## Phases and done-conditions

### PN0. Freeze claims, controls, and test matrix

Choose the main-MCU plus optional-BLE-companion architecture, target endurance
range, supported regional antenna variants, cell safety envelope, charge input
class, and maximum attached dimensions. Record representative and held-out
phones, cases, chargers, orientations, and the centered-antenna control. Fix
acceptance metrics before selecting the winning coupon.

**Done when:** both product profiles have distinct capability manifests; every
claimed operating state maps to a measurement; the power budget names RX, TX,
idle, NFC, BLE, charge, converter, and sleep loads; and the RF/thermal test
matrix can reject either variant without changing its thresholds after seeing
results.

### PN1. Freeze and model the dock contract

Implement and fuzz the bounded dock framing and cursor reducer without phone or
radio hardware. Version the NFC management carrier honestly. Use literal
mailbox-sized fixtures shared by firmware and Signalman.

**Done when:** interrupted reads and writes at every byte/chunk boundary resume
without duplicating a durable intent; stale boot tokens, retired cursors,
overwritten records, queue capacity, unknown stream kinds, wrong nodes, replayed
control, and protocol-version skew have explicit results; observation reads
perform no durable write; and all prior `RHC0` vectors either remain byte exact
or have a recorded version transition.

### PN2. Prove NFC on representative phones

Connect a protected dynamic-tag breakout to the current T114 native-node proof
platform. Build the smallest iOS Core NFC and Android reader adapters around the
same Signalman model. Exercise NDEF launch separately from active mailbox
synchronization.

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

**Done when:** the selected NFC model beats the centered control across the
declared attached-use matrix; its band tune, link-budget penalty, packet
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
and USB-C recovery remains usable.

### PN5. Close autonomous pocket-node operation

Integrate the bounded Retinue node, durable queues, dock drains, observations,
power states, and the unique radio owner in the production-shaped target. Use
USB only for fixture control and final evidence collection.

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

- **2026-09-07:** founded the plan from the owner's decision to pursue an
  autonomous magnetic pocket node in NFC-only and Bluetooth variants, accept
  receiver-only node charging as the trunk, and keep phone-charging passthrough
  conditional. Inspected the live Retinue ownership, control, observation,
  native-node, listener, Signalman, and archived Bluetooth work before fixing
  the boundaries above.
