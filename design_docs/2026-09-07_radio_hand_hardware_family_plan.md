# Radio-hand hardware family and autonomous power implementation plan

**Status (2026-09-07):** planned. The family boundary and preferred next
products are ruled below. HW0 through HW8 are open. This document makes no
board-support, unattended-runtime, energy-budget, outdoor, or protocol claim.

## Purpose

Retinue now has two appliance plans with deliberately different physical
assumptions:

- the [wall node](2026-08-30_wall_node_management_plan.md), a powered Heltec V4
  with WiFi, Bluetooth, USB, and later resident Reticulum;
- the [magnetic pocket node](2026-09-07_magnetic_pocket_node_plan.md), an
  autonomous phone-attached node with NFC, receiver-only Qi, USB-C, and an
  optional Bluetooth companion.

The next question is not which development board to port. It is what a piece
of hardware must provide before `radio-hand` can be its resident board and
radio executive, and which products are worth proving against that contract.
This plan owns that family boundary, autonomous power, outdoor powered nodes,
and the admission of new radio and backhaul hardware. The existing WN and PN
plans retain their product-specific gates.

The [listener executive](2026-08-10_listener_executive_and_protocol_leases.md)
continues to own protocol leases. [Mesh scaling](2026-08-09_mesh_scaling_and_asymmetric_routing.md)
continues to own airtime, path bounds, bidirectionality, and forwarder scope.
The [field security posture](2026-08-09_field_node_security_posture.md) owns
seizure and key custody. [Position disclosure](2026-09-01_position_disclosure_plan.md)
owns GNSS disclosure. Linkboy and WN6 retain image trust, selection, and
rollback. This plan consumes those authorities rather than recreating them.
The [IoT device concepts](2026-08-13_iot_device_concepts.md) retain the
application ideas for field beacons, wildlife sensing, telemetry, and e-ink;
this plan supersedes only that note's now-aged hardware sequencing.

## Verdict

Build a logical hardware family, not one universal PCB.

1. Extract a bounded radio-port seam from `radio-hand::Executive` while
   preserving the current SX126x behavior.
2. Add measured power, energy reservation, watchdog, reset-cause, and recovery
   capabilities as small independent seams. Do not collect every peripheral
   into one `Platform` trait.
3. Make the **outdoor powered edge node** the next new product after the wall
   and pocket designs. Its first power inputs are PoE and certified isolated
   low-voltage DC. It mounts the radio near a high external antenna and brings
   power and data up the cable.
4. Make the **solar field relay** the next consumer. It adds MPPT charging,
   LiFePO4, battery-temperature protection, measured harvest and load, and
   explicit energy-degraded roles.
5. Let those two boards reveal whether a shared compute-and-radio module is
   physically useful. Do not standardize a module connector before two real
   layouts expose their shared signals, noise, thermal, and RF constraints.

Raw parking-lot-pole mains is outside the first board. Pole supplies may be
switched, high voltage, electrically harsh, and subject to installation rules.
The supported first deployment is PoE or a listed, isolated external
AC-to-DC supply installed by a qualified person, feeding a protected
extra-low-voltage input. A later mains carrier is a separately certified power
product, not a population option on the radio PCB.

## What `radio-hand` support means today

The crate is portable at its control, settings, scheduler, and policy layers,
but its live radio path is narrower:

- `crates/radio-hand/src/executive.rs::Executive` stores a concrete
  `&mut lora_phy::LoRa<RK, DLY>`;
- `ChipDiagnostics` also takes that concrete `LoRa` type;
- `BoardStore` combines persistence and entropy because the T114 owns them in
  one object;
- the V4 has an additional concrete `V4RadioOwner` and `QuietWindow` path;
- `tulle::PacketRadio` abstracts host-side serial radios and allocates `Vec`;
  it is not the resident firmware radio seam;
- the vendored `lora-phy` has SX126x and SX127x implementations and an STM32WL
  example, but this tree's physical radio-ownership receipts are confined to
  its current T114 and V4 paths and do not establish the same integration
  level on both boards.

