# Bluetooth Capability Scoping Brief

> **Archived 2026-08-30.** The active
> [wall-node management plan](../../2026-08-30_wall_node_management_plan.md)
> replaces LB1 through LB6. This brief remains as stack research and historical
> context; its pre-decision ladder is not active.

Scoping brief, 2026-08-11. **Pre-decision: every ruling here awaits Mark.**
Facts below are verified against the cited sources on this date; items marked
*verify* are not. Context: the 2026-08-11 demo receipts
([Hopspot BLE](../../2026-08-11_hopspot_v4_com7_demo_receipt.md),
[Meshtastic T114](../../2026-08-11_meshtastic_t114_uf2_demo_receipt.md)) proved the
phone apps pair to foreign firmware; Retinue firmware carries zero BLE code.
Mark's directive: BLE must serve **all the channels, all the personalities**,
not just Reticulum.

## Where BLE sits in the doctrine

Per the [listener executive](../../2026-08-10_listener_executive_and_protocol_leases.md),
the executive owns radios and adapters hold bounded surfaces. BLE obeys the
same ownership but different physics: unlike the SX1262, one BLE host stack
serves several GATT services and several peers **concurrently**. The scarce
resource is not the transceiver, it is **advertising** (one legacy advertising
set, 31-byte payload, and phone apps filter scans by advertised service UUID).
So the executive gains an advertising plan where the SX1262 has a scan plan,
and personalities contribute **facets** (GATT services) instead of taking
turns.

Three facet classes cover "all the personalities":

1. **HostLink-over-BLE, one facet that covers every host lane at a stroke.**
   `HostLink` ([radio-hand `link.rs:61`](../../../crates/radio-hand/src/link.rs))
   is a byte pipe plus session, personality-agnostic by design; decision 3
   reserved exactly this slot. A NUS-style GATT pipe implementing it makes
   every host-controlled personality (RNode KISS today, any future host
   protocol) reachable over BLE with no per-personality work. If the pipe
   matches RNode's BLE serial conventions (*verify: NUS UUIDs + pairing
   scheme from upstream RNode firmware*), Sideband-class hosts pair too.
2. **RNS peer interface: the Prns Bluetooth Auto profile.** The node
   channel's BLE lane speaks the donor's GATT profile, so Retichat and every
   Bluetooth Auto peer dials Retinue boards directly. This is a port from the
   endorsed donor, entered in the
   [Prns donor ledger](../../2026-08-10_prns_donor_ledger.md) per the work-lanes
   shared source lock.
3. **Foreign-app facets, behind their channels' own gates.** sennet's phone
   surface is the Meshtastic client API GATT service; tucket's is the
   MeshCore companion protocol (*verify both UUID sets and framing against
   upstream source*). Trunk guard applies: these are participation surfaces
   granted by the executive, and they wait for their adapters' gates.

## Donor facts (Prns, pinned checkout)

- **ESP32 (S3/C6):** `trouble-host` 0.6.0 (vendored) over
  `esp_radio::ble::controller::BleConnector`, esp-radio 0.18, esp-hal 1.1,
  esp-rtos 0.3 embassy, coex feature. Peer capacity 4 on S3, 8 on C6,
  compile-asserted against the slot pool. Sources:
  `personal-hopspot/embedded/esp32/src/bluetooth_auto.rs`,
  `esp32/Cargo.toml`.
- **nRF52840 (T-Echo):** `nrf-softdevice` (embassy-rs, git pin) with S140,
  features central + peripheral + GATT server/client + L2CAP. App linked at
  `0x27000`, RAM from `0x2000E000` (SoftDevice-resident layout). Source:
  `personal-hopspot/embedded/nrf52840/Cargo.toml`, `memory.x`.
- **The profile:** one service with `control` + `data` characteristics,
  `columba_{rx,tx,identity}` for identity exchange, and an L2CAP PSM lane.
  UUIDs derive from a 128-bit base with per-characteristic last bytes
  (`prns-interfaces/impls/embassy/src/bluetooth_auto/trouble/mod.rs:53`).
  Port the base from source; do not transcribe it here.
- The desktop side has a WinRT Bluetooth Auto backend with a manual hardware
  gate (`validation/platforms/windows-ble-hardware.md`), useful later for a
  laptop-as-peer receipt without phones.

## Retinue ground facts

