# Linkboy public flashing plan

**Date:** 2026-08-08  
**Status:** F1-F4 software slices landed; physical/public acceptance remains
open where each gate requires it. F5 is complete with official per-platform V4
helper custody, cross-platform V4 physical receipts, and the public T114 UF2
real-device receipt. On
2026-08-14 Signalman installed
the admitted Prns Hopspot V4 package, completed its required 115200-baud
self-check, and restored Retinue to terminal `Complete` on the same N39 V4.2.
The graphical T114 path also installed the admitted Meshtastic UF2 package and
restored Retinue. F7's physical cross-firmware proof is therefore complete on
both supported board families.
On 2026-08-15 a Windows V4 staging build resolved its pinned `espflash` helper
from its own `helpers/windows-x86_64` directory, without helper or catalog
environment overrides, installed Hopspot, and restored Retinue on COM6. That
older stage was not a public F5 completion because it deliberately excluded
T114. On 2026-08-19 the public route changed: V4 manifests pin official
per-platform `espflash 4.5.0` artifacts, and T114 uses Linkboy's built-in stock
bootloader UF2 writer. The full Windows stage and headed preflight are recorded
in `2026-08-19_signalman_public_f5_windows_receipt.md`; its physical T114 leg
returned `Complete` with the expected application identity. That same Windows
stage completed the V4 physical loop on O-PC.
Standalone staged Linkboy completed the V4 physical
flash, Hopspot self-check, and Retinue recovery on Intel macOS, Apple-silicon
macOS, and Linux; see `2026-08-19_linkboy_f5_macos_linux_v4_receipt.md`.
Those Linkboy receipts are not a headed Signalman acceptance. The owner has
supplied the Windows screen-reader quality judgement.
Repeated T114 CDC sessions still expose a separate Windows semaphore-timeout
defect. Sidequests remain unstarted unless a later receipt says otherwise.

## Execution list, 2026-08-12

### Active: F3a first-flash preflight

- [x] Keep package parsing, compatibility, planning, execution, and receipts in
  Linkboy.
- [x] Let an owner-selected silent V4 enter the non-writing ESP ROM inspection
  path; require processor, flash-size, and bootloader facts before planning.
- [x] Capture a mounted T114's `INFO_UF2.TXT` facts and retain them for a later
  serial-DFU restore.
- [x] Restore Retinue to the physical T114 through the immutable Linkboy
  package and verify the returned application. This is terminal/package
  evidence, not graphical evidence.
- [x] Offer explicit V4 and T114 family declarations for a selected silent
  serial device in Signalman desktop. The declaration remains owner input, not
  observed hardware evidence.
- [x] Add owner-flow and accessibility coverage for both declarations through
  the real desktop view without opening hardware in the tests.
- [x] Run the Linkboy and Signalman suites.
- [x] Produce a fresh Signalman desktop suite receipt. On 2026-08-14 the
  ordinary locked, offline standalone suite passed 18 tests: one library test,
  five accessibility tests, and twelve owner-flow tests.
- [x] Take the final post-fix T114 recovery receipt. The T114 preflight,
  Meshtastic UF2 transfer, Retinue restore, and terminal graphical `Complete`
  result are recorded in
  `2026-08-14_signalman_t114_graphical_receipt.md`.
- [x] Take the V4 graphical cross-firmware install, required Hopspot
  self-check, and Retinue restore receipt. See
  `2026-08-14_com6_n39_hopspot_signalman_graphical_receipt.md` and
  `2026-08-14_com6_n39_hopspot_retinue_signalman_restore_receipt.md`.
- [x] Take the manual screen-reader pass. The owner confirmed the staged Windows
  screen-reader flow on 2026-08-19. Repeated T114 CDC-session reliability is
  tracked separately from the completed recovery receipt.

F3a stops after those receipts. It does not absorb helper distribution, BLE,
OTA, browser flashing, fleet updates, or catalog promotion.