Therefore “can run Rust” and “has a LoRa radio” do not yet mean “supports
`radio-hand`.” Support is earned in levels:

| Level | Meaning | Required evidence |
| --- | --- | --- |
| Compile target | A supported `no_std` Rust target can link the selected feature set inside measured flash, RAM, and stack bounds. | Locked build plus map/size receipt; allocator requirement stated. |
| Board owner | Firmware owns monotonic wake time, entropy, durable state, watchdog/reset cause, local recovery, and every hardware resource it advertises. | Driver tests and power-cut/reset receipts. |
| Radio-hand resident | `radio-hand` exclusively owns an admitted radio port, exact profile application, IRQ/busy/reset handling, TX regulation, and return to RX. | Existing software corpus plus two-board on-air receipt. |
| Unattended node | The board reports power truth, survives source transitions and brownouts, retains a recovery path, and applies signed control without a resident host. | Source-loss, power-cut, watchdog, rollback, and sustained-operation receipts. |
| Product-qualified | The exact enclosure, antenna, power carrier, region, temperature range, and enabled bearers have been measured together. | Per-SKU RF, power, thermal, ingress, and recovery report. |

A board name never implies a level. Capabilities are discovered from an
explicit, versioned manifest and limited to the exact hardware and firmware
profile that earned them.

## The platform seams

### Radio port

HW1 introduces an allocation-free `RadioPort` used by the live executive. Its
operations are the ones the executive already needs: apply an exact Selvage
profile, CAD, enter RX, receive with RSSI/SNR, transmit, standby/sleep, clear
and inspect faults, and report measured transition durations. The first
implementation is a thin adapter around the current `lora_phy::LoRa`; this is
an extraction with equivalence receipts, not a new radio stack.

A physical port advertises:

- stable port id and radio kind;
- supported bands, modulations, profile limits, and maximum frame size;
- TX capability and region-specific effective power limits;
- antenna path and **collision-domain id**;
- clock, reset, busy, IRQ, RF-switch, and front-end capabilities;
- whether RX, TX, CAD, sleep, and diagnostics are physically implemented.

Collision domains matter more than radio count. Two transceivers behind one RF
switch, antenna, SPI bus, supply limit, or mutually desensing enclosure are not
two independent radios. The board owner reports those relationships; the
scheduler never infers independence from two port ids.

### Power owner

The physical power owner reports facts. `radio-hand` decides which work those
facts can admit.

`PowerSnapshotV1` should be bounded and explicit about uncertainty:

- source class and external-power presence;
- input, battery, and regulated-rail voltage where measured;
- charge/discharge current and accumulated energy where measured;
- cell and board temperature, charger state, and cold/hot inhibit;
- brownout, watchdog, and source-transition counters;
- estimated state of charge plus its confidence/source, or Unknown;
- enabled power domains and the last shed reason.

`PowerPolicyV1` is owner-configurable. It carries a reserve floor, allowed
roles by energy state, charging-temperature bounds, and which bearers or
radios may be shed. Product defaults are recommendations in the manifest,
not immutable constants.

Work that materially changes energy use requests a bounded `PowerLease` with
an expected cost, deadline, and role. The energy model may refuse or shorten a
lease; only the board power owner may report that a rail actually turned off.
The initial policy has four plain states: **Full**, **Managed**, **Reserve**,
and **Recovery**. State transitions are observable and do not masquerade as
radio faults.

### Durable state and recovery

Flash erase already forces a live quiet window and wears under frequent
counters. New boards may provide an SPI/I2C F-RAM or another high-endurance
nonvolatile store for observations, energy integrals, and replay journals.
That changes the physical write mechanism, not the safety contract: formats
remain versioned, integrity checked, bounded, and power-cut tested. Control
authority continues to use its fail-closed durable semantics.

Every unattended board also provides:

- hardware watchdog and a retained reset cause;
- brownout detection at a measured threshold;
- a local recovery carrier that does not depend on the field network;
- a boot path whose image verifier the application cannot rewrite;
- explicit safe output states during reset and partial power;
- a service mode reachable without transmitting.

