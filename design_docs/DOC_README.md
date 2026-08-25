# Retinue Documentation Index

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
   — ordering authority since 2026-08-12, built from a code-reading audit that
   found roughly twenty-five of the work-lane document's asserted dependencies
   unsupported. The work-lane document's own Plan audit table has no row for
   this one; read it anyway.

## Read this before citing any gate

**Three different `S`/`G` namespaces exist and they are not related.** Citing a
gate without its family is ambiguous and has already produced wrong readings.

| Namespace | Family | Meaning |
| --- | --- | --- |
| `G0`–`G4` **(ARDC)** | [ARDC lane](2026-07-31_ardc_application_lane.md) | Grant application gates. All unstarted. |
| `G0`–`G5` **(Signalman desktop)** | [Cambium scope](2026-08-09_signalman_cambium_desktop_scope.md) | Desktop GUI adoption gates. G0–G4 complete, G5 partial. |
| `S0`–`S9` **(Signalman management)** | [Management surface plan](2026-08-15_signalman_management_surface_implementation_plan.md) | Real gates. |
| `S1`–`S7` **(Linkboy sidequests)** | [Public flashing plan](2026-08-08_linkboy_public_flashing_plan.md) | **Not gates.** Explicitly labelled future-work sidequests, separate from that document's F0–F7 trunk gates. |

## Gate index

Status is what a document explicitly *states*, not what a receipt's existence
implies. Several receipts in this repository say outright that they do not close
their gate. `UNKNOWN` means no document states a status — it does not mean open.

### Peer lane — mixed-runtime interop

| Gate | Status | Where |
| --- | --- | --- |
| H8 | **Closed**, local-TCP scope only | [2026-08-11 receipt](2026-08-11_prns_peer_matrix_receipt.md), re-receipted at RNS 1.5.0 in [2026-08-23](2026-08-23_prns_peer_matrix_rns150_receipt.md) |
| H1–H7, H9, H10 | UNKNOWN — catalogue entries, no status stated | [Prns harvest brief](2026-08-09_prns_harvest_brief.md) |
| O-10 | **Partial** — local TCP resolved; RF forwarding receipt still owed | Both peer-matrix receipts |

### Air lane — radio, firmware, scaling

| Gate | Status | Where |
| --- | --- | --- |
| AIR0, AIR1, AIR2 | Closed | [Work lanes](2026-08-09_retinue_work_lanes.md), [AIR0/AIR1](2026-08-12_air0_air1_software_receipt.md), [AIR2](2026-08-12_air2_announce_ingress_receipt.md) |
| AIR3 | Closed, software and on-air | [software](2026-08-12_air3_bounded_transport_software_receipt.md), [T114 on-air](2026-08-13_air3_t114_on_air_receipt.md) |
| AIR4, AIR5 | Open | [Work lanes](2026-08-09_retinue_work_lanes.md) |
| FT1 | Open — needs modelled-versus-measured on-air airtime | [Mesh scaling](2026-08-09_mesh_scaling_and_asymmetric_routing.md) |
| FT2 | **Partial, and probably closeable.** [AIR3 software receipt](2026-08-12_air3_bounded_transport_software_receipt.md) names three facts needed; the [T114 on-air receipt](2026-08-13_air3_t114_on_air_receipt.md) reports all three — but no document ever says FT2 is closed. |
| FT3, FT4, FT5 | Open | [Mesh scaling](2026-08-09_mesh_scaling_and_asymmetric_routing.md) |
| LE1, LE2, LE4, LE5 | Open, post-deadline | [Listener executive](2026-08-10_listener_executive_and_protocol_leases.md) |
| LE3 | Partial — LE3a/LE3b complete; unattended scheduler open | [LE3 scan-physics receipt](2026-08-20_le3_t114_scan_physics_receipt.md) |
| CM1 | Absorbed into LE2 | [Channel murmuration](2026-08-09_channel_murmuration.md) |
| CM2–CM5 | Open, sequenced after LE5 | Same, translated into the executive doc |
| R0–R10 | All closed | [Retinue v0 plan](2026-07-06_retinue_v0_plan.md) |
| N0 | Partial — power-loss unplug leg open | [Retinue Small plan](2026-07-31_retinue_small_plan.md) |
| N1–N4, N6 | Closed | Same |
| N5 | Partial — needs current figures, meter required | Same |
| U0–U5 | All closed | [On-device UI plan](2026-07-28_on_device_ui_implementation_plan.md) |