### Current trunk work

1. **F5, helper delivery: complete.** Official per-platform V4 helper custody,
   the built-in public T114 UF2 route, and the required physical receipts are
   recorded.
2. **F6, graphical acceptance: complete for supported Windows.** The packaged
   Signalman face retained the standalone Linkboy recovery door, passed the
   keyboard and owner screen-reader checks, and carried both physical routes
   through reconnect and returned-application verification.
3. **F7, public catalog projection: implementation ready.** The public index
   now carries package state and exact host receipt evidence, and Mer3ly retains
   a digest-checked copy to derive its firmware cards. Publication remains open
   until the paired Retinue and Mer3ly commits are published and the site is
   deployed.

### Sidequest queue after the trunk

1. Local Bluetooth flashing.
2. Authenticated over-link update with verified staging and rollback.
3. Browser installation as a convenience face over the same package and plan
   authority.
4. Additional board families and printed form factors.
5. Fleet campaigns after one-device recovery is routine.

## Outcome

Turn Linkboy's proven bench flasher into the safe owner-flashing engine beneath
a graphical application. The first public path is local and cabled. It must let
an owner take supported stock hardware, choose a known firmware package, inspect
what will change, flash it, and recover if the application does not return.

Linkboy remains the flashing authority. Its library discovers devices, checks
packages, produces a flash plan, executes that plan, and reports structured
events. The CLI remains one face. Signalman or another graphical host may become
a second face, but it must not duplicate flashing state or policy.

This is not an OTA plan disguised as an installer. Bluetooth, browser flashing,
and over-radio updates are sidequests with their own promotion conditions.

## Baseline

What exists now is real:

- `linkboy list` surveys serial ports and asks running Retinue firmware to name
  its board, region, and channel.
- `linkboy flash PORT IMAGE` sends a Heltec V4 through `espflash` and the ESP ROM
  loader.
- The same command sends a T114 into its bootloader, discovers its new port, and
  invokes `adafruit-nrfutil` serial DFU.
- Both routes ran end to end on physical boards. Linkboy's parsing and refusal
  tests are green.

The present boundary is equally concrete:

- a board must already run Retinue so the `status` probe can identify it;
- an image is checked only for existence;
- the transfer depends on helper programs installed on `PATH`;
- progress is terminal text rather than structured application state;
- completion means the helper exited successfully, not that the expected
  application returned and identified itself;
- image provenance, exact hardware compatibility, write ranges, state impact,
  source correspondence, and recovery are not represented.

That is a proven bench door, not yet a public first-flash path.

## Ownership

### Linkboy owns

- observed device facts and their provenance;
- firmware package parsing and integrity checks;
- compatibility and refusal decisions;
- the immutable flash plan shown before writing;
- bootloader entry, port or volume rediscovery, transfer, and post-flash
  verification;
- structured progress, warnings, failures, and recovery facts;
- a transaction receipt that contains no device secrets.

### Firmware targets own

- truthful `status` and `version` probes;
- bootloader-entry hooks where the running image can provide them;
- declared application and persistent-data ranges;
- region and radio limits at runtime;
- any future on-device verification and rollback mechanism.

### The graphical face owns

- device and firmware selection;
- confirmation and recovery presentation;
- accessibility, localization, and operating-system affordances;
- rendering Linkboy events without inventing success or hiding a refusal.

### Mer3ly owns

- the public device catalog and DIY instructions;
- projecting, without reclassifying, package state from Retinue's published
  package index;
- putting a purchase link after the complete instructions;
- linking public claims to Retinue receipts rather than copying their authority.

## Core model

The model starts concrete. It does not acquire a generic plugin system until a
third genuinely different flash route requires one.

### Device observation

`DeviceObservation` records facts and where each fact came from:

