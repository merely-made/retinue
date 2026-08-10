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
| [Channel murmuration](2026-08-09_channel_murmuration.md) | Active channel-mobility authority. CM1 through CM5 stay serial and consume FT1, FT3, and FT5 facts. |
| [Field node security posture](2026-08-09_field_node_security_posture.md) | Active security authority. FS1 through FS6 own ingest, command authorization, replay, custody, seizure, and bounded tables. |
| [Prns harvest brief](2026-08-09_prns_harvest_brief.md) | Active donor and external-peer program. It supplies candidates and evidence; it does not take ownership from FT, CM, FS, Linkboy, or Signalman. |

The low-power UART personality and first on-device UI design are inputs to the
Retinue Small and UI ledgers, not separate queues. The 2026-07-21 through
2026-07-29 RF, Outrider, UI, host-projection, and power documents are evidence
and acceptance receipts. The wire reference, stamp roles, RNode leading-byte finding,
[receive-future cancellation](2026-08-08_receive_future_cancellation.md), FCC
notes, and collision notes remain references or constraints. None creates an
additional execution lane.

## Shared source lock

All four lanes may begin once this small common record exists:

1. Pin Prns at `72b6b30d27cac910ce20d370e1dc711fe9b95955` and record the
   exact RNS 1.4.2 peer version.
2. Add the itemized Prns donor ledger and the intended inbound license for each
   copied seam.
3. Preserve a clean, untouched Prns executable for black-box peer work.
4. Put the security finding in a private disclosure record before publishing
   board, path, reproduction, or impact details.

The shared lock is a provenance and evidence boundary. It does not require a
board session and should not hold up independent software work.

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

**Owns:** H1 through H4 and H10; FT1 through FT5; CM1 through CM5; FS6.
**Primary write surface:** `crates/tulle`, `crates/retinue`, `apps/radio-hand`,
and board firmware.

Sequence:

1. **AIR1, outbound pressure:** extend `tulle::AirtimeBudget` with
   announce-specific pacing adapted from Prns's `AnnouncePacer`. Keep the
   existing shared transmission budget authoritative. Close FT1 with modeled
   versus measured airtime and an enforced announce cap on two interfaces.
2. **AIR2, inbound pressure:** port the interface and destination announce
   admission state machines with attribution and a separate flood receipt.
3. **AIR3, bounded firmware state:** add route expiry and eviction, then extend
   the existing T114 flood receipt with sustained memory high-water and
   transport relay evidence. This completes the still-open firmware portion of
   FT2/FS6 rather than repeating N0 through N6.
4. **AIR4, hot switching:** close CM1 with teardown-correct retuning and an
   on-air switch receipt.
5. **AIR5, measured MAC work:** evaluate Prns's CCA, fairness, and diagnostics
   against Retinue's current SX1262 path. Port techniques only where Retinue's
   measurements name a real deficit.

After CM1, continue through FT3 bidirectionality, FT4 delivery ratio, and FT5
scope policy before CM2 through CM5. The CM sequence stays together because
those gates share the Executive, radio ownership, dwell state, and firmware
bench. Prns's manifold wake scheduling and `warmth`/`departed-interface` grace
belong in that later design pass.

## Lane 3: Assurance

**Owns:** H5 and H7; FS1 through FS5; validation inventory; coordinated
disclosure.
**Primary write surface:** validation tooling, security tests, signed-artifact
experiments, private disclosure material.

Sequence:

1. **ASSURE1, validation minimum:** inventory existing cross-boundary suites,
   enforce exact-SHA result records, and detect orphan validation assets. Keep
   assertions in their owning tests rather than copying Prns's suite list.
2. **ASSURE2, ingest and unsafe boundaries:** fuzz the whole Retinue ingest
   path with deterministic entropy and immutable seeds copied to a writable
   corpus. Add a first-party unsafe-policy audit with an explicit exception
   list.
3. **ASSURE3, carrier evidence:** reproduce the canonical RNS RSG/RSM vectors
   independently in Retinue.
4. **ASSURE4, command decision:** decide whether FS2 uses the interoperable
   signed-artifact carrier or a smaller Retinue envelope. Then implement FS2
   before FS3, because the durable monotonic counter must bind the settled
   command grammar.
5. **ASSURE5, custody and seizure:** finish FS4 process policy and FS5's
   compromised-node inventory. The Distribution lane supplies the physical
   secure-boot and recovery receipts where installer behavior is involved.

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
hardware. Public helper policy, graphical recovery, the T114 G4 path, manual
screen-reader judgment, and the T114 second-publisher package remain open.

Sequence:

1. **DIST0, landed-gate audit:** check F1 through F4 against their exact done
   conditions. Run the missing structured-executor, as-shipped discovery,
   post-write verification, and recovery receipts on both boards. Keep a gate
   open where its physical claim has not run.
2. **DIST1, package shape:** extend the immutable package and plan model from a
   single payload to ordered sparse parts with offsets, individual hashes, and
   preserved provisioning ranges. Retain Prns's publisher signature as
   package evidence beneath authorization by the signed Merely index.
3. **DIST2, helper custody:** finish F5 for public installation. Record the
   helper version and digest in every plan and receipt; do not promote the
   current T114 helper until its distribution and recovery policy is settled.
4. **DIST3, owner route:** rerun G4 on physical V4 and T114 paths, including
   recovery and the manual accessibility judgment.
5. **DIST4, V4 second publisher:** install official Hopspot on a V4, exercise
   its expected interface, and restore Retinue through the same graphical
   flow.
6. **DIST5, T114 second publisher:** select and admit a separate official T114
   upstream firmware. Hopspot has no T114 target, so the V4 receipt alone
   cannot close F7.
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
3. FT and CM work share one radio-state sequence instead of racing edits;
4. FS policy remains separate from artifact and installer mechanisms;
5. F7 has second-publisher install-and-restore receipts on both supported board
   families; and
6. closed and historical plans are cited as evidence rather than reopened as
   work queues.
