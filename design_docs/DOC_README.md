# Murm Documentation Index

Pipit codec source consolidation is recorded in the [family integration plan](2026-11-02_murm_family_move_plan.md#pipit-consolidation-2026-11-04); its independent crate is consumed by Outrider voice and Signalman.

Station Custody extraction (landed, 2026-11-04): the
[family integration plan](2026-11-02_murm_family_move_plan.md) records the
standalone adapter move from Mere and its unchanged-source and receiving receipts.
Dramatis/Personae and Djinn retain authority; physical qualification stays
with its existing profiles.

Stickleback networking extraction landed (2026-11-03): T7 in the
[transport consolidation plan](2026-10-28_transport_consolidation_plan.md)
places the shared live sessions and address book in `murm-sync`. Eidetic
retains validated history, group encryption and admission policy mechanisms.

WebRTC (2026-11-04): the [family move plan](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#2026-11-01-webrtc-provider-evaluation) records E0, the local E1a murm-rtc provider (moved to Genet 2026-11-08 as [`genet-rtc`](https://github.com/merely-made/genet/blob/main/components/genet-rtc/README.md); cycle-breaks D20) and E1c UDP STUN/TURN, below browser and application consumers. Genet owns conformance, its selected WPT and the qualified headed Chrome/Firefox receiving gate in `genet/design_docs/2026-11-04_webrtc_mdns_plan.md`. The E0 probe (`testing/webrtc-evaluation`) was removed on 2026-11-13; its result stays recorded in the family move plan above and in Genet's conformance plan; E1a uses an exact upstream DCEP fix found by real packet tests. The native host disables the core multicast route and uses the optional bounded provider resolver for remote UUID mDNS candidates. Application-carrier cutover remains separate.

Successor for open work: [Murm family integration and RTC](2026-11-02_murm_family_move_plan.md).

E1c adds resolved UDP STUN/TURN to the same driver, qualified against controlled
coturn with relay privacy, local metadata, refresh, refusal and socket release.
Genet receives IP-literal UDP servers and fixed-server ICE restart through the
same driver. The [restart slice](2026-11-02_murm_family_move_plan.md#rtc-ice-restart-slice)
retains the nominated path and TURN allocations while new credentials/checks
are applied; Genet's native/WPT and 48-case headed receiving gates pass on fixed
numeric UDP servers and the same interface. Explicit-port hostname resolution
also passes its controlled native/headed E2d gates; UDP SRV/S-NAPTR service
discovery passes E2s with 38 native checks, unchanged selected WPT maps and
48 headed Chrome/Firefox restart cases. The remote mDNS provider passes its
working/MSRV gates; Genet passes 44 native checks and 48 controlled headed cases
with Firefox aliases and untouched Chrome names. Its E2m packet is closed after
fixture shutdown and permission restoration. Firefox native responder acceptance,
local mDNS advertisement, IPv6 multicast, TCP/TLS, interface rebinding, server
replacement and carrier cutover remain open.
Candidate signaling intentionally omits local gatherer metadata; the host
observation retains the server URL separately.


- [Secure sessions and live interaction](2026-10-29_secure_sessions_and_live_interaction_plan.md): cut to open work 2026-10-31. N1–N3 and O1–O3 landed and are [archived whole](archive_docs/2026-10-31/2026-10-29_secure_sessions_and_live_interaction_plan.md); O4 handoffs/interoperability and deferred items remain, with the family fit survey in Findings. [Qualification record](archive_docs/2026-10-29/2026-10-29_secure_sessions_and_live_interaction_plan_receipts.md).
- [Transport consolidation](2026-10-28_transport_consolidation_plan.md): what Murm owns family-wide, where family code reaches iroh or p2panda-net without it, and what consolidation does not cut (patch tables, lockfiles). T6 (2026-10-31): admitted services onto the shared secure setup gate; Murm's session lane moved 2026-11-01.
- Moot relay pools: canonical plan at `gemot/design_docs/moothold_docs/implementation_strategy/2026-11-03_moot_relay_pools_plan.md`, with the P0-P2 qualification ledger in its linked archive receipts. Opt-in Murm `relay` and `relay-i2p` supply bounded admitted native and I2P-only stream forwarding; `murm-sync` exposes upstream peer/topic authorization hooks. Gemot owns community authority, consent and offers. Same-host native/process and named two-router public-I2P tests pass. Pool-specific provider lifecycle qualification belongs to that ledger; independent machines/networks and deployment remain P3.
- [Archived Resource client receive limits](archive_docs/2026-11-02/2026-10-22_resource_client_limits_plan.md): early encoded/decoded Resource budgets and bounded Micron page decoding, with Turnstone-owned policy and deadlines.

T114 live Tucket (2026-10-20): the [qualified endpoint receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/tucket-t114/README.md)
records four texts/four protocol ACKs against stock MeshCore across reboot,
restoring the contact before peer work with no decoded fresh advert. The
Tucket-only image keeps the 8-contact/32-dedup/2-pending/8-frame bounds and passes
sixteen pressure rounds: 75820 untouched stack bytes, 21860/49152 heap peak,
zero final occupancy and exact refusals. Actual RF peaks at 6404 heap bytes.
Original named firmware/profiles are restored and helpers stopped. Volatile
setup, installed-daemon/UI, power-cut and wider role qualification remain open.

Standalone Tucket contacts (2026-10-17): the
[firmware receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/tucket-firmware-contacts/README.md)
qualifies an opt-in eight-key public cache in dedicated A/B flash pairs.
Both storage backends recover eight keys without rewriting them; V4 exchanges
four texts/four protocol ACKs across a reboot without a fresh peer advert.
The live constructor now runs on a separate executor turn with boxed state;
the full pressure profile charges that storage and retains the selected memory
reserves. Radio-budget-derived retries replace a one-second interval that
overlapped responses. That checkpoint qualified T114's backend only; the live loop is
qualified separately above. Original named firmware/profiles are restored, and failed trials
remain recorded. Power-cut and broader endpoint/role acceptance remain open.

Tucket persistent contacts (2026-10-17): the [scoped host journal slice](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/tucket-contacts/README.md)
passes cold restore, save/refusal controls and a native Djinn-backed reconnect
without the fixture's fresh peer advert. Four texts/four protocol ACKs and exact
history pass across two connections; seven captures are reviewed. Public keys
share the existing identity/realm journal, with version-2 downgrade refusal and
no replay of routes or pending work. This supersedes R6's host contact-restore
gap below. The standalone slice above adds bounded MCU storage; broader
endpoint/role parity remains open in the [parity plan](2026-11-02_protocol_parity_plan.md).

Board names (2026-10-16): [the family-plan naming slice](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#board-names-2026-10-16) adds persistent public labels, USB naming commands and boot/status titles. Both normal images are installed and all three names saved. SB1, SB2 and T114 all pass post-reboot readback with their original radio settings; SB2 required physical reconnection after its USB endpoint stalled. The [naming receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/device-names/README.md) keeps failed attempts and successful checks distinct. The [direct-PC](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/usb-reboot/direct-pc/README.md) and [swapped-cable](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/usb-reboot/swapped-cables/README.md) controls each reproduce the timeout on their first reset; the later trace below resolves the startup cause. Batteries are connected, so USB-replug recovery does not establish a full power cycle. Tracing found a 20-second physical-claim wait on uncommissioned SB2. The firmware now preserves and replays a read-only USB query to select already-permitted ordinary service early; claim authority and pending/corrupt gates remain enforced. Software checks pass. The [recovery receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/usb-reboot/recovery-fix/README.md) records five consecutive SB2 software reboots without a replug, followed by a final ROM read and two read-only sessions. Names, identity and preferences survive; bytes outside the settings pair are unchanged. SB2 now runs fixed firmware `c8e5eb9309e`; SB1/T114 retain `cb042329232`.

Protocol parity work (2026-10-16): [the three-family plan](archive_docs/2026-11-02/2026-10-16_protocol_parity_plan.md)
separates pinned wire, device-role and firmware/product targets; ranks Tucket
first, Retinue next and Sennet's full product surface last; records measured
V4/T114 memory scopes and requires a current combined-image memory refresh
before wider firmware claims. The [current host/target memory checkpoint](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/protocol-parity-memory/README.md)
passes maximum-PKI retained workloads, fragmented 48/64 KiB host allocator
campaigns and both target capacity links. That build-only checkpoint supplies no
on-device peak/stack/fault or reserve acceptance and installs no firmware.
The subsequent [physical memory continuation](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/protocol-parity-memory/physical/README.md)
passes sixteen fragmented allocator rounds on T114 and V4, both peaking at
18452 occupied bytes with complete release and exact refusal counts. The
separate-task diagnostic avoids a verified T114 stack overrun. V4's 97344-byte
main-stack sample left only 10616 bytes in its linker region. The later
[bounded gate closure](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/protocol-parity-memory/closure/README.md)
measures boot-painted high-water after removing duplicate construction frames:
62140 untouched stack bytes on V4 and 18916 on T114, with sixteen passing
rounds each, explicit 16 KiB stack / 25% heap reserves, runtime queue/failure
pressure and restoration. This closes the named memory profile; physical
radio-failure, power-cut, deadline and unattended product acceptance remain open.

Sennet directed carriage (2026-10-16):
[R8](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#r8--sennet-directed-text-and-delivery-acks-2026-10-16-functional-pass)
records a [native PKI directed reconnect and automated restoration pass](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/sennet-directed/native-restoration/README.md):
four exact texts and four correlated ACKs against pinned 2.7.26, exact history,
disjoint text/control IDs and seven reviewed captures. Both boards restore in
the same automated run; the earlier failed DUT readback and separate recovery
remain preserved in their original receipt.
Scoped custody and software gates pass. ACK proofs for 2.8, private-channel
custody, personal-daemon integration and UI polish remain open. R7 stays frozen.

Sennet carriage (2026-10-16): [R7](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#r7--sennet-channel-text-over-tulle-2026-10-16-functional-pass)
records a [Djinn-backed public LongFast reconnect pass](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/sennet-desktop/native-reconnect/README.md):
four tagged texts across two connections, exact history, disjoint packet IDs,
zero protocol ACK claims, seven reviewed captures and both boards restored.
The Sennet-only runtime and scoped custody pass their software gates. Directed
text and protocol ACKs are qualified separately in R8; private-channel
provisioning and UI polish remain open.

Tucket carriage (2026-10-16): [R6 in the family plan](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#r6--tucket-over-tulle-through-signalman-2026-10-15-in-progress) records a [Djinn-backed reconnect-send functional pass](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/tucket-desktop/custody/native-reconnect/README.md): four tagged texts with all four protocol ACKs against stock MeshCore across two connections, with exact history checked before fresh work after reconnect. Seven captures are reviewed, both boards restored, and owned helpers stopped. Scoped custody passes real-broker, policy and restored-fallback controls; the 300-execution core gate is unchanged, and 41 desktop tests plus strict Clippy/builds pass again. The fixture explicitly advertises after each commissioning; automatic contact restoration and UI polish remain open. Sennet has its separate R7/R8 receipts above.

Radio-to-Comms integration (2026-10-15): [R1-R5 in the family plan](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#6-post-cutover-application-integration) records Signalman LXMF, resident radio Comms and stock companion actors with separate identity/delivery scopes. R5 now records passing Meshtastic and MeshCore Windows native functional runs, six reviewed captures each, and stock-pair reconnect/history results. MeshCore passes the pair fixture with an explicit 500 ms reconnect delay; immediate reconnect remains unqualified. Both V4s were restored to verified Tulle and all owned processes closed. The [stock companion receipts](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/stock-companions/README.md) freeze software and Windows source/binary/evidence ledgers. UI cleanup and Sennet/Tucket stack legs remain separate gates.

Reusable wire engines (2026-10-14): MOVE5 published the independent `wire` workspace and preserved the original Genet ancestry; **the workspace left for Genet on 2026-11-08 (`genet-wire`, `genet-rtc`; Mere cycle-breaks D20), so Murm holds no wire engine.** Genet's Netfetcher keeps its package/defaults/public policy APIs; Mere removes its duplicate crawler exchange and supplies explicit host trust/pooling. Source-bound receipts live in `Code/testing/murm/testing/receipts/wire-engines`, with final receiving checks in the Genet and Mere plans.


Murm family direction (2026-10-13): the owner authorized using this repository
as the foundation for Murm while preserving crate names and history. The
[Murm family move plan](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md) maps radio/native
workspaces and Fetch policy in Genet, and sequences the implementation,
consumer and repository cutovers. T7 supersedes the original Stickleback
placement: its core lives in Eidetic and its network sessions in Murm.
The native import and
named application cutover are published. The existing repository is now
`merely-made/murm` with the same identity, refs and crate names. Canonical URL
validation passed. Supplemental Isocosm dependency adoption is published;
its reproduced gameplay compiler failures remain in the wing after-pass.

> Canonical index per [`DOC_POLICY.md`](DOC_POLICY.md) §6. **If any other index
> in this repository disagrees with this file, this file wins.** Founded
> 2026-08-24 against 93 markdown documents and 19 JSON receipts, none of which
> were previously indexed.
>
> `PROJECT_DESCRIPTION.md` does not exist here. Core §7 reserves it for the
> maintainer, so its derivation rule for the root `README.md` is inert rather
> than violated. Mere is in the same state.

## Required reading order

1. [`DOC_POLICY.md`](DOC_POLICY.md) — documentation governance.
2. This file — the index, the naming collisions below, and the working
   principles at the foot.
3. [Work lanes](2026-08-09_retinue_work_lanes.md) — the four-lane split (Peer,
   Air, Assurance, Distribution) and the shared source lock. **Its lane split
   stands; its ordering does not.**
4. [Program sequencing and deadline order](2026-08-12_program_sequencing_and_deadline_order.md)
   — historical August ordering audit, built from a code-reading audit that
   found roughly twenty-five of the work-lane document's asserted dependencies
   unsupported. The work-lane document's own Plan audit table has no row for
   this one; read it anyway.
5. [Current appliance, listening and observation coordination](2026-08-09_retinue_work_lanes.md#coordinated-appliance-listening-and-observation-work-2026-09-06)
   — 2026-09-06 owner-requested planning and implementation split; current
   authority for these bounded slices, shared lifecycle and collection rules.

## Read this before citing any gate

**Three different `S`/`G` namespaces exist and they are not related.** Citing a
gate without its family is ambiguous and has already produced wrong readings.

| Namespace | Family | Meaning |
| --- | --- | --- |
| `MOVE0`–`MOVE5` **(Murm family move)** | [Murm family move plan](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md) | Source/history qualification, native workspace, imports, consumers, repository cutover and later HTTP/WebSocket wire extraction. |
| `G0`–`G4` **(ARDC)** | [ARDC lane](2026-07-31_ardc_application_lane.md) | Grant application gates. All unstarted; lane dormant by decision 2026-08-26. |
| `G0`–`G5` **(Signalman desktop)** | [Cambium scope](2026-08-09_signalman_cambium_desktop_scope.md) | Desktop GUI adoption gates. G0–G4 complete, G5 partial. |
| `S0`–`S9` **(Signalman management)** | [Management surface plan](archive_docs/2026-11-02/2026-08-15_signalman_management_surface_implementation_plan.md) | Real gates. |
| `S1`–`S7` **(Linkboy sidequests)** | [Public flashing plan](archive_docs/2026-11-02/2026-08-08_linkboy_public_flashing_plan.md) | **Not gates.** Explicitly labelled future-work sidequests, separate from that document's F0–F7 trunk gates. |
| `WN0`–`WN8` **(wall node)** | [Wall-node management plan](archive_docs/2026-11-02/2026-08-30_wall_node_management_plan.md) | Standalone V4, control carriers, safe configuration, image selection, and resident adapter gates. |
| `HW0`–`HW8` **(hardware family)** | [Hardware-family plan](2026-09-07_radio_hand_hardware_family_plan.md) | `radio-hand` platform admission, powered-edge and solar nodes, multi-radio ownership, autonomous power, and follow-on hardware gates. |

## Gate index

The rows retain the original gate namespaces and their historical evidence.
Use the [successors for open work](#current-open-work-successors-2026-11-02)
for remaining execution and done-conditions.

Status is what a document explicitly *states*, not what a receipt's existence
implies. Several receipts in this repository say outright that they do not close
their gate. `UNKNOWN` means no document states a status — it does not mean open.

### Peer lane — mixed-runtime interop

**Reference withdrawn, 2026-09-26.** The owner has raised unresolved concerns
about Prns's provenance. Prns is no longer an approved implementation donor,
recommended dependency, or trusted independent reference for new qualification.
This is a project trust decision, not a finding of infringement. Recommendations
below to harvest, adopt or prefer Prns are historical and superseded. Existing
attribution and measured observations remain; see the Prns donor ledger.

Historical H8 closure records observed runs, not approval for new Prns use.


| Gate | Status | Where |
| --- | --- | --- |
| H8 | **Closed**, local-TCP scope only | [2026-08-11 receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-11_prns_peer_matrix_receipt.md), re-receipted at RNS 1.5.0 in [2026-08-23](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-23_prns_peer_matrix_rns150_receipt.md), and at RNS 1.5.2 in the [current re-pin receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-29_rns_152_repin_receipt.md) |
| H1–H7, H9, H10 | UNKNOWN — catalogue entries, no status stated | [Prns harvest brief](archive_docs/2026-11-02/2026-08-09_prns_harvest_brief.md) |
| O-10 | **Partial** — local TCP resolved; RF forwarding receipt still owed | The three current and historical peer receipts |

### Air lane — radio, firmware, scaling

| Gate | Status | Where |
| --- | --- | --- |
| AIR0, AIR1, AIR2 | Closed | [Work lanes](2026-08-09_retinue_work_lanes.md), [AIR0/AIR1](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air0_air1_software_receipt.md), [AIR2](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air2_announce_ingress_receipt.md) |
| TC0–TC7 | TC0 implemented; TC1 selected OpenRQ profile qualified locally. C incremental host core/persistent peer published in Retinue 73616a78/2ee6f0c. A/D resident admitted exact-inventory/QUIC composition published in Mere 26eb8038/abdf6d51, with separate-process durable reopen. B prepared retries pending. Intermittent existing transport-suite failure preserved; broad qualification deferred until stable contracts. | [Transport conformance plan](archive_docs/2026-11-02/2026-10-11_transport_conformance_plan.md) and [retained-repair prototype](2026-10-11_retained_repair_simulation_profile.md) — canonical source/receipt pointers and owner/carrier/device/RF boundaries. |
| AIR3 | Closed, software and on-air | [software](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air3_bounded_transport_software_receipt.md), [T114 on-air](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-13_air3_t114_on_air_receipt.md) |
| AIR4, AIR5 | Open | [Work lanes](2026-08-09_retinue_work_lanes.md) |
| FT1 | Open — needs modelled-versus-measured on-air airtime | [Mesh scaling](2026-08-09_mesh_scaling_and_asymmetric_routing.md) |
| FT2 | **Partial.** The [AIR3 software receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air3_bounded_transport_software_receipt.md) and [T114 on-air receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-13_air3_t114_on_air_receipt.md) establish bounded memory, eviction and continued relay. Direct physical recent-path retention and explicit owner-gate closure remain outstanding. |
| FT3, FT4, FT5 | Open | [Mesh scaling](2026-08-09_mesh_scaling_and_asymmetric_routing.md) |
| LE1, LE2, LE4, LE5 | Open, post-deadline | [Listener executive](2026-08-10_listener_executive_and_protocol_leases.md) |
| LE3 | Partial — LE3a/LE3b complete; unattended scheduler open | [LE3 scan-physics receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-20_le3_t114_scan_physics_receipt.md) |
| O1–O2 (observations) | Complete in software: bounded codec/recorder, literal replay and timeline model; ARM/Xtensa library checks pass | [Radio observation plan](archive_docs/2026-11-02/2026-09-06_radio_observation_plan.md) |
| O3 (observations) | Physical RX/TX, cursor loss, identical reads, pressured-session recovery, concurrent RF, power-cut reset, retune refusal and zero NVMC mutations verified; detailed CPU/IRQ/USB timing remains open | [Radio observation plan](archive_docs/2026-11-02/2026-09-06_radio_observation_plan.md#2026-09-09-refusal-and-flash-cost-verification) · [Refusal/flash receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-09_t114_observation_refusal_receipt.json) · [Concurrent receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-08_t114_observation_pressure_receipt.json) |
| O4 (observations) | `e7c972d` passes aggregate CI and a physical active-USB run after application reboot: 64 identical rereads, all 12 declared RX payloads during collection, TX lifecycle and explicit overflow loss. Initial startup and the strict unread-host test failed; clean-start reliability, retirement/RF continuation, quiet/sleep and detailed cost remain open. | [V4 receipt and failed trials](archive_docs/2026-11-02/2026-09-06_radio_observation_plan.md#2026-09-10-v4-active-radio-usb-result) |
| O5 (observations) | September 21: keyboard export/reload and overwrite refusal verified; identified V4/T114 desktop collection renders with automatic saving off. Board-switch association clearing is implemented. Clipboard/scrolled accessibility, physical cancellation and synchronized coverage remain open. | [Radio observation plan](archive_docs/2026-11-02/2026-09-06_radio_observation_plan.md#o5-availability-view-and-export) · [Owner-flow receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/signalman-owner-flow/README.md) |
| O6 (observations) | Open: needs a validated second listener, including a V4 adapter with the current one-T114 bench. | [Radio observation plan](archive_docs/2026-11-02/2026-09-06_radio_observation_plan.md#o6-murmuration-acceptance) |
| PN0–PN8 (magnetic pocket node) | Active preparation; iOS probe builds and simulator receipt checked; NFC profile created, phone install awaits M4 signing-key access; every PN gate remains open | [Magnetic pocket-node plan](2026-09-07_magnetic_pocket_node_plan.md) |
| HW0–HW8 (hardware family) | Planned; logical platform boundary and product priority are ruled, while every code and physical gate remains open | [Hardware-family plan](2026-09-07_radio_hand_hardware_family_plan.md) |
| CM1 | Absorbed into LE2 | [Channel murmuration](2026-08-09_channel_murmuration.md) |
| [Murmuration controller plan](archive_docs/2026-11-02/2026-09-10_murmuration_controller_plan.md) | MC0–MC3e receipts, MC4a capacity and MC4b retained runtime; MC4c physical resident traffic, explicit loss and reset continuity. Full stack/fragmentation and fault acceptance remain open. September 26: owner confirms a PPK2, currently disconnected. September 20's non-detection was not evidence of non-ownership. Wiring, supply isolation and power qualification remain open; no power measurement receipt. |
| [Murmuration resident build receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-13_murmuration_resident_receipt.json) | MC4b source/artifact hashes, host tests and actual V4 static layout; build-only, no installed or runtime memory claim. |
| [Murmuration resident physical receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-13_murmuration_resident_physical_receipt.json) | MC4c exact V4 image, retained protocol traffic, home gaps, explicit loss, reset counter continuity and measured heap/stack samples; includes failed experiments and remaining physical gates. |
| [Murmuration session receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-11_murmuration_session_receipt.md) | Retention, resource drain, explicit loss/fresh handshake, Sennet state and V4 return during host silence; exact-build evidence and preserved settings. |
| [Murmuration physical receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-10_murmuration_physical_receipt.md) | Two V4/T114 runs: 32 exact receives, bounded returns, eight counted home receive gaps, pin refusal; V4 USB packet-boundary fix and preserved settings. |
| CM2–CM5 | Open, sequenced after LE5 | Same, translated into the executive doc |
| R0–R10 | All closed | [Retinue v0 plan](archive_docs/2026-11-02/2026-07-06_retinue_v0_plan.md) |
| N0 | Partial — power-loss unplug leg open | [Retinue Small plan](archive_docs/2026-11-02/2026-07-31_retinue_small_plan.md) |
| N1–N4, N6 | Closed | Same |
| N5 | Partial — needs current figures, meter required | Same |
| U0–U5 | USB-first UI closed; metered power and deferred product states remain open in the successor | [On-device UI plan](archive_docs/2026-11-02/2026-07-28_on_device_ui_implementation_plan.md) |

### Assurance lane — validation, security, custody

| Gate | Status | Where |
| --- | --- | --- |
| ASSURE1 | Partial — "substantially closed" | [Assurance lane status](2026-08-10_assurance_lane_status.md) |
| ASSURE2 | Automated smoke receipt passed: unsafe audit and all three Linux fuzz targets; longer campaigns remain separate | Same |
| ASSURE3, ASSURE4 | Closed | Same |
| ASSURE5 | Closed **in software**; physical FS4 separate | Same |
| ASSURE6 | First green Linux fuzz result recorded at `045cb2f`; physical gates remain separate | [Assurance lane status](2026-08-10_assurance_lane_status.md) |
| FS1 | UNKNOWN — defined, never statused | [Field node security posture](2026-08-09_field_node_security_posture.md) |
| FS2 | Closed in software | [FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md) |
| FS3 | Partial on V4: durable counter and replay refusal across power removal are physically recorded; mid-write interruption and erase-life budget remain open | [FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md), [wall-node plan](archive_docs/2026-11-02/2026-08-30_wall_node_management_plan.md) |
| FS4 | Partial — process half written, physical half not run | [FS4/FS5](2026-08-10_fs4_custody_and_fs5_seizure.md) |
| FS5 | Closed with an enforced check | Same |
| FS6 | Partial — same never-closed nuance as FT2 | [AIR3 software receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air3_bounded_transport_software_receipt.md) |
| **LOCK4** | **Open, unstarted.** The private disclosure record. `design_docs/private/` does not exist. Named only in the sequencing doc; it is source-lock item 4 in [work lanes](2026-08-09_retinue_work_lanes.md). |

### Distribution lane — installer, catalog, activation

| Gate | Status | Where |
| --- | --- | --- |
| F0, F1, F2 | Closed | [Public flashing plan](archive_docs/2026-11-02/2026-08-08_linkboy_public_flashing_plan.md) |
| F3 | Partial — no factory as-shipped board receipt | Same |
| F4 | Partial — V4 recovery via the board's own reset/boot controls open | Same |
| F5 | Closed 2026-08-19 | Same; [F5 spike](2026-08-08_linkboy_public_flashing_f5_spike.md) |
| F6 | Partial — complete for Windows only | Same |
| F7 | Closed 2026-08-20 — **but see Known divergences** | Same |
| DIST0–DIST5 | Closed | [Work lanes](2026-08-09_retinue_work_lanes.md) |
| DIST6 | Deferred | Same |
| DIST7 | Partial — no on-device activation receipt on either board | Same; [catalog auth foundation](2026-08-20_catalog_auth_and_activation_foundation.md) |

### Signalman — desktop and management surface

| Gate | Status | Where |
| --- | --- | --- |
| G0–G4 (desktop) | Closed | [Cambium scope](2026-08-09_signalman_cambium_desktop_scope.md) |
| G5 (desktop) | **Partial.** G5.3 done; G5.1 and G5.2 open — two headed Windows runs remain unrecorded. Their accessibility conditions were met 2026-08-24 (`bd71ee1`). | Same |
| S0, S1 (management) | Closed | [Management surface plan](archive_docs/2026-11-02/2026-08-15_signalman_management_surface_implementation_plan.md) |
| S2 | Partial — headed interactive judgement is owner-driven | Same |
| S3 | Closed | Same |
| S4 | Partial — process boundary closed, not a headed serial-radio receipt | Same |
| S5 | Partial — file-backed and host-audio rungs done; headed two-site audible run remains | Same |
| S6–S9 | UNKNOWN — defined, never statused | Same |
| SP0–SP5 (survey and placement) | SP0 bounded GPX/capture import and source-preserving replay pass 11 focused tests and strict library Clippy. SP1–SP5 remain planned; Map and field acceptance are separate. | [Survey and placement plan](2026-09-06_signalman_survey_and_placement_plan.md) |
| WN0 | **Partial** — deliberately revised to `RHC0` v2 with `transaction_sequence` before shipment; host and source-level target consumers exist, but target execution is unreceipted | [Wall-node management plan](archive_docs/2026-11-02/2026-08-30_wall_node_management_plan.md) |
| WN1 | **Partial** — portable durable authority, the pre-radio V4 USB first-write slice, and host `Inspect`/`Claim`/`Resume`/`Abandon` controller exist. The 2026-09-02 physical package was built from `0cbf558c76db52b57794138b82e2c9e088a32f0c`; the 5,918,136-byte release ELF hashes to `6df78fffa2adc006dcdb638b14ce3ebdb5a0e27b64bc447f80eff44a6d944894`, and the official `espflash` merged image is 390,544 bytes ending `0x05F590`, hash `c6244c9d25eb74a0ca0f2a347edfaa4f4f568e230d8d8374f3921d65709dbcab`. Guarded writes covered `0x0..0x3F0000` and preserved `0x3F0000..0x400000`; Linkboy completed COM6 on parent `USB\\VID_303A&PID_1001\\44:1B:F6:6A:FB:28` as Heltec V4 `0.0.1`, US915 modem. Host tests passed for `radio-hand` (2), `postilion` (2), and Signalman (1); the locked Xtensa check was green with two pre-existing warnings. The baseline nonce-bound read was exactly `auth=diagnostic-only transport=modem-only node=a4dbb760a85b3a2e38cb54c62ec6d4df control=valid pending=blank boot=known-good-applied known-good-generation=0 generation-watermark=0`; after true USB power removal/replug with both buttons untouched, the same parent/COM6 re-enumerated at local `2026-09-02 18:11:47` and returned the exact same second read. This closes durable claim/power-cut recovery evidence for ordinary control state, but the read is unauthenticated and the nonce supplies query freshness only. The current worktree adds the shared signed verifier/carrier: `retinue` has a default-on `alloc` feature so its allocation-free floor links into the core-only USB image, which restores the verifier from durable grants, keeps the runtime resident, and answers a signed WN0 `Status` only after journaling the outer counter inside the live quiet window. Physical receipt on the claimed board: `auth=verified-controller` at counters 1 through 5 from the Mere controller key, a replayed counter refused by silence both before and after a true USB power cut, the journaled counter surviving that cut, and the diagnostic still answering afterwards. The carrier now serves the configuration lifecycle (`ProvisionalApply` with a controller-chosen change id and bounded lifetime, `Commit` by board-minted token, `Revert`, and on-board expiry), host-model proven end to end; the image carrying it is flashed and re-verified on the claimed board, where an ungranted key's `apply` earns silence and the granted key still reads `Status`. Open: the applied-and-committed receipt under the Mere controller key, vault, native node, and every other carrier. The earlier clean-pinned init/physical Claim→Resume receipt remains recorded in the plan; WN2–WN8 remain Open. | [Wall-node management plan](archive_docs/2026-11-02/2026-08-30_wall_node_management_plan.md) |
| WN2 | **In progress** (2026-11-20): software half built and host-tested (node image, signed commands in the node loop, live relay, D1 to D22); the node image boots and announces on one V4; nothing relayed yet; heap figure and bench run open | [Wall node plan](2026-11-02_wall_node_management_plan.md#wn2-execution-2026-11-20) |
| WN3–WN8 | Open | Same |

### Live-gate flake lane

| Gate | Status | Where |
| --- | --- | --- |
| FLK1, FLK2 | Open — per-gate baselines at declared *n*, with load recorded | [Flake lane](2026-08-23_live_gate_flake_lane.md) |
| FLK3 | Closed — two identified bugs | Same |
| FLK4 | **Explained, not closed.** Mode survives ~1 in 120; the 750 ms settle is a constant fitted under load, and half its supporting inference was retracted 2026-08-24. | Same |
| FLK5 | Partial — the oracle README now states the policy; the audit of other receipts is not done | Same |

### External and pre-decision lanes

| Gate | Status | Where |
| --- | --- | --- |
| **G0–G4 (ARDC)** | **Dormant by decision 2026-08-26.** The Sep 1 window was deliberately passed; FIVCO-facilitated funding now leads. Next window Feb 1, 2027; the G0 draft email keeps option value for it. | [ARDC lane](2026-07-31_ardc_application_lane.md) |
| CV1–CV7 | Open, pre-decision, post-deadline | [Civic deployment](2026-08-11_civic_deployment_prescribed_paths.md) |
| D1, D2, D3 | Ruled 2026-08-12 (decisions, not implementations) | Same |
| D4 | Partial — ruled, still needs a spike receipt | Same |
| D5 | Dissolved; CV7 carries the primitive | Same |

## Known divergences

Historical audit as of 2026-08-24, with bounded corrections on September 26.
The catalog and G2 entries below have not been revalidated in this refresh;
verify their current owners and artifacts before treating them as active work.
The root README's T114 status and the flake tool path were corrected September 26.

- **The published firmware catalog is out of sync with this repository.**
  Mer3ly pins its projection to `index.toml` at `05b3795`, whose SHA-256 it
  records; `35f3f7b` has since moved that file to `version = "3"` and added
  `release_sequence` to every package. So the published catalog advertises v2
  semantics and carries none of the release sequences the authenticated-update
  path depends on. Owned by Distribution / F7 with DIST7.

  Republishing is blocked on a schema change rather than a content refresh:
  Mer3ly's catalog structs all use `serde(deny_unknown_fields)`, so today's
  index fails to deserialize there, and `RETINUE_PACKAGE_INDEX_URL` is a
  hardcoded constant still pinned at `05b3795`.

  **This is not the `write_bytes` defect, and an earlier version of this entry
  said it was.** Mer3ly's projection carries no payload fields at all — no
  `write_bytes`, `byte_length` or `sha256` — so the live pages never served the
  22×-wrong value and no flash was ever driven from them. That bug lived in
  `heltec-v4-current.toml` and was fixed in `a644ea5`. The claim was inherited
  from an audit summary and restated here without checking the projected file,
  which is the exact failure this document's working principles describe.
- **The Plan audit table** in [work lanes](2026-08-09_retinue_work_lanes.md) is
  a rival index to this file. Six of its 24 rows were stale on 2026-08-24 —
  ARDC, Linkboy/F7, both Signalman rows, the management surface, and Outrider —
  and several were re-edited *after* the evidence that undercuts them existed.
  Per core §6 this file wins. Treat that table as history.
- **The [G2 receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-09_signalman_desktop_g2_receipt.md)** records
  `tests/accessibility.rs, 4 cases, passing`. There are five cases, and as of
  `bd71ee1` all five pass. Its supersession note covers the pin revision only,
  not this count.

## Terminology

Names in this program are invented and cannot be inferred.

**This project's crates.** `retinue` — endpoint-scoped Rust implementation of
the Reticulum mesh protocol, wire-compatible with RNS. `outrider` — LXMF
boundary crate: message codec, delivery state machines, propagation
client/server. `postilion` — shared radio-host library; a Station wraps one
identity, one board, an announce cadence and a peer table behind an event
stream. `selvage` — clean-room LoRa PHY profiles shared by host and firmware.
`tulle` — the shared radio interface layer beneath retinue (`sennet` and
`tucket` were folded into Signalman's stock-companion client on 2026-11-14). `radio-face` — firmware crate rendering bounded on-device UI status.
`radio-mirror` — unpublished browser realization of radio-face: RGBA
framebuffer, the firmware's `Controller`, wasm exports.
`retinue-sim` — unpublished in-process route-trace harness over Retinue nodes;
its `face` feature maps node states onto radio-face and emits face tracks.
`radio-hand` — board-agnostic firmware crate that owns the shared board
executive, radio work, and durable control decisions. Retinue is its RNS
adapter/router, not the executive. Firmware slot activation remains Linkboy
authority and is not a `selvage` responsibility.

**This project's applications.** `linkboy` — CLI that identifies firmware by
asking rather than trusting USB IDs, then verifies and flashes packages.
`signalman` — terminal application running a household radio over one serial
port. `signalman-desktop` — desktop GUI on the Cambium/Genet toolkit; roots its
own workspace, deliberately outside the root one.

**External.** `hopspot` — Prns's Wi-Fi-hotspot firmware for Heltec V4; Linkboy
can flash it but claims no Retinue capability. `cambium` — Genet-native GUI
toolkit. `genet` — sibling engine repository providing Cambium's desktop host.
`prns` — Rust Reticulum implementation previously used as a peer and donor;
withdrawn as a reference September 26 pending provenance reassessment. `mere` — sibling repository
supplying data-graph and identity crates.

## Document index

### Current open-work successors (2026-11-02)

Historical gate numbers and dated findings below cite the archived originals.
**Successors for open work:**

- [Announce timebase: remaining work](2026-11-02_announce_timebase_plan.md): Reservation recovery, guarded package and physical power-cut/on-air evidence; natural expiry and remaining reference corrections.
- [Wire compatibility and ownership boundaries](2026-11-02_wire_compatibility_plan.md): Embedded IFAC, decoder peak allocation, stock/stress and image acceptance; optional v0 flag-mask evidence carried here.
- [Retinue small node: remaining work](2026-11-02_retinue_small_plan.md): Actual power loss and current measurement, scheduling, RNode/client follow-ons and supervision.
- [Wall node and carrier-agnostic management: remaining work](2026-11-02_wall_node_management_plan.md): WN0–WN8 open contract, durable lifecycle, native node, carriers and appliance profiles; preserves completed Claim/Resume evidence.
- [Linkboy public flashing: remaining work](2026-11-02_linkboy_public_flashing_plan.md): Factory-device and V4 hardware recovery, authenticated catalog and optional update routes.
- [On-device radio UI: remaining work](2026-11-02_on_device_ui_implementation_plan.md): Metered display/sleep energy and product states gated by their actual owners.
- [Signalman management surface: remaining work](2026-11-02_signalman_management_surface_implementation_plan.md): Headed interaction, radio text, two-site voice, placement/telemetry, Browse and bounded guest acceptance.
- [Radio observation and Signalman availability](2026-11-02_radio_observation_plan.md): O3/O4 physical cost and retirement, O5 desktop acceptance and O6 two-listener measurements.
- [Murm family integration and RTC](2026-11-02_murm_family_move_plan.md): R1–R8 receiving and product follow-ons, supplier gates and RTC continuation; historical cutover and receiving evidence stays archived.
- [Protocol and firmware feature parity](2026-11-02_protocol_parity_plan.md): Role-specific Retinue/Tucket/Sennet parity and exact-image resource/product acceptance.
- [Transport standards and retained repair conformance](2026-11-02_transport_conformance_plan.md): Broader codec/corpus/carrier and retained-repair qualification, device/physical measurements and optional adapters.
- [Murmuration controller and shared-radio measurements](2026-11-02_murmuration_controller_plan.md): Resident failure and control acceptance, cooperative schedules and MC5 measured shared-radio cost.
- [One shared transport core for Node and Endpoint](2026-11-18_shared_transport_core_plan.md): T0–T5. Settle 13 behavioural differences between the two drivers against stock RNS, then move transport routing into one clock-free module. Landed 2026-11-19: the transit tables and forwarding decisions are shared in `crate::transport`, and route life follows stock RNS 1.6.0.
- [Retinue I2P SAM seam](2026-11-02_i2p_sam_seam_plan.md): Identity fork, remaining current-cohort interoperability, conditional datagrams and upstream findings; 0.1.1 publication is complete.

Retinue v0 and radio-mirror are archived without separate successors. Their
historical alternatives and acceptance evidence remain intact.

### Governance and program

| Document | What it is |
| --- | --- |
| [`DOC_POLICY.md`](DOC_POLICY.md) | Documentation governance; canonical core plus Retinue addendum. |
| [Repository layers plan](2026-11-25_repository_layers_plan.md) | Decided 2026-11-25, nothing moved yet: Murm, P2mere and Stickleback regroup into replication in `p2mere` (log model, storage, processing, verification), peer in `murm` (carriers, one session layer, release-log carriage, conversations) and mesh in `retinue` (Reticulum, LXMF, radios, boards, device apps). Evidence, edges, why the fork stays its own repository, and ordering constraints. |
| [Transport consolidation plan](2026-10-28_transport_consolidation_plan.md) | Family transport boundary: endpoints, tickets, relays, carriers and live sessions are Murm's; Stickleback in Eidetic owns validated history and admission. T1 and T7 landed; Isonetry, Distillery remote, the relay fixture and remaining admitted-services work retain their gates. T6's Murm session lane landed 2026-11-01. |
| [Secure sessions and live interaction plan](2026-10-29_secure_sessions_and_live_interaction_plan.md) | Cut to open work 2026-10-31: N1–N3 (Noise and native iroh behind one setup gate) and O1–O3 (optional bounded CapTP runtime, Gemot/coop adapter, pinned interoperability subset) landed and are archived whole. Open: O4 handoffs, interoperability and carrier classes; native Reticulum profile, coop hosting and current-draft encoding deferred. |
| [Reticulum License adoption](2026-10-05_reticulum_license_adoption.md) | Ruling, 2026-10-05: retinue and outrider move to the Reticulum License (0.3.0) so RNS/LXMF source can be adapted into those two crates only; RNode firmware stays unread; firmware images are no longer GPLv3. Review and adaptation rules, and upstream versions checked. Amended 2026-10-09: tulle (0.2.0) and radio-hand join, for the RNode host semantics. |
| [RNS 1.6 / LXMF 1.3 protocol refresh](2026-11-17_reticulum_protocol_refresh.md) | Signed release identity, configurable destination/path/link stamps, exact fractional cost admission, stock qualification, and retained physical/public-network boundaries. |
| [RNS 1.5.7 comparative review](2026-10-07_rns_157_comparative_review.md) | First source review under the Reticulum License: Transport, Link, Resource, Identity/Packet/Channel against retinue. 39 ranked findings (11 P0 interop/safety, including the DATA-typed resource proof, unbounded bz2, wrong link-proof key, unauthenticated LINKCLOSE), provenance and lines read. P0 landed 2026-10-07 (live 17/17 on RNS 1.5.7); P1 and one-week route TTL landed 2026-10-08; P2 landed 2026-10-09. |
| [LXMF 1.2.0 comparative review](2026-10-09_lxmf_120_comparative_review.md) | Outrider against LXMF 1.2.0: message format, stamps and tickets; client delivery; the propagation node and peering. 11 interop breaks (A, including unproved inbound messages and the ignored fetch acknowledgement), 10 node and feature items (B), efficiency (C), and the larger parity work (Outbox, link reuse, peering). Includes the provenance record. A, B and C landed 2026-10-09 (outrider 18/18, live 30/30); L1-L3 remain. |
| [RNS 1.5.7 interface review](2026-10-09_rns_157_interface_review.md) | RNS `Interfaces/` against retinue: framing and IFAC sizes, modes and ingress control, TCP lifecycle, shared instance, UDP and AutoInterface, Serial/KISS and the RNode host. 11 interop gaps (A, including dropped full-size IFAC frames and the wrong default IFAC size), 13 parity items (B), C items, later work (HW_MTU autoconfiguration, serving as the shared instance), and the provenance record. Batches 1 and 2 landed (batch 2, with L1, integrated on `iv/integration` 2026-10-14); the status block lists what remains: L2-L5, signed TX power and a few scoped leftovers. |
| [Work lanes](2026-08-09_retinue_work_lanes.md) | Four-lane split and shared source lock. Lane split authoritative; ordering superseded. |
| [Program sequencing](2026-08-12_program_sequencing_and_deadline_order.md) | Historical August ordering audit. September coordination and current gate owners govern active slices. |
| [ARDC application lane](2026-07-31_ardc_application_lane.md) | Grant lane, G0–G4. Dormant by decision 2026-08-26: Sep 1 window passed, FIVCO-facilitated funding leads, next window Feb 1, 2027. |
| [Project prospectus](2026-08-26_project_prospectus.md) | Grant-agnostic funding prospectus compressed from the corpus; serves FIVCO-facilitated applications and any future window. Draft pending Mark's review. |
| [Prns donor ledger](2026-08-10_prns_donor_ledger.md) | September 26 disposition applied: fresh RNS fixtures, Hopspot excluded from catalogs/stages with archived evidence, host admission replaced; codec/tooling retained with attribution. Original provenance remains recorded. |
| [Prns mobile adoption brief](2026-08-11_prns_mobile_adoption_brief.md) | Historical recommendation withdrawn September 26; WN5 owns wall-node ordering. |

### Protocol core and wire compatibility

Current execution: [wire compatibility successor](2026-11-02_wire_compatibility_plan.md)
tracks embedded IFAC provisioning and physical acceptance, total decoder allocation,
stock/stress and exact-image acceptance, plus the optional link-ID evidence question.
The [original compatibility ledger](archive_docs/2026-11-02/2026-09-27_wire_compatibility_plan.md)
retains the repins and completed software/target evidence at their named scopes.

| Document | What it is |
| --- | --- |
| [I2P over SAMv3 plan](archive_docs/2026-11-02/2026-10-21_i2p_sam_seam_plan.md) | Retained I2P3/I2P4 reconciled, 2026-10-26. Restores session lookup, fresh-session redial, router termination reporting and harnesses omitted during the history refresh; current `truchman` 0.1.1 pins remain. [Current qualification](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/i2p-reconciliation-20261026/qualification.json): 14 focused tests, native provider checks on Mac/Windows, and a passing Windows stock-RNS no-IFAC dial/restart check. The [I2P3](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/i2p3-external-router/README.md) and [I2P4](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/i2p4-stock-rns/README.md) receipts retain their original cohort scope, including the one RNS-sender Resource failure in 25 runs. |
| [NomadNet resource interoperability receipts](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-13_nomadnet_go_resource_compression_receipt.md) | Go/Python compression receipts and Rust nomadnet-rs qualification, including failed client self-control and multipart limits. |
| [Retinue v0 plan](archive_docs/2026-11-02/2026-07-06_retinue_v0_plan.md) | Historical protocol ledger. R0–R10 all closed; optional flag-mask capture question continues in the wire-compatibility successor. |
| [RNS wire format reference](2026-07-13_rns_wire_format_reference.md) | Wire reference. Historical byte-fixture corpus is pinned at RNS 1.3.8; current announce and route probes use RNS 1.5.4. |
| [Announce timebase plan](archive_docs/2026-11-02/2026-08-25_announce_timebase_plan.md) | Active phased plan for the 5+5 announce blob, persistent stock-RNS decision probes, bounded receive freshness, and crash-monotonic firmware reservations. |
| [RNS 1.5.4 repin receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/rns-1.5.4-repin/README.md) | Superseded live oracle (see the 1.5.6 and 1.5.7 receipts): Retinue, LXMF 1.1.1, repeated Resources, timebase and route decisions. Separate host/core boundary repair tests; physical scope remains historical. |
| [RNS 1.5.6 repin receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/rns-1.5.6-repin/README.md) | Superseded live oracle, 2026-10-03, Windows host: every gate passed in at least one run; `interop_ifac` intermittent on an RNS-side race also present on 1.5.4; route decisions identical to 1.5.4; V1 re-checks. |
| [RNS 1.5.7 repin receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/rns-1.5.7-repin/README.md) | Previous live oracle, 2026-10-06, first macOS host (CPython 3.13): live 13/13, outrider 7/7, resource 12/12 and routing 3/3, each in a single run; routing and timebase identical to 1.5.6; `interop_ifac` 14/14 (host difference noted); V1 re-checks unchanged, both held RNS defects persist; harness needs Python 3.11+; preliminary 3.9 runs kept as evidence. |
| [RNS 1.5.4 compatibility lanes](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/rns-1.5.4-lanes/README.md) | Software receipts for topology-specific announce relay, decoded-output limits and stock Channel/Buffer traffic with compressed EOF. Firmware IFAC, total decoder memory and physical acceptance remain open. |
| [Sennet and Tucket current-reference work](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/sennet-tucket-current/README.md) | September 29 software repairs and September 30 identified stock-radio exchanges: MeshCore 1.17.1 endpoint/one-repeater paths at all three widths and Meshtastic 2.7.26 bidirectional text/rebroadcast/duplicates/node association. Original radio images restored; maximum-size stock acceptance remains unqualified after USB interruptions. |
| [Firmware IFAC software receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/firmware-ifac/README.md) | Shared carrier adapter, Node budgets, stock RNS 1.5.4 boundary captures and target checks. Provisioning and physical acceptance remain open. |
| [September 30 firmware refresh](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/firmware-refresh/README.md) | Initial clean-pin V4 installation, preserved settings, Windows ROM retry and selected RF traffic. Immutable initial receipt; see the follow-up for T114 installation and cancellation isolation. |
| [Firmware refresh follow-up](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/firmware-refresh/follow-up/README.md) | T114 native installation, original settings sequence, guarded admission, resource exchange and signed announces across soft resets. One isolated cancellation passed; a second RF proof timed out after Home return and LinkUp. Same-image T114 recovery retry and resident acceptance remain open. |
| [IFAC provisioning preflight](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/ifac-provisioning-preflight/README.md) | Board storage, framing and confidentiality boundaries; resident Debug redaction regression. Credential provisioning itself remains open. |
| [Re-pin receipt, RNS 1.5.2 / LXMF 1.1.1](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-29_rns_152_repin_receipt.md) | Historical pin. Preserves upstream delta, fixture boundary, Resource stress, Outrider, route-freshness and the withdrawn Prns peer matrix. |
| [RNS 1.5.4 signed-artifact receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/rns-1.5.4-signed-artifact/README.md) | Historical first 1.5.4 slice: six CLI captures match retained 1.5.2 bytes. Rust replay covers both versions. Broader qualification followed in the repin receipt; provenance uncertainty remains. |
| [Historical re-pin receipt, RNS 1.5.0 / LXMF 1.1.1](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-23_rns_150_lxmf_111_repin_receipt.md) | Superseded pin receipt. Records the earlier flake finding and one Outrider defect fixed. |
| [Permissive radio protocol compatibility survey](2026-08-25_permissive_radio_protocol_compatibility_survey.md) | Revision-pinned Reticulum, MeshChat, MeshCore and adjacent LoRa survey; separates donors, executable peers, radio adapters, bearers and semantic bridges. Opens no gates. |
| [Compact signed feed and local control plan](2026-08-25_compact_signed_feed_and_local_control_plan.md) | Exact allocation-free tinySSB core; foreign-source, local-control and radio gates. October 10 refresh scopes canonical device operations across LoRa/IP using the current host sync primitives; embedded execution remains unproven. 2026-10-31: `tinyssb-core` has no consumer; AT1 fulfilled by the secure-session plan's N1. |
| [Peer matrix receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-11_prns_peer_matrix_receipt.md) | H8 three-corner interop at RNS 1.4.2. |
| [Peer matrix at RNS 1.5.0](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-23_prns_peer_matrix_rns150_receipt.md) | Historical H8 re-receipt. Seven runs, 35/35. Establishes that capture byte counts are not constants. Current H8 evidence is in the RNS 1.5.2 re-pin receipt. |
| [Live-gate flake lane](2026-08-23_live_gate_flake_lane.md) | FLK1–FLK5. Per-gate rates, mechanisms, and what a suite run may prove. |
| [IFAC interop](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_ifac_interop.md) | R8 complete. |
| [Ratchet / stamp cost and roles](2026-08-07_stamp_cost_and_roles.md) | Stamp cost measurements from T114 receipts. |
| [RNode RX leading byte](2026-08-07_rnode_rx_leading_byte.md) | Leading byte from stock peer diagnosed; RF opacity question closed. |
| [RNode bulk frame loss](2026-07-26_rnode_bulk_frame_loss.md) | Stock RNode silently drops long frames — reproducible upstream defect. |
| [Receive future cancellation](2026-08-08_receive_future_cancellation.md) | Current V4 receive-phase implementation; physical and live-ownership proof remains open. |

### LXMF and Outrider

| Document | What it is |
| --- | --- |
| [Outrider / LXMF founding](2026-07-25_outrider_lxmf_founding.md) | Gates 1–8. Historical founding closure; seven local gates requalified at RNS 1.5.4 / LXMF 1.1.1 in the September 27 receipt. Older physical receipts retain their original pins. |
| [LXMF field registry capture](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-13_lxmf_field_registry_capture.md) | Field numbers confirmed by wire capture. |
| [Direct-PHY delivery](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_outrider_direct_phy_delivery.md) · [opportunistic direct-PHY](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_outrider_direct_phy_opportunistic.md) · [opportunistic delivery](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_outrider_opportunistic_delivery.md) · [large propagation response](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_outrider_large_propagation_response.md) · [propagation persistence](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_outrider_propagation_persistence.md) | Outrider acceptance receipts, all against LXMF 0.9.6 / RNS 1.4.2. |
| [Outrider publish blocker](2026-08-13_outrider_publish_blocker.md) | Resolved; both crates are on crates.io. |

### Radio, firmware, and on-device UI

| Document | What it is |
| --- | --- |
| [Retinue Small plan](archive_docs/2026-11-02/2026-07-31_retinue_small_plan.md) | Native-node authority. N0–N6; unplug leg and current figures open. |
| [Wall-node management plan](archive_docs/2026-11-02/2026-08-30_wall_node_management_plan.md) | WN0–WN8 authority for standalone V4, management, safe configuration and adapter control; October 11 iroh host probe passes 16 scenarios over existing signed commands. Device integration and physical gates remain separate. |
| [Magnetic pocket-node plan](2026-09-07_magnetic_pocket_node_plan.md) | PN0–PN8. Autonomous NFC-only and optional-Bluetooth nodes, LC0/AT0 attachment, durable custody, M4/iPhone probe, receiver-only charging, RF mechanics, and conditional phone-charging bridge. |
| [Hardware-family plan](2026-09-07_radio_hand_hardware_family_plan.md) | HW0–HW8 platform/product qualification; October 10 source assessment separates host composition, full iroh, LAN-direct noq and bounded radio-device profiles. All HW gates remain open. |
| [On-device UI plan](archive_docs/2026-11-02/2026-07-28_on_device_ui_implementation_plan.md) | Historical USB-first U0–U5 ledger; metered display power and deferred product-state work remain in the successor. |
| [radio-mirror plan](archive_docs/2026-11-02/2026-10-01_radio_mirror_plan.md) | M1: radio-face in the browser (mer3ly S8). Landed on `main` in `fa4f925` (2026-10-02); forks F1–F8 ruled by site Rulings 31–38. wasm-bindgen `=0.2.129` under site Ruling 126 (2026-10-07, branch `radio-consumers`), superseding Ruling 35's `=0.2.127`. |
| [On-device UI direction](2026-07-25_on_device_ui.md) | PANEL×LEDGER face, accepted direction. |
| [Mesh household](2026-07-20_mesh_household_tulle_tucket_sennet.md) | Crate topology and naming authority. **Its "no code exists yet" status line is false** — all three crates exist. |
| [Heltec RNode and embedded Rust](2026-07-19_heltec_rnode_and_embedded_rust.md) | Donor, licensing and architecture record. |
| [Modem/embedded/Meshtastic research](2026-07-19_modem_embedded_and_meshtastic_research.md) | Research record, partly superseded. |
| [Low-power UART personality](2026-07-24_low_power_uart_personality.md) | V1 and V2 implemented, hardware-proved through RF wake. |
| [LoRa collision mitigation](2026-07-24_lora_collision_mitigation_ideas.md) | Techniques documented, not scheduled. |
| [First reliable link over RF](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-21_first_reliable_link_over_rf.md) · [Tulle headed acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-22_tulle_headed_acceptance.md) · [MeshCore relay acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-22_meshcore_relay_headed.md) · [direct-PHY resource acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-23_direct_phy_resource_acceptance.md) · [host snapshot acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_host_snapshot_acceptance.md) · [host projection acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-29_retinue_host_projection_acceptance.md) | On-air and host milestones, July 2026. |
| [T114 UI acceptance](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-28_t114_on_device_ui_acceptance.md) · [V4 UI acceptance](2026-07-28_v4_on_device_ui_acceptance.md) · [display power and field behaviour](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-29_display_power_field_acceptance.md) · [V4 light-sleep and RF wake](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-07-29_v4_light_sleep_rf_wake_acceptance.md) | Board-level UI and power receipts. |
| [AIR0/AIR1](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air0_air1_software_receipt.md) · [AIR2](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air2_announce_ingress_receipt.md) · [AIR3 software](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_air3_bounded_transport_software_receipt.md) · [AIR3 T114 on-air](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-13_air3_t114_on_air_receipt.md) | Air-lane receipts. The T114 receipt also records the CDC endpoint defect. |
| [Transit link receipt spec](2026-08-14_transit_link_receipt_spec.md) | Spec for the deferred three-party transit receipt. |

### Scaling, executive, and security

| Document | What it is |
| --- | --- |
| [Mesh scaling and asymmetric routing](2026-08-09_mesh_scaling_and_asymmetric_routing.md) | FT1–FT5 authority. |
| [Listener executive and protocol leases](2026-08-10_listener_executive_and_protocol_leases.md) | LE1–LE5 interruption/lease work and receipts; product framing superseded by the 2026-09-10 murmuration direction. |
| [Channel murmuration](2026-08-09_channel_murmuration.md) | Current 2026-09-10 Tulle coordination direction: configurable home, excursions, pinning, independent third-party firmware; controller and MC3a physical receipts. Historical CM1–CM5 retained. |
| [LE3 T114 scan-physics receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-20_le3_t114_scan_physics_receipt.md) | LE3a/LE3b complete. |
| [Radio observation plan](archive_docs/2026-11-02/2026-09-06_radio_observation_plan.md) | O1–O6. Shared bounded owner events, RAM recording, collection without control-journal writes, availability timeline and export. |
| [Field node security posture](2026-08-09_field_node_security_posture.md) | FS1–FS6 authority. Also owns the PD0 disclosure-tier ruling. |
| [Position disclosure](2026-09-01_position_disclosure_plan.md) | PD0–PD6. PD0 is ruled; PD1/PD2 are software-complete; PD3's V4 UART/parser path is physically receipted but satellite fix remains unmeasured. Rides FS2's envelope as config authority. |
| [Assurance lane status](2026-08-10_assurance_lane_status.md) | ASSURE1–ASSURE5. |
| [FS2 command carrier decision](2026-08-10_fs2_command_carrier_decision.md) · [FS4 custody and FS5 seizure](2026-08-10_fs4_custody_and_fs5_seizure.md) | Assurance decisions and receipts. |

### Distribution, installer, and catalog

| Document | What it is |
| --- | --- |
| [Linkboy public flashing plan](archive_docs/2026-11-02/2026-08-08_linkboy_public_flashing_plan.md) | F0–F7 trunk gates plus S1–S7 sidequests (**not gates**). |
| [Linkboy F5 spike](2026-08-08_linkboy_public_flashing_f5_spike.md) | Helper-packaging evidence, complete. |
| [Catalog auth and activation foundation](2026-08-20_catalog_auth_and_activation_foundation.md) | Host-side foundation for authenticated firmware delivery. DIST7. |
| [FCC reselling research](2026-07-20_fcc_reselling_flashed_radios.md) | Findings and v1 posture for flash-and-resell lawfulness. |
| [Meshnology N39 V4.2 profile](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_meshnology_n39_v4_2_profile_evidence.md) · [N39 pre-write probe](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_com6_n39_v4_2_prewrite_probe.md) | Product-profile evidence and ESP ROM plan. |
| Physical flash receipts | [F5 macOS/Linux](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-19_linkboy_f5_macos_linux_v4_receipt.md) · [F5 Windows custody](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_linkboy_f5_windows_custody_receipt.md) · [F4 V4 state preservation](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-20_linkboy_f4_v4_state_preservation_receipt.md) · [T114 raw restore](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_t114_retinue_raw_restore_receipt.md) · [T114 package restore](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_t114_retinue_linkboy_package_restore_receipt.md) · [Hopspot V4 COM7 interface](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_hopspot_v4_com7_interface_receipt.md) |
| JSON receipts | Machine-readable records, schema 1–4. Indexed in full below. |

### JSON receipts

Machine-readable evidence. Most records are written by Linkboy and Signalman;
the V4 GNSS RX fix is an explicitly hand-authored ledger joining its Linkboy
transfer receipt to the post-flash probes. `schema` distinguishes generations;
loader snapshots are board-state captures rather than transfer outcomes.

**Two of these record non-success and are easy to miss**, because nothing in
their filenames says so — see the `result` column.

| Receipt | `result` / content |
| --- | --- |
| [V4 COM6](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_linkboy_v4_com6_receipt.json) | schema 2 — Retinue V4 transfer, application verified. |
| [V4 COM7](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_linkboy_v4_com7_receipt.json) | schema 2 — Retinue V4 transfer, application verified. |
| [V4 COM7 helper custody](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_linkboy_v4_com7_helper_custody_receipt.json) | schema 3 — espflash 4.5.0 custody verified. |
| [V4 COM7 Hopspot restore](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_linkboy_v4_com7_hopspot_restore_receipt.json) | schema 3 — full restoration from Hopspot. |
| [Hopspot V4 COM7 transfer](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_hopspot_v4_com7_transfer_receipt.json) | schema 3 — bootloader partition and application. |
| [Hopspot V4 COM7 demo](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-11_hopspot_v4_com7_demo_receipt.json) | schema 3 — demo transfer parts. |
| [T114 transfer](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_meshtastic_t114_linkboy_transfer_receipt.json) | schema 3 — bootloader and application. |
| [T114 recovery](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_meshtastic_t114_linkboy_transfer_recovery_receipt.json) | schema 3 — recovery verification. |
| [T114 replay](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_meshtastic_t114_linkboy_transfer_replay_receipt.json) | schema 3 — **`manual-check-required`.** Not a pass. |
| [T114 retry recovery](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_meshtastic_t114_linkboy_transfer_retry_recovery_receipt.json) | schema 3 — **`recovery-required`.** Not a pass. |
| [T114 restore completed](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_t114_retinue_packaged_restore_completed_receipt.json) | schema 3 — package and loader facts. |
| [T114 restore replay](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_t114_retinue_packaged_restore_replay_receipt.json) | schema 3 — retained post-write recovery. |
| [T114 restore verified](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_t114_retinue_packaged_restore_verified_receipt.json) | schema 3 — terminal receipt for the restore. |
| [F5 Windows V4 Retinue](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-19_linkboy_f5_windows_v4_retinue_receipt.json) | schema 4 — install with settings preservation. |
| [F5 Windows V4 Hopspot](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-19_linkboy_f5_windows_v4_hopspot_receipt.json) | schema 4 — package transfer and verification. |
| [F4 V4 state preservation](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-20_linkboy_f4_v4_state_preservation_receipt.json) | schema 4 — settings preserved across flashing. |
| [V4 GNSS RX fix](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-09-03_v4_gnss_rx_fix_receipt.json) | schema 1 — corrected UART/parser path and preserved state; satellite fix unmeasured. |
| [T114 loader snapshot](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-12_t114_loader_snapshot.json) | schema 1 — HT-n5262, nRF52840, 1 MB flash, SoftDevice S140. |
| [T114 AIR3 loader snapshot](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-13_t114_air3_loader_snapshot.json) | schema 1 — persisted state captured for AIR3. |
| [Signalman T114 loader snapshot](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_signalman_t114_loader_snapshot.json) | schema 1 — UF2 bootloader 0.9.0. |

### Signalman

| Document | What it is |
| --- | --- |
| [Signalman founding](2026-08-06_signalman_founding.md) | Historical founding note; execution authority moved to the Cambium scope. |
| [Cambium desktop scope](2026-08-09_signalman_cambium_desktop_scope.md) | Desktop G0–G5 authority. G0–G4 complete, G5 partial. |
| [G2 receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-09_signalman_desktop_g2_receipt.md) | Evidence, not a queue. **Its accessibility count is wrong** — see Known divergences. |
| [Management surface direction](2026-08-15_signalman_management_surface_direction.md) | Ruling on the device-data mere. |
| [Management surface plan](archive_docs/2026-11-02/2026-08-15_signalman_management_surface_implementation_plan.md) | S0–S9 authority; also records Signalman's headless mixed-realization consumer receipt without changing S2's headed gate. |
| [Survey and placement plan](2026-09-06_signalman_survey_and_placement_plan.md) | SP0–SP5. GPX/capture imports, directional evidence, candidate scenarios, terrain estimates, constrained placement and measured comparison. |
| [S2 live station receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-20_signalman_s2_live_station_receipt.md) | Live bench leg complete, over-the-air announce. |
| Graphical receipts | [V4 COM6 G4](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_signalman_v4_com6_g4_receipt.md) · [V4 COM7 owner flow](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-10_signalman_v4_com7_owner_flow_receipt.md) · [T114 graphical](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_signalman_t114_graphical_receipt.md) · [COM6 N39](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_com6_n39_signalman_graphical_receipt.md) · [N39 Hopspot](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_com6_n39_hopspot_signalman_graphical_receipt.md) · [N39 Hopspot→Retinue restore](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-14_com6_n39_hopspot_retinue_signalman_restore_receipt.md) · [Windows staged helper](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-15_signalman_windows_v4_staged_helper_receipt.md) · [public F5 Windows](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-19_signalman_public_f5_windows_receipt.md) |

### Adjacent programmes and pre-decision material

| Document | What it is |
| --- | --- |
| [Smolweb over Reticulum](2026-08-04_smolweb_over_reticulum_plan.md) | Independent application bridge. Not started; R-A/R-B/R-C serial. |
| [Civic deployment](2026-08-11_civic_deployment_prescribed_paths.md) | CV1–CV7, D1–D5. Phase two, post-deadline. |
| [IoT device concepts](2026-08-13_iot_device_concepts.md) | Application-concept brainstorm; its ARDC-first hardware sequencing is superseded by the hardware-family plan. |
| [Lofi voice codec scoping](2026-08-13_lofi_voice_codec_scoping.md) · [Rung 2 codec2 decision](2026-08-13_rung2_codec2_class_decision.md) | Pipit founding and codec choice. |
| [Hopspot demo receipts](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-11_hopspot_v4_com7_demo_receipt.md) · [Meshtastic T114 UF2 demo](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/design_docs/2026-08-11_meshtastic_t114_uf2_demo_receipt.md) | Phone-app BLE demos. **Not Retinue capability claims.** |

### Archived checkpoint: 2026-11-02

- [Retinue simulation](archive_docs/2026-11-02/2026-10-01_retinue_sim_plan.md): Completed route-trace and face mapping; historical alternatives stand as built.
- [Node link lifecycle](archive_docs/2026-11-02/2026-10-02_node_link_lifecycle_plan.md): Completed 0.2.0 lifecycle, publication and downstream adoption.
- [Signalman stack and custody refresh](archive_docs/2026-11-02/2026-10-11_signalman_stack_refresh_plan.md): Recorded Windows software and custody qualification; headed and physical gates remain with their owners.
- [Resource client receive limits](archive_docs/2026-11-02/2026-10-22_resource_client_limits_plan.md): Completed bounded native receiving and Turnstone adoption; broader acceptance remains separate.
- [RNode RF opacity finding](archive_docs/2026-11-02/2026-07-25_rnode_direct_phy_rf_opacity.md): Superseded by the verified crossing and CRC finding.
- [T114 bulk TX probe](archive_docs/2026-11-02/2026-07-25_t114_bulk_tx_asymmetry_probe.md): Closed by the RNode bulk-frame-loss follow-up.
- [Prns harvest brief](archive_docs/2026-11-02/2026-08-09_prns_harvest_brief.md): Historical findings; donor/reference role withdrawn 2026-09-26.

### Archived checkpoint: 2026-11-03

- [Receipt relocation](archive_docs/2026-11-03/2026-11-03_receipt_relocation_plan.md): Completed relocation and fixture/campaign path repairs. Software verification and remaining registry debt are indexed by the [external receipt pointer](archive_docs/receipts/2026-11-03_receipt_relocation_plan_receipts.md). Checkpoint stops before Woodshed.

### Retained run evidence and executable inputs

Standalone run records and captured source snapshots are stored under
`Code/testing/murm/` at their original repository-relative paths. The
[published pre-relocation tree](https://github.com/merely-made/retinue/tree/b0e31de8b581940dcd347968585c401e3ad786ca)
provides immutable public locators for historical citations. Existing archive
bodies and recorded provenance retain their original text.

Executable inputs live in their owning `tests/fixtures/` directories;
reusable campaign tools live under `testing/campaigns/`. New local captures go
to ignored output or an explicitly selected external directory. A moved
historical capture is separate from a new qualification run.

## Working principles

These are not general good practice. Each is here because its absence cost this
repository something specific and recent.

### Stop at an external blocker

The owner explicitly requires stopping rather than churning when access,
credentials, a compatible device peer or a resource handoff is unavailable.
Report the concrete blocker and preserve completed evidence; do not keep
polling, retrying or expanding preparation while waiting. Resume when the owner
provides the missing condition or explicitly asks to continue.

### Family placement preserves implementation contracts

The [Murm move plan](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md) promotes the family,
not a mandatory aggregate dependency. Preserve published crate names and
original history, radio dependency/target boundaries and protocol-specific
semantics. The owner requires a shared current Rust toolchain (1.99.0 for the planned
rollout) and a separately selected shared MSRV of 1.97.1 across the stack.
The MSRV does not automatically follow the working toolchain; compiler
qualification and manifest/CI rollout remain pending.
Host persistence, identity roots, governance and browser policy keep their
named owners when an implementation changes repository.

Shared wire implementations stay below both Genet and application transports.
Browser APIs and WPT remain Genet's; invitations, application framing and
admission remain above the raw connection. Repository co-location does not
make the native peer facade a prerequisite for a generic protocol provider.

### Live protocols reuse Murm's endpoint authority

The [secure-session plan](2026-10-29_secure_sessions_and_live_interaction_plan.md)
puts Noise and native iroh security behind one setup gate,
`transport::security`, and runs optional OCapN on the existing endpoint. A
service that admits peers sets up through that gate. Keep carrier facts,
secure-channel facts and application authority distinct; live capabilities
invoke the owning domain's durable-operation path.

### Murmuration coordinates our implementations

Follow the [current murmuration direction](2026-08-09_channel_murmuration.md#current-direction-coordinated-runtime-personalities).
Tulle coordinates admitted personalities; radio-hand owns embedded interruption.
Home, excursions, pinning and uncovered intervals are explicit user policy.
Unexpected packets do not authorize switching, and third-party firmware does
not owe our coordinator a runtime integration.

### A receipt states what it established, and nothing more

Between 2026-08-23 and 2026-08-24, six documents were found asserting past their
own evidence — four receipts and two status headers. This is the failure mode
this project is most prone to, because it is receipt-driven and a receipt is
trusted by default.

- **A bare gate count is not evidence.** The live oracle gates flake at a rate
  this repository has not bounded, so "twelve of twelve" without a rate beside
  it says nothing. See the [flake lane](2026-08-23_live_gate_flake_lane.md).
- **Deterministic fixtures are not live gates.** A fixture re-capture cannot
  flake, so "eighteen of eighteen" means what it says. Do not let a fixture
  count lend credibility to a gate count sitting beside it.
- **Captured bytes are not constants.** HDLC byte-stuffing over freshly minted
  ephemeral identities changes the raw length of every capture, every run.
  Unstuff before comparing; the unstuffed length is the invariant.
- **Stateful protocol fixtures must retain state.** Reconstructing a sender with
  the same identity can reset a monotonic ordinal or replay ledger. Repeats and
  retries use one stateful producer or explicit ordered inputs, and the fixture
  proves that semantic precondition before testing a later gate.
- **A done-condition must establish what it is read as establishing.** "Each
  consumer names the dependency in its manifest" cannot establish that any
  consumer uses it.
- **Check the header against the body before believing either.** Every
  contradiction found on 2026-08-24 was a status line disagreeing with the
  document beneath it — the shape most likely to be believed, because a reader
  stops there.

### Verify against the tree, not against another document

Doc-to-doc agreement is not verification; two documents agreeing is the normal
way a wrong belief survives. Check paths, revisions and counts against the
working tree and the git history. Several ledger claims here were false for
weeks while reading perfectly consistent — and rows of the Plan audit table were
re-edited *after* the evidence that undercut them existed, so recency is not
reliability either.

### Supersede; do not rewrite

A receipt records the pin, version and revision that produced it. When it dates,
write a new receipt and annotate the old one. The 2026-08-14 receipt was
annotated rather than corrected; the `rnid` and `PROVENANCE` capture records
deliberately stay at RNS 1.4.2 because that is what produced them.

Distinguish **wrong when written** from **aged out**. The Cambium scope's "no
pin moves it" was true on 2026-08-23 and false on 2026-08-24 because the fix was
authored in between. Both need correcting; only one was ever an error.

### Pins are deliberate, and the friction is the point

A pin bump is a receipted event, not a chore. Bumping is one command; what costs
is deciding which revision and re-establishing the evidence. A floating pin
hides that cost rather than removing it, until something fails to compile.

**When replaying a fix for a consumer that is behind, choose the fix's parent
from the consumer's pin, not from wherever the push has to land.** A ref used
for pinning need not be on `main`. On 2026-08-24 that distinction was the
difference between an 80-line bump and a 24,686-line absorption with a coupled
dependency move.

### This tree is written by many sessions at once

- Prefer `git show HEAD:<path>` over `git diff` when checking state. Git's stat
  cache returns a stale empty diff while another process is mid-write, and that
  has produced wrong conclusions more than once.
- The whole-tree commit convention is safe only when the dirty files are yours.
  Never fold another session's uncommitted work into a commit without asking.
- `validation/results/` is gitignored deliberately: transient identities, ports
  and raw captures.

### Command authority and control meaning are separate

`retinue::command` owns the signed outer statement: target, controller key,
replay counter, opcode, bounded payload, and signature. `radio-hand::control`
owns the bounded semantic payload and capability vocabulary. Do not copy outer
authority fields into the inner request, and never accept a role claimed by the
controller; derive it from the board-local grant for the verified key.

Exact command-byte replay and semantic retry are different facts. The former is
refused by the monotonic outer counter. A retry uses the same transaction and
request under a fresh counter, then receives the cached prior result from the
durable control journal. A volatile duplicate detector is not that journal.

A signature is not encryption. Secret-bearing control operations require a
confidential carrier session or a separately sealed payload bound to the signed
request. Never infer confidentiality from a valid command signature.

Durable control state does not belong in the extensible identity/settings body.
It uses a separate fail-closed A/B pair because grants, generation high-water,
rollback state, and cached results have different corruption and downgrade
semantics. A nonblank corrupt control pair is recovery state, not a blank board.

Semantic retry tags are keyed over the canonical verified request. Storing raw
arguments or an unkeyed credential digest creates a secret leak or an offline
guessing oracle. Cache eviction must refuse older transactions instead of making
their side effects executable again.

Durable grants bind the controller id to its SHA-256 public identity. An
unparsable identity is a boot refusal before any apply, and raw grant creation
stays internal to the durable authority. `Blank` says only that the A/B facts
are erased, never that a caller may commission the board. Once the outer
verifier accepts a counter, persist it even when later semantic checks refuse a
malformed, wrong-opcode, or non-node command. An Observer is authenticated but
has no mutation authority.

### Durable control transitions have separate meanings

`Changed` means the durable state machine performed the requested transition.
`Replayed` means it returned the one retained terminal result for the same
semantic transaction under a fresh outer counter; it must not apply the work
again. Treating both as generic success loses the receipt needed to diagnose a
controller retry.

Retiring a cached result advances a durable monotonic mutation-sequence
watermark. A transaction at or below that watermark is refused rather than
becoming executable again after eviction or reboot. The watermark is part of
the journal's safety boundary, not an in-memory cache hint.

`Ready` means the durable journal belongs to the expected node and its applied
state is the known-good state. A mismatched node or an unapplied/provisional
state is recovery or poison handling, not a routine ready boot. Poison requires
discarding and rebuilding the verifier from durable grants and counters.

### Live durable work requires a board quiet witness

The portable `QuietWindow` must enter at a radio frame boundary, disarm live
RX/TX/IRQ work, and lend a `QuietGuard` through every live erase, program,
readback, and configuration apply. After entry, `ActiveQuietGuard` aborts on a
dropped operation or finish. If an entry future is dropped before it returns a
guard, the board-owned `QuietWindow` implementation must synchronously leave
radio work stopped or latch reset, because the runtime has no guard yet. Boot is
pre-radio and does not need that witness. `ResetRequired` blocks later work on
the runtime instance; its unsafe replacement constructor is reserved for the
board startup owner once after an actual hardware reset. The host model covers
guard ordering and adversarial entry cancellation, not a real board witness.

A pre-entry busy refusal is retryable because stopping has not begun. Once the
board stops the radio, it must clear latched chip IRQ flags and prove the
physical IRQ line low before lending flash or configuration access; any later
cancellation, uncertain hardware apply, or failed resume resets rather than
claiming that the live owner returned to service.

WN0's carrier-agnostic semantic payload is carried by compatible transports;
the authenticated `VerifiedCommand` conversion is a handoff. Reserve a bridge
or semantic gateway for terminating one protocol and re-originating another.
A foreign-mesh bearer carries an opaque Reticulum frame instead.

### Portable attachment is not runtime ownership

A phone-attached node remains autonomous. NFC and BLE are temporary Signalman
carriers for bounded, resumable control and synchronization; phone presence or
carrier lifetime never owns Retinue routing. Magnetic attachment, wireless
power, NFC, and Bluetooth are separately measured capabilities. A charging
interval with RF or thermal derating is reported explicitly rather than folded
into ordinary availability.

### Hardware support is a measured capability set

Host-library composition and MCU execution are separate evidence. Select a
bounded device role and reuse existing transport, replication, control, and
custody contracts; measure its complete RAM/flash/runtime/power profile.
Transport authentication does not replace board-local authority, and gateway
forwarding preserves the original signed object's identity and authorship.

A board name, chip data sheet, compile target, or populated connector does not
establish `radio-hand` support. The manifest exposes only the clock, durable
store, recovery, power, radio-port, collision-domain, and bearer capabilities
earned by that exact hardware and firmware profile. The board owner reports
physical power and radio transitions; policy code may request or refuse work
but cannot claim that a rail, antenna path, or receiver changed state.

### Build notes

For the 2026-09-06 radio lanes, a scheduler decision is not a hardware
transition. Only the board owner may report stopped or restored radio work;
observation gaps remain unknown and routine observation reads must not journal
control counters. The [coordination agreement](2026-08-09_retinue_work_lanes.md#shared-decisions-settled-before-implementation)
is the single source for the shared rules and active file ownership.

- `apps/signalman-desktop` roots its own workspace and is deliberately excluded
  from the root one. Build it with `-j 1`; concurrent builds exhaust the
  pagefile and surface as roughly thirty spurious internal compiler errors.
- Prefer `cargo test` and `cargo build --all-targets` over `--workspace`. Two
  sessions independently reported that `--workspace` dies on a
  `critical-section` feature-unification clash between firmware and host crates.
  *Reported, not verified here.*
- The oracle's live gates need its virtualenv at `crates/retinue/oracle/.venv`,
  pinned by `requirements.txt` (`rns==1.5.4`, `lxmf==1.1.1`).
- **The Prns peer daemon must be built from inside its own worktree.** Cargo
  resolves `.cargo/config.toml` from the working directory, and Prns pins a
  256 MiB Windows stack there. Built from anywhere else the daemon overflows a
  1 MiB stack before it can parse an argument, and the build reports success.


### Mixed-realization receipts

- Count custom paint and retained controls as one scene only when a product
  receipt starts from one projection and proves one focus order, action state,
  pointer-capture route, AccessKit tree, and automation surface. Separate paint
  and input component tests do not prove that composition.

## Status

Founded 2026-08-24 to meet `DOC_POLICY.md` core §6, which this repository did
not satisfy: 93 markdown documents and 19 JSON receipts were unindexed, and the
only index-shaped artefact was a table inside a dated lane document.

Not yet done: `PROJECT_DESCRIPTION.md` (core §7, maintainer-reserved) and the
FLK5 audit of the remaining documents that quote bare gate counts. The first
`archive_docs/` checkpoint was created 2026-08-30 when the pre-decision
Bluetooth capability brief was superseded by the wall-node plan.

The [foundation supplier promotion (2026-10-19)](archive_docs/2026-11-02/2026-10-13_murm_family_move_plan.md#2026-10-19-foundation-supplier-promotion) records the canonical Eidetic dependency cutover, native library tests and the separately open killed-peer gate.

Foundation source adoption (2026-10-19): canonical promoted-family revisions and consumer source validation are recorded in `mere/design_docs/mere_docs/implementation_strategy/2026-10-19_family_promotion_plan.md`. Existing application and domain authority remain with their owning components.

Catalogue discovery carrier (2026-11-04): the [transport consolidation plan](2026-10-28_transport_consolidation_plan.md#catalogue-advertisement-carrier-2026-11-04) records bounded opaque advertisements in `murm-sync`. Gemot owns catalogue semantics and authority; Turnstone owns disclosure and app UI.