### Assurance lane — validation, security, custody

| Gate | Status | Where |
| --- | --- | --- |
| ASSURE1 | Partial — "substantially closed" | [Assurance lane status](2026-08-10_assurance_lane_status.md) |
| ASSURE2 | Partial — fuzz CI has never run green once | Same |
| ASSURE3, ASSURE4 | Closed | Same |
| ASSURE5 | Closed **in software**; physical FS4 separate | Same |
| ASSURE6 | Open — first green Linux fuzz result | [Work lanes](2026-08-09_retinue_work_lanes.md) |
| FS1 | UNKNOWN — defined, never statused | [Field node security posture](2026-08-09_field_node_security_posture.md) |
| FS2 | Closed in software | [FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md) |
| FS3 | Open — replay counter, named "the next gate" | Same |
| FS4 | Partial — process half written, physical half not run | [FS4/FS5](2026-08-10_fs4_custody_and_fs5_seizure.md) |
| FS5 | Closed with an enforced check | Same |
| FS6 | Partial — same never-closed nuance as FT2 | [AIR3 software receipt](2026-08-12_air3_bounded_transport_software_receipt.md) |
| **LOCK4** | **Open, unstarted.** The private disclosure record. `design_docs/private/` does not exist. Named only in the sequencing doc; it is source-lock item 4 in [work lanes](2026-08-09_retinue_work_lanes.md). |

### Distribution lane — installer, catalog, activation

| Gate | Status | Where |
| --- | --- | --- |
| F0, F1, F2 | Closed | [Public flashing plan](2026-08-08_linkboy_public_flashing_plan.md) |
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
| S0, S1 (management) | Closed | [Management surface plan](2026-08-15_signalman_management_surface_implementation_plan.md) |
| S2 | Partial — headed interactive judgement is owner-driven | Same |
| S3 | Closed | Same |
| S4 | Partial — process boundary closed, not a headed serial-radio receipt | Same |
| S5 | Partial — file-backed and host-audio rungs done; headed two-site audible run remains | Same |
| S6–S9 | UNKNOWN — defined, never statused | Same |

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
| **G0–G4 (ARDC)** | **All unstarted. Intake closes 2026-09-01.** G0 is still an unsent draft email; G1 needs an external fiscal sponsor with lead time. | [ARDC lane](2026-07-31_ardc_application_lane.md) |
| CV1–CV7 | Open, pre-decision, post-deadline | [Civic deployment](2026-08-11_civic_deployment_prescribed_paths.md) |
| D1, D2, D3 | Ruled 2026-08-12 (decisions, not implementations) | Same |
| D4 | Partial — ruled, still needs a spike receipt | Same |
| D5 | Dissolved; CV7 carries the primitive | Same |
| LB1–LB6 | Open — not active gates until the stack ruling is accepted | [Bluetooth scoping](2026-08-11_bluetooth_capability_scoping.md) |

## Known divergences

Live as of 2026-08-24. Each is a place where a document, or a shipped artefact,
disagrees with the tree.

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
- **The [G2 receipt](2026-08-09_signalman_desktop_g2_receipt.md)** records
  `tests/accessibility.rs, 4 cases, passing`. There are five cases, and as of
  `bd71ee1` all five pass. Its supersession note covers the pin revision only,
  not this count.
- **The root [`README.md`](../README.md)** still says the T114 image's on-metal
  RF receipt is open; AIR3's on-air receipt landed 2026-08-13.
- **The [sequencing doc](2026-08-12_program_sequencing_and_deadline_order.md)**
  still describes a dirty-tree crisis — "79 dirty entries, 5 unpushed commits,
  25 untracked" — as live.