### Bearers and gateway facets

Five different things must stay different:

| Kind | Examples | Ownership |
| --- | --- | --- |
| Resident radio adapter | Reticulum, Tucket/MeshCore, Sennet/Meshtastic, later UMSH | Adapter borrows admitted radio leases from `radio-hand`. |
| Reticulum bearer | Ethernet/IP, WiFi/IP, cellular/IP, perhaps Wi-Fi HaLow/IP | Retinue interface with its own link, cost, availability, and metering facts. |
| Management carrier | USB, NFC, BLE, local IP, Reticulum | Carries the same signed control semantics; transport access grants no authority. |
| Local gateway facet | RS-485/Modbus, CAN, dry contact, environmental sensors | Parses a local application protocol and re-originates typed events under an explicit gateway identity. |
| External radio appliance | SX1302 concentrator, Linux gateway, proprietary satellite terminal | Lives behind a host/coprocessor interface until its execution model is genuinely embeddable. |

An Ethernet or cellular interface does not become a radio personality. A
Modbus register does not become a Reticulum destination by being forwarded.
The gateway owns the semantic and identity transition and records provenance.

## Product opportunities, in priority order

| Priority | Product | Why it is useful | Main new proof |
| --- | --- | --- | --- |
| Existing plan | Wall node | Planned household powered transport, WiFi bearer, and BLE/USB commissioning. | WN0-WN8 remain its authority. |
| Existing plan | Magnetic pocket node | Planned personal autonomous node with proximate NFC and optional BLE. | PN0-PN8 remain its authority. |
| 1 | Outdoor powered edge node | High placement, external antenna, reliable power, Ethernet backhaul, and room for a second radio or backup cell. Fits poles, roofs, eaves, barns, and public buildings. | Protected power, outdoor mechanics, antenna isolation, Ethernet, watchdog, and unattended recovery. |
| 2 | Solar field relay | Autonomous coverage where power and backhaul are absent. Can also act as a low-duty listener or sensor origin. | Worst-season energy budget, MPPT, cell-temperature safety, role shedding, and multi-day source loss. |
| 3 | DIN-rail/DC node | Clean installation in cabinets, farms, workshops, utilities, and building controls. | Wide-input protected DC, Ethernet, and isolated local-bus gateway facets. |
| 4 | Field base node | Portable high-duty node with replaceable battery, USB-C PD, foldable solar input, and two well-separated antennas. | Human-portable thermal, charge, battery-swap, and multi-radio operation. |
| 5 | Sensor stake | Small solar or primary-cell endpoint for weather, soil, water, or contact sensors. Endpoint/announce duty by default, not a router. | Sub-threshold sleep, sensor power gating, bounded telemetry, and low-energy protocol schedule. |
| 6 | Vehicle node | Mobile relay with GNSS and optional read-only CAN gateway. | 12/24 V transients, ignition states, EMC, temperature, and position policy. High validation burden. |
| Research | Multichannel listening station | Dense survey and LoRaWAN observation using an SX1302-class concentrator. | Separate host/coprocessor execution and capture semantics; it is not a `radio-hand` port today. |
| Research | Multiband or satellite field node | Sub-GHz plus 2.4 GHz or satellite-band experiments using an LR1121-class radio, or cellular/NTN through a certified modem. | New driver, RF front end, antennas, regional approval, and explicit recurring service cost. |

The rooftop/eave, parking-lot, farm, and campus nodes are installation profiles
of the outdoor powered edge product, not separate boards. The cabinet and
solar products may later share a compute/radio module, but only after their
first schematics make the common boundary concrete.

## Outdoor powered edge node

The first board is single-radio-capable and dual-radio-ready. It should not pay
the cost of the second transceiver until HW5 establishes a use that one radio
and disciplined leases cannot serve.

### Power and mechanics

- Primary input: IEEE 802.3af/at Type 1 PoE, or a protected isolated DC input
  fed by an external listed supply. A 13 W Type 1 PoE budget is already much
  larger than the radio requires and leaves room for Ethernet, heaters only if
  justified, charging, and transient margin.