- current serial port or mounted volume;
- running-firmware status reply, when available;
- bootloader USB identifiers and descriptors;
- processor, flash size, and other facts reported by a supported loader;
- exact board and revision selected by the owner;
- whether the board is running Retinue, known upstream firmware, a bootloader,
  or something unknown;
- the confidence and contradiction state of the combined evidence.

A VID/PID or COM number is never a board identity by itself. A first flash may
need explicit owner confirmation because a generic bootloader often identifies
the processor rather than the carrier board. Linkboy can accept that fact and
record it; it must not silently upgrade it into certainty.

### Flash package

`FlashPackage` binds bytes to the facts needed to judge and explain a write:

- schema, package identifier, display name, version, and publisher;
- payload format, byte length, and SHA-256;
- supported board families and revisions;
- required processor, flash size, bootloader, and flash route;
- write ranges, preserved ranges, and known state impact;
- supported regions or the mechanism that applies regional limits;
- expected post-flash status and version facts;
- firmware license, notices, source revision, and corresponding-source link;
- origin URL and optional publisher signature.

The signed Merely package index supplies trust for network-delivered packages.
An upstream signature can be recorded when one exists, but the model does not
pretend every upstream project publishes one. A local expert package still
needs an explicit manifest and hash.

State impact is first-class. Switching firmware may preserve Retinue identity
and settings, replace them, or have an unknown effect. Unknown is a visible
warning that requires confirmation, not a quiet default.

### Flash plan

`plan_flash(observation, package)` is a pure decision. Its output contains:

- the exact observed device and selected package;
- facts that establish compatibility;
- facts supplied manually by the owner;
- the concrete route and helper requirements;
- bytes and address ranges that will be written;
- persistent state expected to survive, change, or become unknown;
- bootloader and post-flash rediscovery expectations;
- recovery instructions available before the write starts.

Contradiction, ambiguity, an unsupported board revision, a bad hash, overlapping
protected ranges, or a missing recovery route produces a `Refusal`, not a
warning that a UI can click past.

### Execution events

The executor emits data rather than printing:

```text
Inspecting
WaitingForOwnerAction
EnteringBootloader
Rediscovering
Erasing
Writing { written, total }
VerifyingTransfer
Rebooting
VerifyingApplication
Complete { receipt }
RecoveryRequired { facts, instructions }
Refused { reasons }
```

Cancellation is stage-aware. It is available before a destructive stage and
where a concrete transport documents safe interruption. A graphical cancel
button must not imply safety during an indivisible erase or write.

## Trunk gates

### F0. Preserve the proven bench baseline

This gate is complete.

Receipts:

- V4 flashed through Linkboy and `espflash` on the physical bench;
- T114 entered DFU, re-enumerated, and flashed through Linkboy on the physical
  bench;
- all three attached boards surveyed correctly afterwards;
- five Linkboy unit tests pass.

Stop rule: later work must keep these concrete routes working. A new model is
not progress if the bench command stops working.

### F1. Package and plan before execution

Files:

- `apps/linkboy/src/package.rs`
- `apps/linkboy/src/device.rs`
- `apps/linkboy/src/plan.rs`
- `apps/linkboy/src/lib.rs`
- `firmware/packages/*.toml`

Work:

1. Add strict `FlashPackage`, `DeviceObservation`, `FlashPlan`, and `Refusal`
   types.
2. Add manifests for the current Retinue V4 and T114 artifacts.
3. Make package hashing and board/package compatibility pure and exhaustively
   testable.
4. Record write ranges and persistence impact for both current images.
5. Add `linkboy inspect PACKAGE` and `linkboy plan DEVICE PACKAGE`.
6. Keep raw-image flashing only as a plainly named expert command. It must not
   be the path the future graphical face calls.

Done conditions:

- a package with one changed byte is refused;
- V4 and T114 packages are mutually refused on the wrong board;
- unsupported revisions and conflicting evidence are refused;
- protected-range overlap is refused;
- each plan explains data impact and recovery before execution;
- no serial port is opened by package parsing or planning tests.