- **The [flake lane](2026-08-23_live_gate_flake_lane.md)** cites
  `scratchpad/flake_census.py`; the tool was committed to
  `crates/retinue/oracle/flake_census.py`.

## Terminology

Names in this program are invented and cannot be inferred.

**This project's crates.** `retinue` — endpoint-scoped Rust implementation of
the Reticulum mesh protocol, wire-compatible with RNS. `outrider` — LXMF
boundary crate: message codec, delivery state machines, propagation
client/server. `postilion` — shared radio-host library; a Station wraps one
identity, one board, an announce cadence and a peer table behind an event
stream. `selvage` — clean-room LoRa PHY profiles shared by host and firmware.
`sennet` — clean-room Meshtastic-compatible mesh protocol. `tucket` — MeshCore
interop. `tulle` — the shared radio interface layer beneath retinue, sennet and
tucket. `radio-face` — firmware crate rendering bounded on-device UI status.
`radio-hand` — board-agnostic firmware crate that works the radio.

**This project's applications.** `linkboy` — CLI that identifies firmware by
asking rather than trusting USB IDs, then verifies and flashes packages.
`signalman` — terminal application running a household radio over one serial
port. `signalman-desktop` — desktop GUI on the Cambium/Genet toolkit; roots its
own workspace, deliberately outside the root one.

**External.** `hopspot` — Prns's Wi-Fi-hotspot firmware for Heltec V4; Linkboy
can flash it but claims no Retinue capability. `cambium` — Genet-native GUI
toolkit. `genet` — sibling engine repository providing Cambium's desktop host.
`prns` — independent MIT/Apache Rust Reticulum implementation, used as a
black-box peer and as an attributed harvest donor. `mere` — sibling repository
supplying data-graph and identity crates.

## Document index

### Governance and program

| Document | What it is |
| --- | --- |
| [`DOC_POLICY.md`](DOC_POLICY.md) | Documentation governance; canonical core plus Retinue addendum. |
| [Work lanes](2026-08-09_retinue_work_lanes.md) | Four-lane split and shared source lock. Lane split authoritative; ordering superseded. |
| [Program sequencing](2026-08-12_program_sequencing_and_deadline_order.md) | Ordering authority since 2026-08-12. Deadline order, not dependency depth. |
| [ARDC application lane](2026-07-31_ardc_application_lane.md) | Grant lane, G0–G4, intake 2026-09-01. All gates unstarted. |
| [Prns harvest brief](2026-08-09_prns_harvest_brief.md) | Donor and external-peer programme; H-gate catalogue. |
| [Prns donor ledger](2026-08-10_prns_donor_ledger.md) | Every donor debt itemised with elected inbound licence. |
| [Prns mobile adoption brief](2026-08-11_prns_mobile_adoption_brief.md) | Mobile lane collaboration, dependency-first recommendation. |

### Protocol core and wire compatibility

