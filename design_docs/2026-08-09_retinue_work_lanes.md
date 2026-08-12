# Retinue work lanes

**Date:** 2026-08-09
**Status:** cross-plan execution map

This document splits Retinue's remaining work without replacing the plans that
own each gate. It exists because the Prns harvest brief exposed work in four
different ownership domains and its ten-item order made them look needlessly
serial.

The split is four engineering lanes: **Peer**, **Air**, **Assurance**, and
**Distribution**. They can start in parallel after the shared source lock
below. Work inside each lane stays serial where it touches the same state or
where one receipt is a real precondition for the next.

## Plan audit

Live code and current receipts outrank an older plan's status paragraph. The
current plan set sorts as follows.

| Document | Present authority |
| --- | --- |
| [Retinue v0 plan](2026-07-06_retinue_v0_plan.md) | Historical protocol and receipt ledger. R9/R10 and the on-air milestone landed; its old R2/R3 next-actions block is not a current queue. |
| [Heltec RNode and embedded Rust](2026-07-19_heltec_rnode_and_embedded_rust.md) | Donor, licensing, and architecture record. Native-node execution moved to Retinue Small. |
| [Modem, embedded, and Meshtastic research](2026-07-19_modem_embedded_and_meshtastic_research.md) | Research record, superseded in part by Retinue Small and the direct-PHY work. |
| [Mesh household](2026-07-20_mesh_household_tulle_tucket_sennet.md) | Naming, component-boundary, and license authority. Its statement that the components do not exist is stale. |
| [Outrider / LXMF founding](2026-07-25_outrider_lxmf_founding.md) | Closed ledger. Gates 1 through 8 have receipts. |
| [On-device UI plan](2026-07-28_on_device_ui_implementation_plan.md) | Closed ledger. U0 through U5 are complete. |
| [ARDC application lane](2026-07-31_ardc_application_lane.md) | Active external lane. The 2026-09-01 intake deadline remains separate from engineering gates. |
| [Retinue Small plan](2026-07-31_retinue_small_plan.md) | Current native-node authority. Software gates N0 through N6 are substantially closed. The unplug leg, current measurements, and the cheaper carrier-sense question remain owner or research work. |
| [Smolweb over Reticulum](2026-08-04_smolweb_over_reticulum_plan.md) | Active independent application bridge. R-A, R-B, and R-C remain serial within that program. |
| [Signalman founding](2026-08-06_signalman_founding.md) | Historical founding note. Execution authority moved to the Cambium desktop scope and receipts. |
| [Linkboy public flashing plan](2026-08-08_linkboy_public_flashing_plan.md) | Active installer authority. F1 through F4 have landed software, but their physical/public acceptance remains open where required. F5 is open. F7 has a signed Prns V4 Linkboy install, interface, and restore receipt; its graphical cross-firmware and T114 halves remain open. |
| [Linkboy F5 spike](2026-08-08_linkboy_public_flashing_f5_spike.md) | Current evidence for the helper-packaging decision. PATH dependencies remain; the T114 helper is not yet a public bundling candidate. |
| [Signalman Cambium desktop scope](2026-08-09_signalman_cambium_desktop_scope.md) | Active GUI authority. G0 through G3 are complete. G4 has a keyboard-operated physical V4 install receipt; recovery, T114, and manual screen-reader judgment remain. |
| [Signalman G2 receipt](2026-08-09_signalman_desktop_g2_receipt.md) | Evidence, not a work queue. It proves the headed shell and automated accessibility surface, not a board flash. |
| [Mesh scaling and asymmetric routing](2026-08-09_mesh_scaling_and_asymmetric_routing.md) | Active network-scaling authority. FT1 through FT5 own airtime, expiry, bidirectionality, ETX, and scope policy. |
| [Channel murmuration](2026-08-09_channel_murmuration.md) | Framing superseded 2026-08-10 by the listener-executive doc; its rules and CM2 through CM5 survive translated and stay serial, consuming FT1, FT3, and FT5 facts. CM1 is absorbed into LE2. |
| [Listener executive and protocol leases](2026-08-10_listener_executive_and_protocol_leases.md) | Active executive-boundary authority. LE1 through LE5 own the adapter boundary, bounded leases, and the DetectionProfile/ReceiveProfile scan plan; supersedes retinue-small decision 4's channel-ownership clause. |
| [Field node security posture](2026-08-09_field_node_security_posture.md) | Active security authority. FS1 through FS6 own ingest, command authorization, replay, custody, seizure, and bounded tables. |
| [Prns harvest brief](2026-08-09_prns_harvest_brief.md) | Active donor and external-peer program. It supplies candidates and evidence; it does not take ownership from FT, CM, FS, Linkboy, or Signalman. |
| [Assurance lane status](2026-08-10_assurance_lane_status.md), [FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md), and [FS4/FS5](2026-08-10_fs4_custody_and_fs5_seizure.md) | Current assurance evidence. ASSURE1 and the unsafe audit pass; ASSURE3 through ASSURE5 are complete in software. A first green Linux fuzz run, FS3, on-metal command verification, physical FS4, and disclosure remain open. |
| [Bluetooth capability scoping](2026-08-11_bluetooth_capability_scoping.md) | Pre-decision candidate lane. LB1 through LB6 do not become active gates until the stack ruling is accepted; LB1 is the only opening hardware risk. |
| [Civic deployment](2026-08-11_civic_deployment_prescribed_paths.md) | Phase-two program consuming FT/FS/LE/LB facts. CV1 through CV6 and D1 through D5 are not pilot-critical engineering gates. |