### F2. Structured execution

Files:

- `apps/linkboy/src/executor.rs`
- `apps/linkboy/src/route/esp_rom.rs`
- `apps/linkboy/src/route/adafruit_dfu.rs`
- `apps/linkboy/src/main.rs`

Work:

1. Move process invocation and port rediscovery behind an executor that emits
   `FlashEvent` values.
2. Keep `espflash` and `adafruit-nrfutil` as the first concrete route adapters.
3. Parse useful progress when the helper exposes it and preserve the helper's
   complete diagnostic output on failure.
4. Add an injected process and device runner for deterministic tests.
5. Make the CLI a renderer of plans and events rather than the owner of flash
   decisions.

Done conditions:

- simulated success, missing helper, helper failure, disappearing device,
  unexpected new port, timeout, and post-write silence have tests;
- the T114 route selects the newly appeared bootloader port rather than a fixed
  COM number;
- no event reports `Complete` before application verification;
- both physical cable routes pass again through the structured executor.

### F3. First-flash device evidence

Files:

- `apps/linkboy/src/discovery.rs`
- route-specific discovery modules under `apps/linkboy/src/route/`
- fixtures under `apps/linkboy/tests/fixtures/`

Work:

1. Discover Retinue devices by their existing status probes.
2. Discover bootloaders and processors through route-specific facts.
3. Add explicit exact-board confirmation for cases where the loader cannot name
   the carrier or revision.
4. Add a contradiction check between selected board, observed processor, flash
   size, loader, and package.
5. Support a board that arrives with stock or unknown firmware and therefore
   cannot answer a Retinue probe.

The first-flash flow is allowed to say, "I can prove this is an ESP32-S3 with
this flash size; please confirm that the label says Heltec V4.2." It is not
allowed to say, "COM7 means V4."

Done conditions:

- an as-shipped V4 and an as-shipped T114 can reach a valid plan without first
  running Retinue;
- choosing the wrong physical model produces a refusal when observed hardware
  facts contradict it;
- an indistinguishable carrier requires an explicit recorded confirmation;
- unplug, reset, and re-enumeration do not transfer identity to another device.

### F4. Verification and recovery

Files:

- firmware status/version probes for both targets;
- `apps/linkboy/src/verify.rs`
- `apps/linkboy/src/receipt.rs`
- public recovery instructions beside each package manifest.

Work:

1. Give each Retinue image a stable version or build identifier that Linkboy can
   query after reboot.
2. Rediscover the application after the helper exits.
3. Match its board, version, region state, and expected channel capabilities to
   the package.
4. Save a local receipt containing package hash, observed hardware facts,
   route, stages, and result. Never include identity keys or message content.
5. Provide recovery instructions before the write and preserve them after any
   failure.
6. Exercise recovery from an interrupted or non-returning application on each
   target without relying on the application firmware itself.

Done conditions:

- helper success followed by the wrong application is a failure;
- helper success followed by silence is `RecoveryRequired`;
- both boards can be recovered through hardware bootloader entry;
- identity/settings preservation claims are verified by before-and-after facts;
- receipts can be exported for support without exposing secrets.

### F5. Remove public PATH dependencies

**Complete 2026-08-19.**

The structured routes deliberately land before packaging policy. Then one
measured spike chooses, per route, between a supported library API, a pinned
bundled helper, or a simpler native operating-system path such as UF2.
`2026-08-14_linkboy_f5_windows_custody_receipt.md` records the present local
Windows executables and digests without promoting them to shipped custody.
`2026-08-15_signalman_windows_v4_staged_helper_receipt.md` adds a physical
Windows V4 stage: its executable resolved `espflash` from the adjacent,
digest-checked helper directory, then flashed and recovered the physical N39.
Ambient `PATH` lookup now requires the explicit development-only
`LINKBOY_ALLOW_PATH_HELPERS=1` setting.