| Document | What it is |
| --- | --- |
| [Retinue v0 plan](2026-07-06_retinue_v0_plan.md) | Historical protocol ledger. R0–R10 all closed. Its "next actions" block is not a queue. |
| [RNS wire format reference](2026-07-13_rns_wire_format_reference.md) | Wire reference, byte-fixture corpus, pinned at RNS 1.3.8. |
| [Announce timebase plan](2026-08-25_announce_timebase_plan.md) | Active phased plan for the 5+5 announce blob, persistent stock-RNS decision probes, bounded receive freshness, and crash-monotonic firmware reservations. |
| [Re-pin receipt, RNS 1.5.0 / LXMF 1.1.1](2026-08-23_rns_150_lxmf_111_repin_receipt.md) | Current oracle pin. Records the flake finding and one outrider defect fixed. |
| [Permissive radio protocol compatibility survey](2026-08-25_permissive_radio_protocol_compatibility_survey.md) | Revision-pinned Reticulum, MeshChat, MeshCore and adjacent LoRa survey; separates donors, executable peers, radio adapters, bearers and semantic bridges. Opens no gates. |
| [Compact signed feed and local control plan](2026-08-25_compact_signed_feed_and_local_control_plan.md) | Active cross-repository plan: exact allocation-free tinySSB core first; Mere foreign-source probe, ULCP extraction, Noise attach and radio personality behind explicit gates. |
| [Peer matrix receipt](2026-08-11_prns_peer_matrix_receipt.md) | H8 three-corner interop at RNS 1.4.2. |
| [Peer matrix at RNS 1.5.0](2026-08-23_prns_peer_matrix_rns150_receipt.md) | H8 re-receipted. Seven runs, 35/35. Establishes that capture byte counts are not constants. |
| [Live-gate flake lane](2026-08-23_live_gate_flake_lane.md) | FLK1–FLK5. Per-gate rates, mechanisms, and what a suite run may prove. |
| [IFAC interop](2026-07-28_ifac_interop.md) | R8 complete. |
| [Ratchet / stamp cost and roles](2026-08-07_stamp_cost_and_roles.md) | Stamp cost measurements from T114 receipts. |
| [RNode RX leading byte](2026-08-07_rnode_rx_leading_byte.md) | Leading byte from stock peer diagnosed; RF opacity question closed. |
| [RNode bulk frame loss](2026-07-26_rnode_bulk_frame_loss.md) | Stock RNode silently drops long frames — reproducible upstream defect. |
| [Receive future cancellation](2026-08-08_receive_future_cancellation.md) | Characterised, deliberately not fixed. |

### LXMF and Outrider

| Document | What it is |
| --- | --- |
| [Outrider / LXMF founding](2026-07-25_outrider_lxmf_founding.md) | Gates 1–8. **Closed against pins that no longer exist** — see Known divergences. |
| [LXMF field registry capture](2026-08-13_lxmf_field_registry_capture.md) | Field numbers confirmed by wire capture. |
| [Direct-PHY delivery](2026-07-28_outrider_direct_phy_delivery.md) · [opportunistic direct-PHY](2026-07-28_outrider_direct_phy_opportunistic.md) · [opportunistic delivery](2026-07-28_outrider_opportunistic_delivery.md) · [large propagation response](2026-07-28_outrider_large_propagation_response.md) · [propagation persistence](2026-07-28_outrider_propagation_persistence.md) | Outrider acceptance receipts, all against LXMF 0.9.6 / RNS 1.4.2. |
| [Outrider publish blocker](2026-08-13_outrider_publish_blocker.md) | Resolved; both crates are on crates.io. |

### Radio, firmware, and on-device UI

| Document | What it is |
| --- | --- |
| [Retinue Small plan](2026-07-31_retinue_small_plan.md) | Native-node authority. N0–N6; unplug leg and current figures open. |
| [On-device UI plan](2026-07-28_on_device_ui_implementation_plan.md) | U0–U5, all closed. |
| [On-device UI direction](2026-07-25_on_device_ui.md) | PANEL×LEDGER face, accepted direction. |
| [Mesh household](2026-07-20_mesh_household_tulle_tucket_sennet.md) | Crate topology and naming authority. **Its "no code exists yet" status line is false** — all three crates exist. |
| [Heltec RNode and embedded Rust](2026-07-19_heltec_rnode_and_embedded_rust.md) | Donor, licensing and architecture record. |
| [Modem/embedded/Meshtastic research](2026-07-19_modem_embedded_and_meshtastic_research.md) | Research record, partly superseded. |
| [Low-power UART personality](2026-07-24_low_power_uart_personality.md) | V1 and V2 implemented, hardware-proved through RF wake. |
| [LoRa collision mitigation](2026-07-24_lora_collision_mitigation_ideas.md) | Techniques documented, not scheduled. |
| [First reliable link over RF](2026-07-21_first_reliable_link_over_rf.md) · [Tulle headed acceptance](2026-07-22_tulle_headed_acceptance.md) · [MeshCore relay acceptance](2026-07-22_meshcore_relay_headed.md) · [direct-PHY resource acceptance](2026-07-23_direct_phy_resource_acceptance.md) · [host snapshot acceptance](2026-07-28_host_snapshot_acceptance.md) · [host projection acceptance](2026-07-29_retinue_host_projection_acceptance.md) | On-air and host milestones, July 2026. |
| [T114 UI acceptance](2026-07-28_t114_on_device_ui_acceptance.md) · [V4 UI acceptance](2026-07-28_v4_on_device_ui_acceptance.md) · [display power and field behaviour](2026-07-29_display_power_field_acceptance.md) · [V4 light-sleep and RF wake](2026-07-29_v4_light_sleep_rf_wake_acceptance.md) | Board-level UI and power receipts. |
| [T114 bulk TX asymmetry probe](2026-07-25_t114_bulk_tx_asymmetry_probe.md) | Does not reproduce under Tulle direct-PHY. |
| [RNode direct-PHY RF opacity](2026-07-25_rnode_direct_phy_rf_opacity.md) | **Superseded 2026-08-07** — they do cross. |
| [AIR0/AIR1](2026-08-12_air0_air1_software_receipt.md) · [AIR2](2026-08-12_air2_announce_ingress_receipt.md) · [AIR3 software](2026-08-12_air3_bounded_transport_software_receipt.md) · [AIR3 T114 on-air](2026-08-13_air3_t114_on_air_receipt.md) | Air-lane receipts. The T114 receipt also records the CDC endpoint defect. |
| [Transit link receipt spec](2026-08-14_transit_link_receipt_spec.md) | Spec for the deferred three-party transit receipt. |