The low-power UART personality and first on-device UI design are inputs to the
Retinue Small and UI ledgers, not separate queues. The 2026-07-21 through
2026-07-29 RF, Outrider, UI, host-projection, and power documents are evidence
and acceptance receipts. The wire reference, stamp roles, RNode leading-byte finding,
[receive-future cancellation](2026-08-08_receive_future_cancellation.md), FCC
notes, and collision notes remain references or constraints. None creates an
additional execution lane.

## Shared source lock

The source lock has four items:

1. Pin Prns at `72b6b30d27cac910ce20d370e1dc711fe9b95955` and record the
   exact RNS 1.4.2 peer version.
2. Add the itemized Prns donor ledger and the intended inbound license for each
   copied seam.
3. Preserve a clean, untouched Prns executable for black-box peer work.
4. Put the security finding in a private disclosure record before publishing
   board, path, reproduction, or impact details.

Items 1 through 3 are present: the clean oracle checkout remains
`Code/crates/prns` at the pin, distinct from the working T114 port in
`Code/repos/Prns` and its current-main adaptation in
`Code/worktrees/Prns-t114-upstream`. Item 4 is still owed: the donor ledger
records that no private disclosure record has been sent. This blocks
publication of the finding and any release claim that the disclosure duty is
complete; it does not freeze unrelated software or hardware work. Assurance
owns its closure.

## Lane 1: Peer

**Owns:** H8, mixed-runtime interop, independent discrepancy records.
**Primary write surface:** `crates/retinue/oracle/`, oracle scripts, captured
receipts.

Sequence:

1. Run the three exact pairings: Retinue to RNS, Retinue to Prns, and Prns to
   RNS.
2. Reuse the existing live-gate shape where it describes the same protocol
   claim. Keep process versions, commands, ports, and captured bytes exact.
3. Cross-check O-10, hops on rebroadcast, against Prns's retransmit behavior.
4. Record disagreements before any implementation harvest touches that seam.

Done means each pairing has a reproducible live receipt and each disagreement
has an owner. Agreement after a Prns-derived port is donor-conformance evidence
for that seam, even if the untouched Prns process remains a useful mixed-peer
participant.

This lane does not import donor code. It may add tests to the owning crate, but
the Assurance lane owns the central validation inventory so two lanes do not
edit the registry concurrently.

## Lane 2: Air

**Owns:** H1 through H4 and H10; FT1 through FT5; LE1 through LE5 and the
translated CM2 through CM5; FS6.
**Primary write surface:** `crates/tulle`, `crates/retinue`, `crates/radio-hand`,
and board firmware.

Sequence:

1. **AIR0, profile model:** keep CAD detection separate from exact packet
   capture. The executive schedules DetectionProfiles and ReceiveProfiles;
   shared SF/BW may share a CAD observation, but different sync words require
   different capture dwell. LE3's proof measures both stages.
