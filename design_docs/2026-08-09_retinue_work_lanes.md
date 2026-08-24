# Retinue work lanes

**Date:** 2026-08-09
**Status:** cross-plan execution map. **Ordering superseded 2026-08-12** by
[program sequencing](2026-08-12_program_sequencing_and_deadline_order.md),
which was built from a code-reading audit and found roughly twenty-five of
this document's asserted dependencies unsupported (LE3 does not gate on
LE1/LE2; FT5 does not gate on FT3/FT4; CV2 gates only on FT1's configuration
surface; FS6 does not gate on the Prns H3 harvest). **The four-lane split
below stands; its sequences do not.** That doc also orders the program against
the 2026-09-01 ARDC intake, which this one walls off as external. **Source-lock
locations verified and corrected 2026-08-23** against the actual tree; three of
this document's four placement claims no longer held.

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
| [Linkboy public flashing plan](2026-08-08_linkboy_public_flashing_plan.md) | Active installer authority. The 2026-08-20 audit closed F1 and F2 outright; F3 and F4 are complete except two physical claims (as-shipped-board plan and V4 reset/boot-control recovery). The paired V4 preservation fact closed on 2026-08-20. F5 is complete: V4 has official per-platform helper custody and physical Windows, Intel-macOS, Apple-silicon-macOS, and Linux receipts; public T114 uses built-in UF2 and has its graphical Windows real-device receipt. F7 is complete: the graphical Prns V4 and Meshtastic T114 install-and-Retinue-restore receipts landed, and the catalog was published 2026-08-20 (Retinue `05b3795`, Mer3ly `94a7d64`, live V4/T114 pages). |
| [Linkboy F5 spike](2026-08-08_linkboy_public_flashing_f5_spike.md) | Complete helper-packaging evidence. V4 selects and verifies official per-platform `espflash 4.5.0` artifacts; public T114 uses Linkboy's built-in stock-bootloader UF2 route. Ambient PATH is development-only. Cross-platform V4 receipts and the public T114 real-device receipt are recorded. |
| [Signalman Cambium desktop scope](2026-08-09_signalman_cambium_desktop_scope.md) | Active GUI authority. G0 through G3 are complete. G4 has physical V4 and T114 cross-firmware-and-recovery receipts, and the owner supplied the manual screen-reader judgement for Windows. |
| [Signalman G2 receipt](2026-08-09_signalman_desktop_g2_receipt.md) | Evidence, not a work queue. It proves the headed shell and automated accessibility surface, not a board flash. |
| [Mesh scaling and asymmetric routing](2026-08-09_mesh_scaling_and_asymmetric_routing.md) | Active network-scaling authority. FT1 through FT5 own airtime, expiry, bidirectionality, ETX, and scope policy. |
| [Channel murmuration](2026-08-09_channel_murmuration.md) | Framing superseded 2026-08-10 by the listener-executive doc; its rules and CM2 through CM5 survive translated and stay serial, consuming FT1, FT3, and FT5 facts. CM1 is absorbed into LE2. |
| [Listener executive and protocol leases](2026-08-10_listener_executive_and_protocol_leases.md) | Active executive-boundary authority. LE1 through LE5 own the adapter boundary, bounded leases, and the DetectionProfile/ReceiveProfile scan plan; supersedes retinue-small decision 4's channel-ownership clause. |
| [Field node security posture](2026-08-09_field_node_security_posture.md) | Active security authority. FS1 through FS6 own ingest, command authorization, replay, custody, seizure, and bounded tables. |
| [Prns harvest brief](2026-08-09_prns_harvest_brief.md) | Active donor and external-peer program. It supplies candidates and evidence; it does not take ownership from FT, CM, FS, Linkboy, or Signalman. |
| [Assurance lane status](2026-08-10_assurance_lane_status.md), [FS2 carrier decision](2026-08-10_fs2_command_carrier_decision.md), and [FS4/FS5](2026-08-10_fs4_custody_and_fs5_seizure.md) | Current assurance evidence. ASSURE1 and the unsafe audit pass; ASSURE3 through ASSURE5 are complete in software. A first green Linux fuzz run, FS3, on-metal command verification, physical FS4, and disclosure remain open. |
| [Bluetooth capability scoping](2026-08-11_bluetooth_capability_scoping.md) | Pre-decision candidate lane. LB1 through LB6 do not become active gates until the stack ruling is accepted; LB1 is the only opening hardware risk. |
| [Civic deployment](2026-08-11_civic_deployment_prescribed_paths.md) | Phase-two program consuming FT/FS/LE/LB facts. CV1 through CV6 and D1 through D5 are not pilot-critical engineering gates. |
| [Live-gate flake lane](2026-08-23_live_gate_flake_lane.md) | Active measurement lane, opened 2026-08-23. FLK1 through FLK5 own the per-gate failure rates of the live RNS/LXMF oracle gates and what a suite run is allowed to prove. Until it closes, a bare "twelve of twelve" count is not evidence. Does not cover the peer matrix, which never runs the flaking gates. |
| [Signalman management surface plan](2026-08-15_signalman_management_surface_implementation_plan.md) | Active product authority for the Signalman desktop beyond flashing: Devices, Network, Messages, Map, and Browse. S0/S1 and the S2/S3 software slices are verified, and S2's live bench leg completed 2026-08-20 through Mere `1609cb90`'s lease-checked station getter and the desktop's live station actor. The owner-driven S2 headed interactive judgement, G5 hide/reopen receipt, and S5 headed two-site audible voice remain open. Presentation code takes no radio or flashing authority. |

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
   exact stock RNS peer version in each receipt. It was 1.4.2 when this
   document was written and is 1.5.0 since the 2026-08-23 re-pin; the duty is
   to record whichever version actually ran, not to hold a particular one.