### Scaling, executive, and security

| Document | What it is |
| --- | --- |
| [Mesh scaling and asymmetric routing](2026-08-09_mesh_scaling_and_asymmetric_routing.md) | FT1–FT5 authority. |
| [Listener executive and protocol leases](2026-08-10_listener_executive_and_protocol_leases.md) | LE1–LE5. Supersedes retinue-small's channel-ownership clause. |
| [Channel murmuration](2026-08-09_channel_murmuration.md) | CM1–CM5. Framing superseded 2026-08-10; rules survive translated. |
| [LE3 T114 scan-physics receipt](2026-08-20_le3_t114_scan_physics_receipt.md) | LE3a/LE3b complete. |
| [Field node security posture](2026-08-09_field_node_security_posture.md) | FS1–FS6 authority. |
| [Assurance lane status](2026-08-10_assurance_lane_status.md) | ASSURE1–ASSURE5. |
| [FS2 command carrier decision](2026-08-10_fs2_command_carrier_decision.md) · [FS4 custody and FS5 seizure](2026-08-10_fs4_custody_and_fs5_seizure.md) | Assurance decisions and receipts. |

### Distribution, installer, and catalog

| Document | What it is |
| --- | --- |
| [Linkboy public flashing plan](2026-08-08_linkboy_public_flashing_plan.md) | F0–F7 trunk gates plus S1–S7 sidequests (**not gates**). |
| [Linkboy F5 spike](2026-08-08_linkboy_public_flashing_f5_spike.md) | Helper-packaging evidence, complete. |
| [Catalog auth and activation foundation](2026-08-20_catalog_auth_and_activation_foundation.md) | Host-side foundation for authenticated firmware delivery. DIST7. |
| [FCC reselling research](2026-07-20_fcc_reselling_flashed_radios.md) | Findings and v1 posture for flash-and-resell lawfulness. |
| [Meshnology N39 V4.2 profile](2026-08-14_meshnology_n39_v4_2_profile_evidence.md) · [N39 pre-write probe](2026-08-14_com6_n39_v4_2_prewrite_probe.md) | Product-profile evidence and ESP ROM plan. |
| Physical flash receipts | [F5 macOS/Linux](2026-08-19_linkboy_f5_macos_linux_v4_receipt.md) · [F5 Windows custody](2026-08-14_linkboy_f5_windows_custody_receipt.md) · [F4 V4 state preservation](2026-08-20_linkboy_f4_v4_state_preservation_receipt.md) · [T114 raw restore](2026-08-12_t114_retinue_raw_restore_receipt.md) · [T114 package restore](2026-08-12_t114_retinue_linkboy_package_restore_receipt.md) · [Hopspot V4 COM7 interface](2026-08-10_hopspot_v4_com7_interface_receipt.md) |
| JSON receipts | Machine-readable records, schema 1–4. Indexed in full below. |