- Field connection: outdoor-rated Ethernet through a gland or a keyed
  pluggable DC connector with a separate local service port. The exact DC
  range is selected from measured 12/24/48 V installations rather than called
  “wide input” in advance.
- Backup: a small LiFePO4 cell or supercapacitor sized for an owner-selected
  graceful-outage goal. It preserves routing only if the energy budget admits
  it; otherwise it closes durable work and enters Reserve.
- Protection: replaceable fuse where appropriate, reverse input, surge, ESD,
  brownout, connector shield/chassis strategy, and measured conducted/radiated
  noise. Lightning protection and bonding are installation-system concerns as
  well as board features.
- Enclosure: UV-stable outdoor enclosure, gasket and pressure equalization,
  drain/condensation path, tamper evidence, and serviceable antenna/power
  connectors. Ingress and temperature claims wait for the assembled SKU.
- Placement: put the radio close to the antenna and carry Ethernet/power down
  the pole. Long lossy coax spends the benefit of high placement before the
  receiver sees it.

A switched lighting circuit is modeled as an intermittent source, not a
failure. Its backup target may be “bridge the daylight outage” rather than a
few minutes of graceful shutdown. A pole's existing solar controller or
battery may be used only through an approved, current-limited DC interface
whose undervoltage policy protects the lighting system. The radio node never
assumes that another product's panel or battery has spare energy.

### Radio and backhaul

An external sub-GHz antenna with a real ground/reference design is the trunk.
The enclosure reserves physical separation for a second antenna and for
Ethernet magnetics/switching supplies. Dual SX1262 is the lowest-risk second
radio after HW1; one radio may keep Reticulum receive while the other scans or
serves a foreign profile. Their collision domain remains shared until
desensitization testing proves otherwise.

Ethernet is both a Reticulum bearer and a management carrier, with separate
authority at the semantic layer. Loss of Ethernet does not stop the radio
executive. Loss, restoration, DHCP/static configuration, captive upstream,
metering, and route preference are explicit states surfaced to Signalman.

## Solar field relay

The solar product is designed from an energy ledger, not from a nominal panel
wattage. Its physical chain is:

```text
panel -> input protection -> MPPT charger -> LiFePO4 -> protected load switch
                                                |-> fuel/energy measurement
```

This diagram is architectural, not a selected schematic. A current solar
charger such as TI's BQ24650 demonstrates the relevant class: 5-28 V solar
input, MPPT input regulation, LiFePO4 support, and battery-temperature
monitoring. Part selection remains HW4 and must include availability,
quiescent current, cold behavior, conversion efficiency, and real panel/cell
measurements.

Sizing uses the worst credible service season and installation orientation:

```text
measured daily harvest * conversion margin
    >= measured daily load + recovery reserve + battery aging margin
```

Every term is a receipt value. Firmware does not invent sunlight forecasts.
Signalman may combine panel history and weather data to recommend a policy,
while the node remains safe and autonomous without that forecast.

Suggested energy shedding order is owner-configurable:

1. reduce observation retention and high-cost backhaul;
2. reduce foreign-profile scans and optional second-radio receive;
3. narrow forwarding/announce work within FT policy;
4. become a Retinue endpoint with sparse presence;
5. enter Recovery with local wake and signed-control reception only where the
   remaining budget can honestly support it.

Battery charge is inhibited outside the cell's qualified temperature range.
The node may still operate directly from harvest if the power design proves
that state. “Solar present” never implies “battery may charge.”

## Candidate silicon and what it would prove

These are evaluation classes, not BOM decisions.