2. Add the itemized Prns donor ledger and the intended inbound license for each
   copied seam.
3. Preserve a clean, untouched Prns executable for black-box peer work.
4. Put the security finding in a private disclosure record before publishing
   board, path, reproduction, or impact details.

Items 1 through 3 hold, but this document's account of *where* they hold was
wrong by 2026-08-23 and is corrected here against the actual tree.

The clean oracle checkout is `Code/crates/prns`, which now sits on `main` at
`df05c6bf` (Prns 0.3.6) rather than at the pin. The H8 pin `72b6b30d` remains
reachable there by revision, and the peer matrix detaches a disposable worktree
from it, so nothing depends on the checkout itself resting on the pinned commit.
The working T114 port this document placed in `Code/repos/Prns` no longer
exists, and `Code/worktrees/Prns-t114-upstream` is orphaned: its `.git` file
still points into that vanished repository. Neither is load-bearing for H8. The
stale `repos/Prns` location had also been written into the peer-matrix driver,
where it silently broke the receipt procedure until `d0d31c3`.

Item 3 is satisfied by recipe rather than by a stored binary, which is a change
of kind and is recorded deliberately. The executable this document meant lived
in `%TEMP%` and is gone. What replaces it is the reproducible path from pinned
source to a working daemon, repaired and documented on 2026-08-23: the peer must
be built from **inside its own worktree**, because Prns pins a 256 MiB Windows
stack in its own `.cargo/config.toml` and Cargo resolves that file relative to
the working directory rather than to `--manifest-path`. Built from anywhere
else the daemon overflows a 1 MiB stack before it can parse an argument. See the
[RNS 1.5.0 peer matrix receipt](2026-08-23_prns_peer_matrix_rns150_receipt.md).

Item 4 is still owed: the donor ledger records that no private disclosure record
has been sent, and no disclosure directory exists in the tree. This blocks
publication of the finding and any release claim that the disclosure duty is
complete; it does not freeze unrelated software or hardware work. Assurance owns
its closure.

## Lane 1: Peer

**Owns:** H8, mixed-runtime interop, independent discrepancy records.
**Primary write surface:** `crates/retinue/oracle/`, oracle scripts, captured
receipts.
**Current receipt:** [RNS 1.5.0 peer
matrix](2026-08-23_prns_peer_matrix_rns150_receipt.md), seven runs all passing,
which supersedes the version claim of the 2026-08-11 receipt but not its lane
boundary.