- **T114 (`t114-phy`):** embassy-nrf 0.11, embassy-executor 0.10. `memory.x`
  already links the app at `0x26000..0xEA000` with RAM from `0x20006000`,
  because the stock bootloader's S140 layout demands it. The 2026-08-11 boot
  drive receipt confirms **S140 6.1.1 is resident on the board**. Adopting
  `nrf-softdevice` therefore changes no flash layout; the blob our layout
  already tiptoes around finally gets used.
- **V4 (`heltec-v4-phy`):** esp-hal 1.1.1, esp-rtos 0.3, embassy-executor
  0.10, the same line the donor builds against. Today the image links no
  esp-radio and no esp-alloc; BLE introduces both (first use of the S3's
  radio coprocessor; the SX1262 is SPI and unaffected).
- `radio-hand` exists with the `HostLink` trait; dispatch is shared per
  decision 3.

## The stack decision (the one real fork)

**nRF52840:** follow the donor onto `nrf-softdevice`. The SoftDevice is
already on every board, the layout already reserves it, and the donor proves
the crate against this exact hardware class. The alternative
(trouble + nrf-sdc, pure Rust end to end) abandons the resident SoftDevice,
changes the flash layout, and diverges from the donor; it stays on the table
as a later migration, not the opening move. Licensing note for review: the
SoftDevice blob is not linked into the image (separate flash region, SVC
calls), the same aggregation posture GPL-licensed Meshtastic ships with;
`nrf-softdevice` bindings are MIT/Apache. *Verify: nrf-softdevice crate
against S140 6.1.1 specifically (the crate's s140 feature tracks 7.x; the
donor's T-Echo runs the same 6.1.1-era layout, so check what they pin).*

**ESP32-S3:** trouble over esp-radio is the only serious lane and the donor
already runs it on this exact chip. Cost: esp-alloc heap and esp-radio enter
the image; measure RAM against current headroom.

Divergent stacks per board are acceptable because the seam above them is
shared: facets are defined against GATT/L2CAP shapes, not against a stack.

## Proposed ladder

LB numbering, clear of N/CM/FT/FS/H/LE. Done conditions, no durations.
Ordering follows consumer pull: the byte pipe has consumers today (signalman,
RNode hosts); the peer profile needs the node channel; foreign facets need
their gates.

**LB1: SoftDevice up on the T114.** S140 enabled under embassy without
regressing the radio: the existing direct-PHY acceptance suite passes with
the SoftDevice running (counted blocks, control image per the RF receipt
rule). *This is the keystone risk: SoftDevice interrupt priority versus
`lora-phy` SPI timing. Meshtastic ships BLE + SX1262 on this exact board, so
the class is feasible; our timing is our own to prove.*

**LB2: HostLink-over-BLE byte pipe.** A GATT pipe implements `HostLink`
(attached = subscribed, detached = disconnect/unsubscribe); the shared
dispatch serves a host over BLE on the T114 with no personality code
changed. Receipt: existing host-session acceptance over BLE instead of USB.

**LB3: V4 bring-up.** esp-radio + trouble on the S3; the same byte pipe
facet; the same receipt shape. RAM high-water recorded.

**LB4: Bluetooth Auto peer facet.** Port the donor profile (ledger entries
per seam); Retinue's node channel exposes it as an RNS interface. Receipt:
Retichat pairs to a Retinue board and traffic flows both ways. *Verify
first: Retichat's bonding expectations, and whether it requires the L2CAP
lane or serves GATT-only peers.*

**LB5: Advertising plan under the executive.** Facet registry + advertising
policy (which facets advertise, rotation, payload budget), the BLE sibling
of the LE scan plan. Constraint to verify: extended/multiple advertising
sets under S140 6.1.1 versus one legacy set (rotation as fallback).

**LB6: Foreign-app facets.** sennet's Meshtastic client API service and
tucket's MeshCore companion service, each behind its channel's own gates,
each granted as a participation level. Explicitly out of scope until then.

## Open questions

- SoftDevice RAM appetite under our config versus the `0x20006000` origin
  and 232K budget; measure at LB1, adjust `memory.x` RAM origin if needed.
- Bond store: one bonding database shared across facets, and what deleting a
  facet does to existing phone bonds.
- V4 light-sleep versus advertising (the low-power personality's hand-on
  posture conflicts with a persistent advertiser, same tension the LE doc
  names for the scan plan).
- `columba` semantics (donor's identity exchange): port as-is for
  compatibility or map onto personae identities; interacts with the
  murmuration doc's persona-derivation question.
- Whether HostLink-over-BLE should also match RNode's BLE serial conventions
  for Sideband compatibility, or stay a Retinue-host-only pipe at first.