| Candidate | Opportunity | Admission issue |
| --- | --- | --- |
| Current nRF52840 + SX1262/T114 path | Known board, firmware, radio, and BLE baseline. | Existing memory limits, open metered-power work, and one concrete board layout. |
| ESP32-S3 + SX1262/V4 path | Known wall-node route with WiFi/BLE. | Coexistence, power, native-node allocator, and unattended RX remain active WN work. |
| STM32WL5x/STM32WL5M | MCU and sub-GHz LoRa-capable radio in one low-power part/module; the vendored driver already points to an STM32WL integration. | Prove enough RAM/flash for the selected resident feature set, internal-radio ownership, board store, boot trust, and physical RF. |
| STM32U5-class MCU + SX1262 | More memory and a main MCU without a 2.4 GHz radio, useful for NFC-only, solar, or industrial boards. | New board HAL and boot/update path; no integration exists here. |
| RAK4631/WisBlock | nRF52840 plus SX1262, modular sensors and I/O, external antenna, and battery/solar base options make it a strong sensor-stake and energy-policy prototype. | A related MCU/radio pair is not a board port; pin, power, charger, store, boot, and on-air facts must be earned on the exact base/modules. |
| External F-RAM | Fast, high-endurance nonvolatile event and counter storage without flash erase stalls. | Device-specific integrity, retention, capacity, bus contention, and power-cut receipts. |
| LR1121-class transceiver | Sub-GHz, 2.4 GHz, and satellite S-band LoRa/LR-FHSS experimentation. | Outside current `lora-phy`/Executive support; new radio port, RF network, antennas, regulatory matrix, and on-air peers. |
| nRF9151-class SiP or certified cellular modem | LTE-M/NB-IoT, GNSS, and possible NTN backhaul for isolated sites. | Prefer a bounded modem/coproc bearer first. Vendor firmware, subscriptions, network availability, current spikes, and certification prevent treating it as a transparent MCU swap. |
| SX1302-class concentrator | Simultaneous multi-channel/SF listening for surveys or LoRaWAN gateways. | Gateway baseband and host execution, not the current single-packet-radio executive. Keep behind Tulle/host until a bounded embedded owner is proven. |

## Protocol and bearer admission

The family should widen hardware opportunities without turning “supports every
LoRa protocol” into a goal.

### Admit next

- **Ethernet/IP Reticulum bearer.** Highest value on powered edge and DIN-rail
  nodes. It gives deterministic backhaul and PoE in one installation while
  preserving autonomous LoRa operation.
- **Energy-aware Retinue forwarding.** Apply FT1 airtime accounting, FT3
  bidirectionality/ETX, and FT5 forwarder scope to `PowerLease` decisions.
- **A LoRaWAN Class A telemetry adapter.** Useful for sensor stakes and solar
  health reports where a compatible gateway already exists. It is a separate
  network, key set, address model, and server relationship. It does not bridge
  Reticulum identities. Continuous Class C receive is considered only on
  externally powered hardware and after measured coexistence.
- **UMSH exact-wire evaluation.** The protocol survey already names it the best
  new permissive candidate. It enters through the same resident-adapter lease
  and source/provenance gates as any other radio protocol.
- **One explicit local-bus gateway facet.** RS-485/Modbus is the best cabinet
  proof; read-only CAN is a later vehicle proof. Each mapping names source
  register/frame, units, sampling, identity, and failure semantics.

### Keep experimental

- **LR-FHSS and multiband LoRa** need an LR1121-class driver and real peers.
- **Wi-Fi HaLow** may be a useful long-range IP bearer, but is admitted as a
  certified module/coprocessor after its current, region, driver, and ecosystem
  costs are measured.
- **LTE-M, NB-IoT, and NTN** are paid backhauls with coverage and provider
  authority. They are failover bearers, never prerequisites for local routing.
- **Thread/802.15.4** can be a building or sensor gateway only when a concrete
  local application needs it. It does not improve the sub-GHz mesh by existing
  on the MCU.

### Keep out of the first embedded family

- SX1302 concentrators and Linux gateways remain host-tier appliances.
- APRS/AX.25 and M17 require their own PHY, legal, and peer receipts. SX1262
  packet FSK capability alone does not implement their audio or 4FSK systems.
- Proprietary cloud-only sensor protocols are not admitted without an offline
  failure mode and an explicit authority boundary.