Sequence:

1. Run the three exact pairings: Retinue to RNS, Retinue to Prns, and Prns to
   RNS.
2. Reuse the existing live-gate shape where it describes the same protocol
   claim. Keep process versions, commands, and ports exact. Captured **bytes**
   cannot be held exact and must not be compared raw: HDLC byte-stuffing over
   freshly minted ephemeral identities gives every run a different capture
   length. Compare unstuffed lengths, which are invariant, as the 2026-08-23
   receipt sets out.
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

**Opening software status (2026-08-12):** AIR0's bounded profile registry,
AIR1's Reticulum announce pacing path, and AIR2's attributed ingress admission
are implemented and host-tested; see the
[AIR0/AIR1 receipt](2026-08-12_air0_air1_software_receipt.md) and
[AIR2 receipt](2026-08-12_air2_announce_ingress_receipt.md). The
[LE3 scan-physics receipt](2026-08-20_le3_t114_scan_physics_receipt.md) now
supplies the real-radio CAD, retune, handoff, acquisition, cross-sync miss,
and exact-capture facts for the LE3a/LE3b physical slice. It does not supply
the long-run rotating-scheduler miss-rate calibration or FT1. AIR2 is not the
bounded firmware-state proof required by FT2/FS6. AIR3 now supplies the
native-node route/relay model, a true T114 heap-peak instrument, and an
[on-air bounded-state receipt](2026-08-13_air3_t114_on_air_receipt.md): live
route eviction, peer refusal, a stable 18,168-byte heap peak, and independently
observed hop-one type-2 announce relays. The stronger link-request/proof relay
transaction remains later transport coverage. See also the
[AIR3 software receipt](2026-08-12_air3_bounded_transport_software_receipt.md).

Sequence:

1. **AIR0, profile model and LE3 scan physics (physical slice complete
   2026-08-20):** CAD detection stays separate from exact packet capture. The
   T114 consumer schedules two DetectionProfiles and three ReceiveProfiles;
   shared SF/BW shares detection while different sync words receive separate
   measured dwell. Long-run rotating-scheduler calibration remains later.
2. **AIR1, outbound pressure:** extend `tulle::AirtimeBudget` with
   announce-specific pacing adapted from Prns's `AnnouncePacer`. Keep the
   existing shared transmission budget authoritative. Close FT1 with modeled
   versus measured airtime and an enforced announce cap on two interfaces.
3. **AIR2, inbound pressure (complete):** the Retinue-owned interface and
   destination announce-admission state machines retain ingress attribution,
   bound held work, and have a separate host flood receipt. This does not turn
   the host receipt into a firmware high-water proof.
4. **AIR3, bounded firmware state (software complete):** native-node routes
   expire and evict inside a typed table, transit has bounded bridge and
   de-duplication state, and the T114 exposes a peak heap counter. Run the
   existing flood on the newly flashed board for sustained high-water and
   transport-relay evidence before calling FT2/FS6 complete.
5. **AIR4, executive boundary:** land LE1 and LE2 (adapter boundary, lease
   revocation) with an on-air receipt. Absorbs the old CM1 hot-switch gate.
6. **AIR5, measured MAC work:** evaluate Prns's CCA, fairness, and diagnostics
   against Retinue's current SX1262 path. Port techniques only where Retinue's
   measurements name a real deficit.

Per the 2026-08-12 sequencing correction, LE3's physical scan slice ran
independently of LE1 and LE2. After LE2, continue through FT3
bidirectionality, FT4 delivery ratio, FT5 scope policy, LE4, LE5, and the
translated CM2 through CM5. Those remaining gates share the Executive, radio
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
expiry, target class, replay policy, and key custody. Distribution now supplies
the Merely package index's cryptographic verification boundary: detached
Minisign over canonical bytes, checked against owner-selected local trust, with
an authenticated wrapper type for downstream staging. Assurance adoption is
not claimed here. The public index is still unsigned, so the offline key
ceremony and first real signed-index receipt remain owed before any
network-fetched catalog or OTA bearer relies on it.