The 2026-08-19 public route admits official `espflash 4.5.0` release artifacts
for Windows x86-64, macOS Arm and x86-64, and Linux Arm and x86-64. Every
platform entry records both the extracted executable digest and retained archive
digest and URL. Linkboy refuses an external-helper plan when the current
platform has no admitted artifact. The public T114 route uses a deterministic,
application-only UF2 and Linkboy's built-in volume writer, so owners do not need
Python or `adafruit-nrfutil`. The serial DFU path remains an expert recovery
route outside the public catalog. The reproducible full-catalog Windows stage
and its current receipt live in
`2026-08-19_signalman_public_f5_windows_receipt.md`.

`2026-08-19_linkboy_f5_macos_linux_v4_receipt.md` supplies the V4 physical
route evidence for Intel macOS, Apple-silicon macOS, and Linux. The stage is
Linkboy-only because the current native Signalman desktop build stops in the
separate Netrender `wgpu` 29/30 dependency split. The full-catalog Windows
receipt supplies the matching official-helper V4 loop and the graphical public
T114 UF2 real-device loop. Together those receipts close F5 without weakening
F6's separate headed-flow boundary.

Decision criteria:

- reproducible packaging for Windows, macOS, and Linux;
- clear license and source obligations;
- usable progress and diagnostics;
- recovery behavior;
- maintainable compatibility with the relevant bootloader;
- no silent download or execution of an unpinned binary.

Done conditions:

- the public build does not tell an owner to install Python, Cargo, `espflash`,
  or `adafruit-nrfutil` separately;
- every shipped helper or library is version-pinned and represented in the
  application notices;
- Windows, macOS, and Linux each have a real-device flash and recovery receipt
  before the application is called cross-platform.

### F6. Graphical owner flow

The first graphical consumer renders Linkboy's library. Its pages are concrete:

1. **Choose device**
2. **Choose firmware**
3. **Review changes**
4. **Prepare the device**
5. **Install**
6. **Verify or recover**

The default firmware list comes from the signed package index. Local packages
live behind an expert action and receive the same compatibility checks. The
review page shows publisher, version, hash, license, source, board revision,
write route, state impact, and recovery path.

Signalman is the natural radio-management host for this face, but that choice
is made only after Linkboy's engine is usable without a UI. Whether the face is
inside Signalman or a dedicated Linkboy application, Linkboy events remain the
single authority.

Done conditions:

- the UI cannot construct or mutate an already-approved `FlashPlan`;
- every refusal visible in the CLI is visible in the GUI;
- reconnecting a device does not lose recovery context;
- keyboard-only operation and screen-reader labels cover the complete flow;
- an owner can complete and recover both physical routes without a terminal.

### F7. Firmware choice and public catalog

Start with the two Retinue packages, then admit upstream firmware one package at
a time. A supported upstream package needs the same board constraints, hash,
state-impact statement, license/source links, and recovery receipt as ours.
"Upstream has a flasher" is useful evidence, not a Merely acceptance receipt.

The first interoperability proof should install one official upstream image on
each board, then restore Retinue through the same graphical flow. This proves
that firmware choice is real rather than a Retinue-only reinstall button.

Mer3ly consumes a published package-index artifact only after Linkboy is its
first real consumer. The site may then derive firmware availability and recipe
state while retaining its own catalog wording and sale policy.

Done conditions:

- at least one official upstream image and one Retinue image are installed and
  restored through the same UI on each supported board;
- package metadata and public instructions agree;
- the site links to complete DIY and recovery instructions;
- a purchase link, when separately authorized, remains after those instructions;
- catalog status does not advance to `proven-recipe` until public installer and
  recovery receipts exist.

## Sidequests

Sidequests are deliberately useful. None may weaken a trunk refusal or become a
condition for the first cabled release.

### S1. T114 UF2 route