Every new protocol or bearer must have a concrete consumer, readable/public
specification or permitted oracle, regional profile, energy model, bounded
queues and response windows, malformed-input tests, coexistence receipt, and
Signalman visibility. A hardware data-sheet capability is not a protocol
receipt.

## Proposed ownership

| Concern | Owner | Stop line |
| --- | --- | --- |
| Radio-port vocabulary, collision domains, energy policy/leases, live interruption, and shared capability types | `crates/radio-hand` | No board HAL, network credentials, fleet projections, or product UI. |
| Concrete MCU, radio, Ethernet, charger, sensors, watchdog, store, and recovery implementation | board firmware crate | No cross-board scheduling or command-authority policy. |
| Reticulum route, link, announce, and bearer semantics | `crates/retinue` plus its resident adapter | No physical rail, charger, or radio-register ownership. |
| Host serial/concentrator/coprocessor interfaces | `crates/tulle` | No resident board policy. |
| Product commissioning, fleet state, energy history, recommendations, and placement | Signalman | No claim that a requested rail/profile actually became effective. |
| Signed package catalog, offline firmware authority, flashing, recovery image, and rollback authorization | Linkboy/WN6 | No protocol-adapter selection disguised as an image. |
| Installation, enclosure, supply, antenna, surge/bonding, and product qualification | exact hardware SKU and deployment receipt | No family-wide inference from one prototype. |

## Phases and done-conditions

### HW0. Freeze the admission and product-profile vocabulary

**Writes:** radio-free capability types and golden fixtures for support level,
clock/store/recovery, power source, power measurement, radio port,
collision domain, bearer, and local gateway facets. Record exact current V4
and T114 facts plus proposed powered-edge and solar facts; Unknown is valid.

**Done when:** host and at least one firmware target consume the same bounded
fixtures; missing capabilities refuse the dependent feature; the V4 and T114
do not gain claims beyond their existing receipts; Signalman can render an
unknown or partial profile without inventing a board class.

### HW1. Extract the live radio port

**Writes:** allocation-free `RadioPort` and diagnostics boundaries in
`radio-hand`, with the current `lora_phy::LoRa` adapter. Move concrete radio
types outward without moving region, duty, observation, or adapter policy out
of the executive.

**Done when:** base and feature-gated host tests pass; locked T114 and V4 builds
pass; existing exact profile, CAD, TX, RX, damaged-frame, duty, and diagnostic
fixtures are unchanged; physical T114 and V4 exchanges reproduce their
current received bytes and transition evidence; a fake second driver proves
the executive contains no `LoRa` concrete type.

### HW2. Add measured power and energy admission

**Writes:** `PowerSnapshotV1`, `PowerPolicyV1`, `PowerLease`, state transitions,
observation events, and a deterministic harvest/load/brownout model. Keep
physical rail operations behind a board-owned witness, parallel to the live
radio quiet witness.

**Done when:** model runs cover external loss, uncertain state of charge,
night/low harvest, cold-charge inhibit, load spike, brownout, recovery, and
policy change; leases cannot spend below reserve; cancellation leaves a
bounded state; Signalman distinguishes measured, estimated, and unknown facts;
no scheduler-only result is reported as a physical rail transition.

### HW3. Prove the outdoor powered edge node

**Writes:** one prototype firmware target and board support for single SX1262,
external antenna, Ethernet, PoE or protected isolated DC, watchdog/reset cause,
power telemetry, local recovery, and optional backup storage.

**Done when:** the assembled node cold-boots without a host, exchanges a
Reticulum payload over LoRa in both directions, transports one payload between
Ethernet and LoRa, survives Ethernet loss/restoration, survives abrupt primary
power loss at every durable transition, reboots from watchdog into a stated
cause, remains locally recoverable, and records measured idle/RX/TX/backhaul
power plus RF and thermal behavior in its final enclosure. The receipt names
the exact supply and installation protection used; it makes no raw-mains
claim.

### HW4. Prove the solar field relay

**Writes:** protected panel input, MPPT charger, LiFePO4 and temperature
monitoring, load/harvest measurement, energy-state driver, and the single-radio
solar product policy.