2. **AIR1, outbound pressure:** extend `tulle::AirtimeBudget` with
   announce-specific pacing adapted from Prns's `AnnouncePacer`. Keep the
   existing shared transmission budget authoritative. Close FT1 with modeled
   versus measured airtime and an enforced announce cap on two interfaces.
3. **AIR2, inbound pressure:** port the interface and destination announce
   admission state machines with attribution and a separate flood receipt.
4. **AIR3, bounded firmware state:** add route expiry and eviction, then extend
   the existing T114 flood receipt with sustained memory high-water and
   transport relay evidence. This completes the still-open firmware portion of
   FT2/FS6 rather than repeating N0 through N6.
5. **AIR4, executive boundary:** land LE1 and LE2 (adapter boundary, lease
   revocation) with an on-air receipt. Absorbs the old CM1 hot-switch gate.
6. **AIR5, measured MAC work:** evaluate Prns's CCA, fairness, and diagnostics
   against Retinue's current SX1262 path. Port techniques only where Retinue's
   measurements name a real deficit.

After LE2, continue through FT3 bidirectionality, FT4 delivery ratio, and FT5
scope policy before LE3 through LE5 and the translated CM2 through CM5. The
sequence stays together because those gates share the Executive, radio
ownership, dwell state, and firmware bench. Prns's manifold wake scheduling and `warmth`/`departed-interface` grace
belong in that later design pass.

## Lane 3: Assurance

**Owns:** H5 and H7; FS1 through FS5; validation inventory; coordinated
disclosure.
**Primary write surface:** validation tooling, security tests, signed-artifact
experiments, private disclosure material.

Current ground truth:

- **ASSURE1:** validation inventory, orphan detection, and exact-SHA recording
  pass.
- **ASSURE2:** the unsafe audit passes and three fuzz targets exist. Windows
  cannot run them; the Linux CI job exists but has not yet produced a witnessed
  green campaign receipt.
- **ASSURE3:** six stock RNS 1.4.2 signed artifacts were reproduced byte for
  byte.
- **ASSURE4 / FS2:** the compact Retinue command envelope is implemented. The
  RNS signed artifact remains a host-tier carrier.
- **ASSURE5:** custody policy and the enforced seizure inventory exist in
  software. Physical FS4 remains Distribution evidence.

Next sequence:

1. **ASSURE6, run what exists:** capture the first green Linux fuzz result for
   all registered targets rather than treating registration as execution.
2. **FS3, durable replay state:** bind the settled command grammar to the
   wear-leveled flash counter and prove power-cut monotonicity and erase life.
   Settle opcode ownership in the same pass.
3. **Command lifecycle:** write the bootstrap, key-rotation, and last-key
   revocation ceremony. Then capture an on-metal command over RF; a host-only
   reboot simulation cannot close that claim.
4. **Disclosure:** send the private Prns report through its documented security
   route and record only its state here. Never publish the board/path details
   from the disclosure record.

The signed artifact is a carrier. Retinue still owns command authorization,
expiry, target class, replay policy, and key custody. The signed Merely index
continues to authorize installable packages.

## Lane 4: Distribution

**Owns:** H6 and H9; remaining Linkboy F1 through F7 acceptance; Signalman G4.
**Primary write surface:** `apps/linkboy`, `apps/signalman`, package manifests,
installer and recovery receipts.

Current ground truth: Linkboy's F1 through F4 software slices and sparse-package
extension are present; 50 library tests and the retained Prns signature test
pass. Signalman G0 through G3 have receipts and the V4 keyboard route ran on
hardware. Linkboy installed, exercised, and restored official Hopspot on V4.
Official Meshtastic ran on the T114 through a manual UF2 demo, but no admitted
Linkboy package or graphical restore receipt exists. Public helper policy,
graphical recovery, the T114 G4 path, manual screen-reader judgment, and the
T114 second-publisher package remain open.

Sequence:

1. **DIST0, landed-gate audit:** check F1 through F4 against their exact done
   conditions. Run the missing structured-executor, as-shipped discovery,
   post-write verification, and recovery receipts on both boards. Keep a gate
   open where its physical claim has not run.