## Lane 4: Distribution

**Owns:** H6 and H9; remaining Linkboy F1 through F7 acceptance; Signalman G4.
**Primary write surface:** `apps/linkboy`, `apps/signalman`, package manifests,
installer and recovery receipts.

Current ground truth: Linkboy's F1 through F4 software slices and sparse-package
extension are present. Signalman G0 through G3 have receipts. On Windows,
Signalman installed Prns Hopspot 0.3.4 on the N39 V4.2, recorded the package's
required serial self-check, and restored Retinue to terminal `Complete`.
Signalman also installed the admitted Meshtastic T114 package and restored
Retinue through its graphical flow. F7's first interoperability proof is now
complete. The older Windows V4 stage bundles and resolves its pinned ESP helper,
flashes Hopspot, and recovers Retinue without helper or catalog environment
overrides. F5 now admits official per-platform `espflash 4.5.0` artifacts and
uses a built-in stock-bootloader UF2 route for public T114. The full Windows
stage has a software and headed preflight receipt. Its staged Linkboy completed
the V4 Hopspot self-check and Retinue recovery on O-PC. Standalone staged
Linkboy completed the same loop on Intel macOS, Apple-silicon macOS, and Linux.
Those receipts are Linkboy physical evidence, not a headed Signalman receipt.
The graphical public T114 UF2 flow returned `Complete` with the expected
Retinue application identity, closing F5. The owner supplied the manual Windows
screen-reader judgement. The T114 repeated-CDC-session fault is a separate
reliability defect. F7 publication closed on 2026-08-20: Retinue `05b3795` and
Mer3ly `94a7d64` are published and the live V4 and T114 device pages project
the receipted catalog; the Meshtastic T114 entry stays `partial` until its own
interface confirms the installed version.

Sequence:

1. **DIST0, landed-gate audit: run 2026-08-20.** F1 and F2 are complete —
   every done condition maps to a named Linkboy test, and the structured
   executor carried both physical cable routes again in the 2026-08-12/14
   `adafruit-dfu` and 2026-08-19 ESP-ROM receipts. Two physical claims stay
   open and keep F3 and F4 from full closure: a factory as-shipped board
   reaching a valid plan (F3), and V4 recovery through the board's own
   reset/boot controls (F4; the T114 double-tap-reset leg is receipted). The
   paired V4 identity/settings preservation fact closed on 2026-08-20.
   The gate-by-gate evidence is recorded in the flashing plan's F1 through F4
   sections.
2. **DIST1, package shape: landed.** Ordered sparse parts, per-part offsets and
   hashes, preserved ranges, and publisher-signature evidence are present.
3. **DIST2, helper custody: complete.** V4 records and verifies official
   per-platform helper artifacts; public T114 uses the built-in UF2 writer. The
   public T114 real-device receipt closes F5.
4. **DIST3, owner route:** graphical V4 and T114 recovery receipts are complete.
   The manual Windows accessibility judgement is complete.
5. **DIST4, V4 second publisher: complete.** Signalman installed Hopspot,
   recorded its required interface check, and restored Retinue to a complete
   terminal result. No Retichat pairing is claimed.
6. **DIST5, T114 second publisher: complete.** Signalman installed the admitted
   Meshtastic UF2 and restored Retinue through the graphical flow.
7. **DIST6, optional product consumer:** deferred. No browser-rendezvous
   exposure is authorized by these installer receipts; it is not an F7 gate.
8. **DIST7, authenticated update foundation: host slice complete.** Catalog
   signatures are now verified against owner-selected local trust before an
   authenticated catalog type exists. Monotonic staging, trial confirmation,
   rollback, and recovery-only refusal are modeled and tested. The public index
   remains unsigned and neither board has an on-device activation receipt, so
   Bluetooth, Wi-Fi, and LoRa OTA bearers remain blocked. See
   `2026-08-20_catalog_auth_and_activation_foundation.md`.

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