### JSON receipts

Machine-readable evidence, written by Linkboy and Signalman rather than by hand.
`schema` distinguishes generations; loader snapshots are board-state captures
rather than transfer outcomes.

**Two of these record non-success and are easy to miss**, because nothing in
their filenames says so — see the `result` column.

| Receipt | `result` / content |
| --- | --- |
| [V4 COM6](2026-08-10_linkboy_v4_com6_receipt.json) | schema 2 — Retinue V4 transfer, application verified. |
| [V4 COM7](2026-08-10_linkboy_v4_com7_receipt.json) | schema 2 — Retinue V4 transfer, application verified. |
| [V4 COM7 helper custody](2026-08-10_linkboy_v4_com7_helper_custody_receipt.json) | schema 3 — espflash 4.5.0 custody verified. |
| [V4 COM7 Hopspot restore](2026-08-10_linkboy_v4_com7_hopspot_restore_receipt.json) | schema 3 — full restoration from Hopspot. |
| [Hopspot V4 COM7 transfer](2026-08-10_hopspot_v4_com7_transfer_receipt.json) | schema 3 — bootloader partition and application. |
| [Hopspot V4 COM7 demo](2026-08-11_hopspot_v4_com7_demo_receipt.json) | schema 3 — demo transfer parts. |
| [T114 transfer](2026-08-12_meshtastic_t114_linkboy_transfer_receipt.json) | schema 3 — bootloader and application. |
| [T114 recovery](2026-08-12_meshtastic_t114_linkboy_transfer_recovery_receipt.json) | schema 3 — recovery verification. |
| [T114 replay](2026-08-12_meshtastic_t114_linkboy_transfer_replay_receipt.json) | schema 3 — **`manual-check-required`.** Not a pass. |
| [T114 retry recovery](2026-08-12_meshtastic_t114_linkboy_transfer_retry_recovery_receipt.json) | schema 3 — **`recovery-required`.** Not a pass. |
| [T114 restore completed](2026-08-12_t114_retinue_packaged_restore_completed_receipt.json) | schema 3 — package and loader facts. |
| [T114 restore replay](2026-08-12_t114_retinue_packaged_restore_replay_receipt.json) | schema 3 — retained post-write recovery. |
| [T114 restore verified](2026-08-12_t114_retinue_packaged_restore_verified_receipt.json) | schema 3 — terminal receipt for the restore. |
| [F5 Windows V4 Retinue](2026-08-19_linkboy_f5_windows_v4_retinue_receipt.json) | schema 4 — install with settings preservation. |
| [F5 Windows V4 Hopspot](2026-08-19_linkboy_f5_windows_v4_hopspot_receipt.json) | schema 4 — package transfer and verification. |
| [F4 V4 state preservation](2026-08-20_linkboy_f4_v4_state_preservation_receipt.json) | schema 4 — settings preserved across flashing. |
| [T114 loader snapshot](2026-08-12_t114_loader_snapshot.json) | schema 1 — HT-n5262, nRF52840, 1 MB flash, SoftDevice S140. |
| [T114 AIR3 loader snapshot](2026-08-13_t114_air3_loader_snapshot.json) | schema 1 — persisted state captured for AIR3. |
| [Signalman T114 loader snapshot](2026-08-14_signalman_t114_loader_snapshot.json) | schema 1 — UF2 bootloader 0.9.0. |

### Signalman