**Done when:** calibrated measurements bound sleep, RX, TX, processing,
storage, and charger losses; a declared worst-season model predicts a physical
day/night run within a stated tolerance; charge is inhibited at the tested
temperature bounds; removal/restoration of panel and battery cannot corrupt
durable state; every energy state executes its declared role; the node returns
from Reserve without a host and resumes an on-air Reticulum exchange.

### HW5. Add multi-radio ownership

**Writes:** multiple radio ports, collision/resource domains, scheduler
assignment, shared-antenna/front-end exclusion, and per-port observation.
Start with two SX1262-class ports; multiband silicon is a later driver.

**Done when:** a two-radio prototype keeps one exact Reticulum receive profile
while the other performs a declared scan or adapter lease; conflicting TX/RX,
shared supply, SPI, and RF-switch use is deterministically serialized; reset or
fault of one port does not falsify the state of the other; measured self-
desensitization establishes which supposedly simultaneous states are actually
allowed; Signalman attributes every event and gap to a port and collision
domain.

### HW6. Close unattended durability, recovery, and update

**Writes:** high-endurance store adapter where justified, watchdog history,
energy-safe observation retention, and the exact board integration needed to
consume WN6/Linkboy verified trial, confirmation, rollback, and recovery.

**Done when:** replay/control monotonicity and bounded observation retention
survive injected cuts at every write point; high-rate records do not require a
flash erase quiet window when the admitted store says they do not; a bad,
silent, or power-interrupted trial image rolls back; the offline firmware key
and boot verifier satisfy FS4; local recovery works after all network bearers
are removed. This gate cannot close before the reused WN6 trust path is itself
closed for the target board.

### HW7. Admit one adjacent network and one local gateway

**Writes:** LoRaWAN Class A telemetry adapter on a sensor/solar profile and an
RS-485/Modbus gateway facet on a powered/DIN profile. Reuse the executive lease,
energy, region, provenance, and signed-control boundaries.

**Done when:** the LoRaWAN adapter interoperates with an independent gateway,
returns to the declared Retinue scan plan, accounts nested airtime/energy, and
cannot reuse or expose Reticulum identity keys; the Modbus facet records exact
register/unit/source mappings, rejects malformed/late data, and re-originates
typed events under the gateway identity; removal of either feature leaves the
core node behavior and recovery path intact.

### HW8. Qualify follow-on products by measured capability

**Writes:** per-SKU manifests and receipts for the DIN-rail node, field base,
sensor stake, vehicle node, or later multiband/cellular variants. Only a real
consumer opens a SKU lane.

**Done when:** each shipped claim names its exact PCB, populated options,
firmware revision, enclosure, antenna, region, supply, enabled bearers,
temperature/ingress class, power policy, and receipt; absent options disappear
from the manifest; Signalman does not infer capability from a marketing name;
and regulatory, battery transport, installation, and end-of-life obligations
are recorded before distribution.

## Dependency order

```text
HW0 admission vocabulary
  -> HW1 radio port
  -> HW2 power and energy
       -> HW3 powered edge -> HW4 solar
       -> HW5 multi-radio (only for a measured consumer)
       -> HW6 unattended durability/update (also consumes WN6 and FS4)
       -> HW7 adjacent protocol and local gateway
       -> HW8 exact-SKU qualification
```

WN and PN continue independently where they do not need HW1/HW2. If their
implementation reveals the same radio or power seam first, that code lands
under HW1/HW2 and the product plan records the consumer receipt.

The T114 and a RAK4631/WisBlock base can exercise HW2 and early solar/sensor
policy before a custom PCB. Neither development platform closes HW4: its
charger, quiescent load, protection, enclosure, battery, panel, antenna, and
temperature behavior are part of the solar product receipt.

The smallest coherent implementation slice is HW0 plus the current-SX1262
half of HW1. The smallest new hardware slice is then a single-radio powered
edge prototype. Solar, dual radio, cellular, and concentrator work do not block
it.

## Findings