Add mounted-volume discovery, UF2 package validation, copy progress, eject/reset
handling, and application rediscovery. Promote it to the default T114 route if
it proves simpler across all three desktop systems and retains a reliable serial
DFU recovery path.

### S2. Local Bluetooth flashing

Treat BLE as another local transport, not as generic OTA. It needs authenticated
device selection, an authoritative bootloader or application update protocol,
image verification, progress, reconnect handling, and cable recovery. It is not
promoted because Bluetooth pairing alone works.

### S3. Over-link firmware update

This earns the other half of Linkboy's name. It requires:

- an authenticated management destination distinct from ordinary messages;
- signed package authorization and replay protection;
- bounded, resumable chunks with whole-image verification;
- explicit airtime and battery budgets;
- an inactive image slot or equivalent safe staging area;
- atomic activation, boot health reporting, and automatic rollback;
- cable recovery after power loss at every stage;
- per-target memory-layout proofs rather than one universal OTA claim.

The V4 and T114 may need different update designs. A/B is not asserted merely
because the V4 has more flash, and the current settings A/B record is not an
application rollback system.

### S4. Authenticated remote listener-policy updates

The resident executive supersedes boot-selected channel switching. Remote
management may change the enabled ReceiveProfiles, adapter participation
levels, lease policy, and coverage assignment. It does not put the board into a
durable Sennet, Tucket, or RNode mode. Such updates need FS2 authorization,
FS3 replay persistence, an acknowledgement before apply, a bounded fallback
when the new listening plan cannot be reached, and an audit event. RNode host
control remains an explicitly exclusive compatibility mode.

### S5. Browser installer

A browser face can reuse package and plan semantics through Web Serial, WebUSB,
or a native bridge. It remains a convenience surface because those APIs do not
cover all browsers or mobile systems. The native desktop flow stays the recovery
authority.

### S6. More boards and form factors

RAK and later catalog devices enter through exact manifests, real stock-device
discovery, one flash route, one recovery route, and a hardware receipt. Do not
generalize from processor family to carrier-board support.

### S7. Fleet campaigns

Updating several owned radios needs serial execution by default, explicit device
identity, per-device plans, resumability, and independent receipts. It follows a
boringly reliable single-device path. Parallel flashing is not the first proof.

## Acceptance matrix

Each promoted device and route gets one checked row:

| Fact | V4 ESP ROM | T114 serial DFU | Optional route |
| --- | --- | --- | --- |
| Stock-device discovery | required | required | required |
| Wrong-package refusal | required | required | required |
| Hash failure refusal | required | required | required |
| State-impact statement | required | required | required |
| First flash | physical receipt | physical receipt | physical receipt |
| Post-flash identity | physical receipt | physical receipt | physical receipt |
| Interrupted-write recovery | physical receipt | physical receipt | physical receipt |
| Restore upstream firmware | physical receipt | physical receipt | physical receipt |
| Windows | required | required | required before claim |
| macOS | required before cross-platform claim | required before claim | required before claim |
| Linux | required before cross-platform claim | required before claim | required before claim |

## Global stop rules

- Do not write before a package and device produce an immutable accepted plan.
- Do not infer a carrier board from a COM number, VID/PID, or processor alone.
- Do not call a helper's zero exit status application verification.
- Do not describe local channel selection as OTA firmware replacement.
- Do not make Bluetooth or a browser the only recovery route.
- Do not distribute an image without integrity, license, notices, and source
  correspondence represented in its package.
- Do not claim persistent state survives an upstream image unless the exact
  before-and-after path proves it.
- Do not publish a generic flasher extension interface before a third concrete
  route demonstrates the shared boundary.
- Do not call the application cross-platform until each named desktop system has
  a real-device flash and recovery receipt.
- Do not let this lane re-center Retinue on protocol parity. Firmware choice is
  a product capability; Reticulum and the native Rust node remain the trunk.