| Document | What it is |
| --- | --- |
| [Signalman founding](2026-08-06_signalman_founding.md) | Historical founding note; execution authority moved to the Cambium scope. |
| [Cambium desktop scope](2026-08-09_signalman_cambium_desktop_scope.md) | Desktop G0–G5 authority. G0–G4 complete, G5 partial. |
| [G2 receipt](2026-08-09_signalman_desktop_g2_receipt.md) | Evidence, not a queue. **Its accessibility count is wrong** — see Known divergences. |
| [Management surface direction](2026-08-15_signalman_management_surface_direction.md) | Ruling on the device-data mere. |
| [Management surface plan](2026-08-15_signalman_management_surface_implementation_plan.md) | S0–S9 authority. |
| [S2 live station receipt](2026-08-20_signalman_s2_live_station_receipt.md) | Live bench leg complete, over-the-air announce. |
| Graphical receipts | [V4 COM6 G4](2026-08-10_signalman_v4_com6_g4_receipt.md) · [V4 COM7 owner flow](2026-08-10_signalman_v4_com7_owner_flow_receipt.md) · [T114 graphical](2026-08-14_signalman_t114_graphical_receipt.md) · [COM6 N39](2026-08-14_com6_n39_signalman_graphical_receipt.md) · [N39 Hopspot](2026-08-14_com6_n39_hopspot_signalman_graphical_receipt.md) · [N39 Hopspot→Retinue restore](2026-08-14_com6_n39_hopspot_retinue_signalman_restore_receipt.md) · [Windows staged helper](2026-08-15_signalman_windows_v4_staged_helper_receipt.md) · [public F5 Windows](2026-08-19_signalman_public_f5_windows_receipt.md) |

### Adjacent programmes and pre-decision material

| Document | What it is |
| --- | --- |
| [Smolweb over Reticulum](2026-08-04_smolweb_over_reticulum_plan.md) | Independent application bridge. Not started; R-A/R-B/R-C serial. |
| [Civic deployment](2026-08-11_civic_deployment_prescribed_paths.md) | CV1–CV7, D1–D5. Phase two, post-deadline. |
| [Bluetooth capability scoping](2026-08-11_bluetooth_capability_scoping.md) | LB1–LB6. Pre-decision; awaits the stack ruling. |
| [IoT device concepts](2026-08-13_iot_device_concepts.md) | Brainstorm record. |
| [Lofi voice codec scoping](2026-08-13_lofi_voice_codec_scoping.md) · [Rung 2 codec2 decision](2026-08-13_rung2_codec2_class_decision.md) | Pipit founding and codec choice. |
| [Hopspot demo receipts](2026-08-11_hopspot_v4_com7_demo_receipt.md) · [Meshtastic T114 UF2 demo](2026-08-11_meshtastic_t114_uf2_demo_receipt.md) | Phone-app BLE demos. **Not Retinue capability claims.** |

## Working principles

These are not general good practice. Each is here because its absence cost this
repository something specific and recent.

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

### Build notes

- `apps/signalman-desktop` roots its own workspace and is deliberately excluded
  from the root one. Build it with `-j 1`; concurrent builds exhaust the
  pagefile and surface as roughly thirty spurious internal compiler errors.
- Prefer `cargo test` and `cargo build --all-targets` over `--workspace`. Two
  sessions independently reported that `--workspace` dies on a
  `critical-section` feature-unification clash between firmware and host crates.
  *Reported, not verified here.*
- The oracle's live gates need its virtualenv at `crates/retinue/oracle/.venv`,
  pinned by `requirements.txt` (`rns==1.5.0`, `lxmf==1.1.1`).
- **The Prns peer daemon must be built from inside its own worktree.** Cargo
  resolves `.cargo/config.toml` from the working directory, and Prns pins a
  256 MiB Windows stack there. Built from anywhere else the daemon overflows a
  1 MiB stack before it can parse an argument, and the build reports success.

## Status

Founded 2026-08-24 to meet `DOC_POLICY.md` core §6, which this repository did
not satisfy: 93 markdown documents and 19 JSON receipts were unindexed, and the
only index-shaped artefact was a table inside a dated lane document.

Not yet done: `PROJECT_DESCRIPTION.md` (core §7, maintainer-reserved), an
`archive_docs/` checkpoint (core §4 — nothing has been retired yet), and the
FLK5 audit of the remaining documents that quote bare gate counts.