2. **DIST1, package shape: landed.** Ordered sparse parts, per-part offsets and
   hashes, preserved ranges, and publisher-signature evidence are present.
3. **DIST2, helper custody:** finish F5 for public installation. Record the
   helper version and digest in every plan and receipt; do not promote the
   current T114 helper until its distribution and recovery policy is settled.
4. **DIST3, owner route:** rerun G4 on physical V4 and T114 paths, including
   recovery and the manual accessibility judgment.
5. **DIST4, V4 second publisher:** Linkboy install/interface/restore is proven.
   Repeat the cross-firmware restore through Signalman's graphical flow and
   retain the complete terminal receipt. Record an actual Retichat pairing if
   the BLE demo is claimed; boot readiness alone is not a phone receipt.
6. **DIST5, T114 second publisher:** admit the already-observed official
   Meshtastic T114 UF2 as a signed package, then install and restore Retinue
   through the graphical flow. The manual demo is candidate evidence, not F7.
7. **DIST6, optional product consumer:** only after the install and restore
   receipts, decide whether the V4 field gateway should expose browser
   rendezvous to Turnstone.

Linkboy owns package policy, immutable plans, execution, recovery, and
receipts. Signalman is its Cambium face. Turnstone may consume a field gateway;
it does not become the routing or flashing authority.

## Collision map

| Lane | Exclusive authority while active | Shared seam rule |
| --- | --- | --- |
| Peer | Oracle process control and interop receipts | Capture a seam before that seam imports Prns code. |
| Air | Tulle budget, radio Executive, firmware routing tables | Book V4/T114 bench sessions with Distribution; do not share a live flash session. |
| Assurance | Validation registry and security-policy tests | Other lanes add owner tests, then Assurance registers them. |
| Distribution | Linkboy/Signalman plan, execution, and recovery state | It consumes FS4 policy and returns physical custody receipts. |

One lane may provide evidence to another, but it cannot close the other plan's
gate. A physical receipt says which board and route actually ran. A software
test cannot be promoted to that claim.

## Work outside the four engineering lanes

- **Prns upstream:** the hardware-proven old-base port is preserved in
  `Code/repos/Prns` as `3939a726` plus `365e0b01`. The current-main adaptation
  lives in `Code/worktrees/Prns-t114-upstream` as `1e83d23a` plus `855330a1`,
  split into shared SX1262 correctness and the board target. Its T114 and
  T-Echo locked builds, T114 clippy, focused radio/USB tests, and validation
  registry pass. The current-main binary still needs the physical USB/RF
  receipt before publication; the older UF2 receipt is preserved evidence, not
  proof of this rebuild. Public signed-flasher admission and multi-node CSMA
  qualification follow the board contribution. A separate modest proposal may
  expose one configurable LoRa sync word; Retinue's multi-ReceiveProfile
  scheduler is not smuggled into Prns with it.
- **Bluetooth, pre-decision:** if the LB ruling is accepted, LB1 is the opening
  proof: enable the resident T114 SoftDevice, record RAM, and rerun counted
  SX1262 receipts. LB2 through LB6 remain gated by that result. BLE transport,
  BLE firmware update, and LoRa ReceiveProfile scanning are separate claims.
- **Civic deployment, phase two:** CV1 through CV6 consume measured FT facts.
  The pilot ships the open mesh and measurement machinery first; prescriptions,
  emergency precedence, remote shaping, and the atlas do not jump that gate.
- **Smolweb:** R-A, then R-B, then optional R-C. It can begin independently and
  should not be folded into the Prns harvest.
- **ARDC:** application writing and submission remain an external lane. The
  next stated intake deadline is 2026-09-01.
- **Owner receipts:** the Retinue Small unplug leg and current measurements can
  be captured whenever the bench and meter are available. They do not block
  software in the four lanes.

## Global done conditions

This lane split has done its job when:

1. every active engineering gate in this map names exactly one owning lane;
2. the untouched Prns peer receipt precedes donor work in the same seam;
3. FT, LE, and translated CM work share one radio-state sequence instead of
   racing edits;
4. FS policy remains separate from artifact and installer mechanisms;
5. F7 has second-publisher install-and-restore receipts on both supported board
   families; and
6. closed and historical plans are cited as evidence rather than reopened as
   work queues.