- **2026-09-07, live radio seam:** `crates/radio-hand/src/executive.rs` and
  `service.rs` depend directly on `lora_phy::LoRa<RK, DLY>`. `ChipDiagnostics`
  exposes the same concrete type. A new radio family cannot be called resident
  `radio-hand` support until this seam changes or it implements the exact
  `lora-phy` shape.
- **2026-09-07, host boundary is not reusable:**
  `crates/tulle/src/radio_io.rs::PacketRadio` is an allocating host-side async
  interface over serial RNode/direct-PHY links. It does not own profile apply,
  CAD, IRQ/reset, duty, or RX restoration and should not be lifted into the
  firmware executive.
- **2026-09-07, current driver reach:** the vendored `lora-phy` contains SX126x
  and SX127x implementations and documents STM32WL integration. This is source
  feasibility, not a Retinue board receipt.
- **2026-09-07, capability vocabulary is incomplete:** WN0's `BoardClass`,
  `RadioCapability`, `ManagementCarrier`, and `ResidentAdapter` describe
  product/control capability, but do not express physical power sources,
  measurement confidence, watchdog/recovery, multiple collision domains,
  backhaul cost, or local gateway facets.
- **2026-09-07, current hardware execution remains narrow:** the T114 owns the
  resident executive path; the V4 retains additional concrete radio ownership
  while its standalone native-node, WiFi/BLE coexistence, and low-power work
  remain WN gates. This plan must not rewrite those receipts as a generic
  platform success.
- **2026-09-07, durable-write pressure is already architectural:**
  `BoardStore::save` documents an erase stall that blanks receive; live control
  already requires `QuietWindow`. A high-endurance external store is therefore
  a functional radio-availability opportunity, not merely more capacity.

## External references inspected 2026-09-07

- [Semtech LR1121](https://www.semtech.com/products/wireless-rf/lora-connect/lr1121)
  for the multiband LoRa/(G)FSK and LR-FHSS opportunity. It is not supported by
  the current Retinue driver path.
- [Semtech SX1302](https://www.semtech.com/products/wireless-rf/lora-core/sx1302)
  for the multichannel gateway/baseband distinction.
- [ST STM32WL series](https://www.st.com/en/microcontrollers-microprocessors/stm32wl-series.html)
  and [STM32WL5M modules](https://www.st.com/en/microcontrollers-microprocessors/stm32wl5m-modules.html)
  for integrated MCU/sub-GHz radio and module candidates.
- [ST STM32U5 series](https://www.st.com/en/microcontrollers-microprocessors/stm32u5-series.html)
  for an ultra-low-power main-MCU class without an integrated 2.4 GHz radio.
- [Nordic nRF9151](https://www.nordicsemi.com/Products/nRF9151) for the
  LTE-M/NB-IoT, GNSS, NTN, and DECT NR+ modem/SiP opportunity.
- [RAK4631/WisBlock](https://docs.rakwireless.com/product-categories/wisblock/rak4631/datasheet/)
  for an nRF52840/SX1262 modular sensor and early solar-policy prototype.
- [TI TPS23758](https://www.ti.com/product/TPS23758) as one current Type 1 PoE
  powered-device topology reference, not a selected part.
- [TI BQ24650](https://www.ti.com/product/BQ24650) as one current solar MPPT and
  LiFePO4-capable charger topology reference, not a selected part.
- [Infineon F-RAM](https://www.infineon.com/products/memories/f-ram-ferroelectric-ram)
  for bus-speed nonvolatile writes, high endurance, and low-energy event
  logging as an alternative to frequent internal-flash erases.
- [LoRa Alliance developer reference](https://lora-alliance.org/lorawan-for-developers/)
  for the distinct LoRaWAN end-to-end architecture, communication classes,
  regional parameters, and certification boundary.

## Progress

- **2026-09-07:** plan founded. Ruled the logical platform contract, ranked the
  outdoor powered edge and solar field relay as the next new consumers, kept
  raw mains and concentrators outside the first embedded board, and defined
  HW0-HW8. All implementation and physical gates remain open.
