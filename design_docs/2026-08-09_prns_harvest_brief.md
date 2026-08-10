# Prns Harvest Brief

2026-08-09. Survey of [Prns](https://github.com/KenAKAFrosty/Prns), a ground-up
Rust Reticulum implementation, at exact commit
`72b6b30d27cac910ce20d370e1dc711fe9b95955` (v0.3.4). Prns is dual-licensed
MIT OR Apache-2.0 ("Copyright (c) 2026 The Prns Authors"). Mark endorsed
studying and harvesting it with attribution. Paths below are repo-relative to
that pinned Prns tree.

The pinned checkout passed `python validation/run.py verify` with 102
registered suites, 43 owned Cargo manifests, 18 Kani proofs, 8 fuzz targets,
and 70 oracle / interop / smoke assets. `prns-flash-manifest` passed 46 tests;
the `signed-artifact` feature's three canonical RNS 1.4.2 tests passed. Those
checks establish the inventory and the two focused donor seams. They do not
substitute for Prns's unrun hardware qualifications or Retinue's own receipts.

## License and provenance posture

Prns is permissively licensed, so reading and porting from it is ordinary
open-source reuse with attribution and notice obligations. The prohibition on
reading the Python RNS reference source remains unchanged. Retinue's declared
implementation provenance does change when a Prns-derived port lands: the
current retinue README, crate docs, and oracle policy list the public protocol
material, Beechat, and black-box wire observation as their implementation
inputs. They must add Prns for the affected seams rather than continue saying
"nothing else."

Mechanics for any ported code: record the chosen inbound license, add a
per-file header line ("Portions derived from Prns,
github.com/KenAKAFrosty/Prns, Copyright (c) 2026 The Prns Authors, MIT OR
Apache-2.0"), add a `THIRD_PARTY_NOTICES` entry in the consuming crate, and
update retinue's top-level provenance declarations. Prns's own
`THIRD_PARTY_NOTICES.md` (cargo-about, per-release graph, deduplicated by SPDX
id) is itself a model for retinue's release-time notice generation. Prns's
upstream provenance does not remove retinue's downstream review obligation.

Keep evidence labels exact. An untouched Prns executable is an independent
external peer. A vector, test, or implementation derived from Prns is
donor-conformance evidence in that seam, not an independent oracle.

Prns documents its constants against RNS reference names (`IC_BURST_FREQ`,
`PATHFINDER_M`, and so on with values and rationale). Those are protocol facts;
citing them is fine and their `wire/limits.rs` is now the best public
cross-check for our own
[wire format reference](2026-07-13_rns_wire_format_reference.md).

## Harvest inventory, ranked by seam cleanliness

### H1. Announce ingress admission (cleanest seam, code-portable)

`prns-core/src/routing/announce/interface_announce_limit/` and
`destination_announce_limit/`: small no_std state machines implementing RNS's
ingress control faithfully. Per-interface burst detection (age-keyed 3 Hz new /
10 Hz established, 15 s burst latch and penalty, held-announce drip release at
5 s minimum spacing) and per-destination limits (escalating `rate_violations`,
`blocked_until`). Every constant documented against its RNS reference name.
This is useful anti-flood admission and backpressure, but it is not FT1. FT1's
done-condition is outbound transmission-cost accounting plus an enforced
announce airtime cap. H1 is engine-agnostic: the tables are plain structs plus
verdict functions, and should land under its own ingress-admission receipt.

### H2. Airtime accounting, announce pacing, and interface policy (FT1 donor)

`prns-runtime/core/src/manifold/airtime.rs` keeps projected short and long
airtime windows. `prns-runtime/core/src/manifold/announce_pacer.rs` applies
`AnnounceBandwidthCap` from interface bitrate, queues announces in bounded
storage, releases them at the calculated cadence, and prefers lower-hop work
under pressure. These are the direct donors for FT1.

Retinue already owns the transmit gate: `tulle::AirtimeBudget` is the
sliding-window duty-cycle budget every protocol on the radio shares (its doc
comment already distinguishes EU duty-cycle from US 15.247 dwell regimes). The
FT1 harvest is therefore an extension, not an import: announce-specific pacing
(the pacer's bounded queue and lower-hop-first release) composes on top of
tulle's existing budget. Do not land a second airtime accountant beside the
one that already gates every transmission.

`prns-core/src/interfaces/policy/` supplies `AirtimeDutyCycle`,
`AnnounceRateLimit`, `AnnounceBandwidthCap`, `InterfaceGravity` (an opaque i64
route-preference weight), and the full RNS `InterfaceMode` set (Full /
PointToPoint / AccessPoint / Roaming / Boundary / Gateway / Internal) with
documented propagation semantics per mode. The mode vocabulary also feeds FT5:
the boundary/gateway hierarchy primitive the scaling doc calls underused is
implemented here and legal to read.

### H3. Bounded structures for the firmware tier (FS6 donor, gate still open)

- `prns-core/src/lemire_index/`: 408 production lines, 542 including tests at
  the pinned checkout. Open-addressed side index,
  Lemire reduction on already-uniform truncated hashes, right-walk probing,
  re-pack on removal, const-asserted headroom invariants, u16 slots with the
  top value as the empty marker. `Fixed` and `Heap` variants.
- The `StorageLayout` pattern: every table exists as `Fixed*` (const capacity)
  and `Heap*`; a board picks per-table capacities in one hand-written layout
  struct. Their T-Echo runs the same engine without an allocator, under a
  deliberately reduced layout: 8 tracked destinations, 32 link sessions, 16
  packet hashes, one channel, one resource assembly, and zero-capacity
  facilities where the board profile omits them.
- `prns-core/src/storage/impls/esp32s3.rs`: a shipped ESP32-S3 recipe generic
  over `allocator_api2::Allocator`, placing bulk (512-entry tables, window
  pools) in PSRAM. The `external-alloc` trick: fixed-capacity columns generic
  over the stable allocator-api backport, `Global` on host, PSRAM on the V4's
  own chip.

Retinue already has part of the FS6 receipt: a T114 accepted 40 valid announces,
plateaued at 32 peers with 8 counted refusals, and completed byte-exact traffic
through a concurrent flood. Prns contributes a better TTL, eviction, and
storage-layout model. Still open are Retinue's TTL/eviction implementation,
sustained memory measurement, and the on-metal claim that a transport node
keeps relaying under the flood. A Prns T-Echo result cannot close that T114
gate.

### H4. LoRa MAC: CSMA, fairness, and spectrum diagnostics

`prns-interfaces/impls/embassy/src/lora.rs` (1,713 lines) plus
`channel_access.rs`, `airtime_quantum.rs`, `transmit_queue.rs`. Two-slot DIFS
CSMA with an adaptive noise floor (32 samples, 20th percentile, 11 dB
interference margin, CCA ceiling -83 dBm), randomized fine ticket, final
IRQ+RSSI check before TX; airtime fairness where decoded peer airtime earns
bounded 1x-3x countdown acceleration; `AirtimePolicy::Regional` (policy may
tighten but never weaken a regional limit); split packets kept contiguous on
air for RNode interop. `LoRaSpectrumStatus::snapshot()` exposes channel-busy
per-mille, noise floor, deferrals, false preambles, contention timeouts, duty
holds: diagnostics-as-invariants done well. This is the MAC-layer "collision
avoidance" answer the
[collision mitigation notes](2026-07-24_lora_collision_mitigation_ideas.md)
chose over PHY survival, already built. Technique harvest first; a module port
is plausible later since it is generic over embedded-hal-async.

### H5. Validation hub patterns

The single most institution-shaped harvest. Survey findings:

- **Drift detection instead of duplication.** `validation/manifest.toml` never
  re-lists assertions: Kani proofs are re-discovered from `#[kani::proof]` in
  source, fuzz targets from the fuzz Cargo.toml, Cargo manifests from
  `git ls-files`, and `run.py verify` fails on any asymmetry in either
  direction. Orphan-asset check: every smoke script and oracle file must be a
  declared input of some suite or hold a reasoned, expiring exemption.
- **Evidence discipline.** Per-suite `result.json` against a strict JSON
  schema: commit SHA, worktree-clean checked before and after (a suite that
  dirties tracked files fails), resolved command, tool versions, timing. The
  release aggregate re-validates everything against an exact 40-char SHA.
- **Tiers.** `pr` (deterministic, bounded), `release` (exact-SHA: Kani, fuzz,
  sanitizers x3, Miri under both aliasing models, dependency audit),
  `scheduled` (long fuzz, coverage, mutation, hardware sims). Doctrine worth
  quoting: scheduled evidence cannot substitute for exact-SHA release
  evidence, and a skipped CI result is not green.
- **Mutation testing as audit, not gate.** Semantic fingerprints (SHA-256 over
  package/file/function/genre/replacement, line-numbers stripped), and the
  accepted-survivor set must exactly equal the current unresolved set: both
  untriaged mutants and stale waivers fail. Every waiver needs reason,
  reviewer, expiry.
- **Unsafe audit.** `validation/security/unsafe-audit.py` (stdlib-only,
  portable as-is): per-shipped-graph, per-target, per-feature unsafe counts
  across all 692 reachable packages, hand-rolled lexer with a self-test, and a
  built-in policy gate: first-party crates must `#![forbid(unsafe_code)]`
  except named exceptions, which must instead deny
  `unsafe_op_in_unsafe_fn` + `clippy::undocumented_unsafe_blocks`, resolved
  through workspace lint inheritance. Vendored code is deliberately not
  laundered. Byte-comparable snapshot with drift mode.
- **Fuzzing shape (FS1's model).** `engine_ingest_never_panics` drives the
  whole engine (`ingest_packet_into`) with deterministic injected entropy, not
  just the parser. Retinue's FS1 harness should copy this shape: fuzz the
  ingest path end to end, seeds checked in as immutable corpus copied to a
  writable dir at run time.
- **Hardware gates as fill-in procedures.** `validation/platforms/*.md`:
  equipment list, exact capture commands, numbered pass/fail checks, a literal
  evidence template. And `validation/lora-csma-qualification.md` gates an
  embedded scheduler change on a 128-byte reserved-RAM ceiling measured
  per-linker-section on a Heltec V4 and an nRF52840, with an explicit
  "hardware qualification: not yet run" section enumerating what remains
  unproven. That honesty format, and the RAM-delta gate, both transplant
  directly into retinue's acceptance-doc practice.

### H6. Flash manifest and flasher chain (linkboy / FS4)

- Two signed documents: a mutable `ChannelDescriptor` pointer (version +
  manifest URL + manifest sha256) and an immutable per-release
  `FlashManifest`, both minisign-signed, verified against a public key baked
  into the flasher by `include_str!` (with a test asserting LF endings so the
  byte-exact key comparison survives). All verification happens before any
  device is opened: channel sig, manifest sig, manifest hash against the
  descriptor, key-id match, HTTPS enforcement, bounded downloads, per-part
  sha256. The web flasher runs the same Rust verification compiled to wasm;
  JS handles transport only.
- Sparse parts (bootloader / partition table / application at separate
  offsets) with a preserved provisioning hole: updates never touch the config
  slot unless asked. `SourceArchiveIdentity` ties a release to its exact
  source archive by route, checksum route, byte length, and SHA-256. That is a
  useful artifact-identity shape, but it is not by itself a GPLv3 corresponding
  source offer. Linkboy must still retain license, source revision,
  corresponding-source location, build material, and offer facts.
- Minisign convergence: luggage already chose minisign manifests, and Prns
  independently landed on the same signature mechanism for flashing. Prns's
  pinned key proves upstream publisher custody. The signed Merely package index
  remains the authority that admits a network-delivered package; Linkboy
  verifies the authorized package, constructs the immutable device-specific
  plan, executes it, and owns recovery and receipts. Do not create a competing
  Linkboy-global trust root.
- Release custody process (`release/flash/README.md`): offline maintainer key
  with the CI copy confined to a protected environment; two independent
  reproducible builds byte-compared before signing; sign-without-rebuild;
  Sigstore attestation as supplemental, never replacing the pinned key;
  SHA-pinned third-party actions; rollback via retained complete candidate
  bundles. FS4's process half, written down and runnable.

### H7. RNS signed artifacts as a carrier, not an authorization policy

`prns-core/src/identity/signed_artifact.rs`, behind the `signed-artifact`
allocation feature, creates and validates the RNS RSG/RSM envelope: signer
identity hash and public key, SHA-256 message binding, optional embedded
message and metadata, and Ed25519 signature. Its tests reproduce the canonical
RNS 1.4.2 vectors exactly.

This is useful for signed service descriptions, invitations, distribution
records, and carrying an immutable firmware manifest over Reticulum. It may
also carry an FS2 command payload. It does not supply FS2's monotonic counter,
target class, expiry, command key id, payload semantics, replay ledger, or
authorization decision. Those remain Retinue policy. It likewise does not
replace Minisign release custody, rollback protection, or the signed Merely
package index.

### H8. Prns as an independent interoperability peer

Before importing code, run the pinned Prns executable as the third corner of
the live matrix:

```text
Retinue <-> stock RNS 1.4.2
Retinue <-> pinned Prns
pinned Prns <-> stock RNS 1.4.2
```

Keep this as a black-box process boundary with exact versions, commands, and
captured results. Once a Retinue seam is derived from Prns, agreement in that
seam is donor-conformance evidence. The untouched process still remains useful
as a mixed-network participant and regression peer.

### H9. Official Hopspot as Linkboy's first upstream package and field gateway

Prns's signed Heltec V4 Hopspot release is the strongest first F7 package. It
forces Linkboy to represent ordered sparse artifacts with individual hashes and
offsets while preserving the provisioning slot; the current Linkboy package
binds one payload. The acceptance receipt is install official Hopspot, exercise
its expected interface, then restore Retinue through the same graphical flow.
That proves a second publisher and a real firmware choice rather than merely
copying another project's schema.

The shipping V4 application is also a concrete field-gateway donor: LoRa plus
SoftAP/station TCP rendezvous, captive DNS/HTTP, USB, ESP-NOW, and a selectable
BLE boot mode. It is a small Reticulum ingress appliance, not a general internet
hotspot. A later browser rendezvous consumer belongs with Turnstone. Treat that
as a product integration decision, not as a reason to move routing, flashing,
or trust authority into Turnstone.

Board identity stays exact. Prns ships V4/V4 R8, T-Beam Supreme, XIAO ESP32-C6,
and T-Echo targets. T-Echo is an nRF52840/SX1262 and UF2 donor for T114 work; it
is not a T114 receipt.

### H10. Embedded practice nuggets (retinue's own boards)

- Hand-written SX126x driver (1,185 lines, embedded-hal-async generic, no
  lora-phy): config hoisted out of the per-packet path because the SX1262
  retains registers across standby/TX/RX, verified on hardware. Heltec V4
  runtime FEM detection: GPIO2 CSD read distinguishes KCT8103L vs GC1109 and
  selects RX gain and FEM-switch GPIO (5 vs 46). TCXO 1.8 V on the V4, 1.6 V
  on LilyGo boards. Directly applicable to tulle/selvage on the same silicon.
- nRF52840 + SoftDevice landmine: `evt-max-size-512` is mandatory once
  att_mtu is 247, or the SoftDevice panics on the first full-MTU write.
  Identity flash writes must use raw Nvmc before `Softdevice::enable()`.
- ESP32-S3 dual-core split: engine alone on core 1, all I/O on core 0, RWDT
  fed by core 0 but gated on a core-1 heartbeat atomic; watchdogs disabled
  during boot because zeroing PSRAM columns overruns the timeout. Boot-phase
  breadcrumb: a 27-stage enum in RTC-fast persistent RAM, printed on next
  boot. Cheap, copyable, and exactly the loud-divergence diagnostics retinue
  favors.
- PSRAM discipline: register PSRAM first so boot allocations land external;
  reclaim the documented-unused 32 KiB DCache window; A/B page pattern with
  generation counters and a commit word for the radio profile, and a flash
  layout locked by const adjacency asserts plus a test that parses the real
  partition CSVs.
- Live LoRa profile apply without reboot (`LORA_CONTROL.apply`) next to
  reboot-based BLE/AP mode switching (RTC persistent word + software reset,
  because those two cannot coexist on the S3 radio). Precedent for
  murmuration CM1: profile-level retune is hot today in shipping firmware;
  what reboots is stack coexistence, not the radio.

## Cautionary findings (study, do not copy)

- **Cleartext secrets on flash, by design.** The identity vault stores the
  raw 32-byte secret plus its bitwise complement (corruption check only); the
  provisioning slot stores the WiFi PSK in cleartext; the persistence journal
  stores self-ratchet snapshots. No flash encryption, no secure boot anywhere
  in the tree. Their device is a full sovereign node, so seizure means
  identity loss, which is the exact posture the
  [field node security posture](2026-08-09_field_node_security_posture.md)
  rejects for retinue's field tier. Prns is the honest counterexample that
  makes the seizure paragraph worth its cost.
- **Upstream security finding under coordinated disclosure.** The pinned tree
  contains a reproducible embedded entropy issue that may affect cryptographic
  operations. Keep the affected board, source path, reproduction, and impact
  analysis in a private disclosure record; do not publish them in this brief
  before the Prns maintainer has received a report through `SECURITY.md` and a
  disclosure state is recorded. Retinue keeps its hardware RNG live and must
  not copy the affected pattern.
- **Process vs practice.** The elaborate acceptance matrix shipped 0.3.4
  under a recorded maintainer override waiving physical acceptance. The
  custody design is still worth harvesting; the receipt discipline of
  recording the waiver is itself worth harvesting.

## Execution lanes

The harvest is splittable. Its execution authority is the
[Retinue work lanes](2026-08-09_retinue_work_lanes.md), which audits the other
plans and keeps shared seams from colliding. All lanes first pin the source,
record donor provenance, preserve an untouched Prns executable, and move the
security finding into private coordinated disclosure.

| Lane | Prns harvests | Immediate sequence |
| --- | --- | --- |
| **Peer** | H8 | Run and capture all three pairings before donor work touches the same seam; cross-check O-10. |
| **Air** | H1-H4, H10 | Extend `tulle::AirtimeBudget` for FT1, add ingress admission, finish firmware FT2/FS6, then close CM1. |
| **Assurance** | H5, H7 | Establish the validation minimum, whole-ingest fuzzing, and RSG/RSM vectors before settling FS2 and FS3. |
| **Distribution** | H6, H9 | Sparse signed packages and the V4 Linkboy install/restore landed; settle F5, close graphical G4 and cross-firmware recovery, then admit a T114 upstream. |

The Air lane keeps Retinue's existing airtime budget authoritative; it does not
import Prns's `AirtimeLedger` as a parallel accountant. It also preserves the
existing T114 flood receipt and extends it with expiry, eviction, sustained
memory, and relay evidence.

The Distribution lane cannot close F7 with Hopspot alone because Prns does not
ship a T114 target. A separate official T114 upstream package is required. The
Peer lane does not import donor code, and the Assurance lane owns the central
validation registry to prevent concurrent edits from turning it into a second
test authority.

Later Air work reads `prns-runtime`'s manifold/wake-scheduling model before the
murmuration timer design and evaluates whether `warmth`/`departed-interface`
grace belongs in FT3. After the interop and upstream-package receipts, reassess
whether Retinue's independent wire engine earns its maintenance through policy,
footprint, or protocol experimentation. This brief does not settle that choice
in advance.
