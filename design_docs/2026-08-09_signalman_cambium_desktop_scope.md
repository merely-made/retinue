# Cambium adoption and upgrade scope for Signalman

**Date:** 2026-08-09  
**Status:** G0 through G5 are complete for the supported Windows route.
Signalman pins the G5 host at an immutable Genet revision; Woodshed
intentionally tracks the same moving Genet reference as Mere and records its
resolved revision matrix in the lockfile. G4 has physical V4 and T114 receipts,
and the owner supplied the manual screen-reader judgement. Consumer order is
woodshed first, Signalman second, Pelt optional.

## Decision

Use Cambium for Signalman's native desktop face. Do not put a GUI in Linkboy
and do not make Retinue invent a second component toolkit.

This is a Cambium adoption and upgrade plan. Cambium is the GUI toolkit;
Signalman is the first Retinue installer integration used to expose and
prioritize real toolkit gaps.

Cambium is currently a Genet-native application toolkit, not a proven universal
toolkit for every Merely application. Its portable assets are Meristem's
reactive model and the component interaction contracts. Its DOM, layout, paint,
and native-host path are deliberately Genet-specific. Keep that distinction.

The first work is therefore two connected but separate tracks:

1. Genet gets one small, private, single-root Cambium desktop host. It closes
   the repeated native event-loop, layout, paint, hit-test, and accessibility
   seam without pulling product policy into the toolkit.
2. Signalman gets a `signalman-desktop` application that renders Linkboy's
   existing owner flow. It is the first real product consumer of that host.

The host does not become a public or stack-wide API until a second real
consumer uses the same boundary without product-specific additions.

**Corrected 2026-08-09.** That second consumer was written here as "initially a
simple Pelt surface", and G3's own audit note then found the redundancy is
wider than Pelt and named woodshed-genet as the simplest first dedup target.
The two halves disagreed. Woodshed wins: it is a real product that was the
host's donor, so migrating it deletes live duplication instead of adding a new
example, and it exercises the boundary as a whole application rather than as a
surface. **Woodshed is consumer one and Signalman is consumer two; Pelt is an
optional later migration, not a promotion prerequisite.** Both consumers landed
on 2026-08-09; see G3.

## Live baseline

The useful pieces already exist:

- `cambium 0.3.3` owns application view composition and the retained
  `GenetAppRunner`.
- `cambium-winit 0.3.0` deliberately maps only native input into Cambium key,
  IME, modifier, and wheel vocabulary.
- `cambium-winit-a11y 0.3.0` is private and already projects Genet layout and
  Sprigging semantics into AccessKit, but does not own a whole application
  window or event loop.
- Genet's `RenderCore` and `SurfaceHost` provide target and winit surface
  plumbing, while applications still own composition and input routing.
- Pelt assembles those pieces itself, and so do woodshed-genet, cleromancy,
  isometry, and turnstone's panes. **The extraction's donor turned out to be
  woodshed-genet, not Pelt** (corrected 2026-08-09): it is the simplest
  single-root assembler of the set, and it became the host's first consumer.
  None of them is a dependency of Signalman.
- Signalman has a semantic `FirmwareInstaller` and `FirmwareView` over
  Linkboy's verified catalog, immutable plan, structured events, recovery
  state, and receipt facts. That is the correct GUI boundary.

There is one documentation defect to close before any release claim:
`components/cambium/docs/genet-compatibility.md` still describes Cambium
0.3.0 and `cambium-winit` 0.2.0 as the published line. The live manifests say
0.3.2 and 0.3.0 respectively. The compatibility document and changelog are
not release authority until reconciled with package inspection.

Package inspection (2026-08-09) makes it a three-way disagreement: crates.io
has `cambium` current at 0.3.2 (the compatibility doc is stale about it), but
`cambium-winit` exists there only as 0.1.0 plus a **yanked** 0.2.0, so the
document's deliberately held 0.2.0 line is not installable either. Until the
three sources agree, the private host route in the packaging section is the
only honest delivery story.

## Ownership

| Layer | Owns | Must not own |
| --- | --- | --- |
| Linkboy | device observations, package verification, compatibility, immutable plans, transfer, verification, recovery, receipts | widgets, view state, native windows |
| Signalman | owner-facing vocabulary and projections of Linkboy state | a second flash planner or hardware policy |
| Signalman desktop | selection, page state, presentation of every refusal and event, background-work coordination | package trust decisions, raw flashing commands |
| Cambium | controls, retained view composition, DOM event handling, focus | product workflow policy or device state |
| Cambium desktop host | native lifecycle, input routing, layout, paint, hit testing, accessibility synchronization | Tokio, Linkboy, terminal commands, product actions |
| Genet | DOM, style, layout, rendering, native platform seams | a Cambium dependency |

The data route is intentionally one-way at the authority boundary:

```text
native input -> Cambium desktop host -> GenetAppRunner -> Signalman desktop action
                                                         -> Signalman projection
                                                         -> Linkboy owner flow

Linkboy event / result -> Signalman desktop state -> GenetAppRunner -> rendered DOM and a11y tree
```

Linkboy remains the only layer that can create or execute a flash plan. The
desktop application renders plan facts; it never reconstitutes them from form
inputs.

## Packaging decision

Do not add a `path = ../../../genet/...` dependency to a normal Retinue
workspace member. That would make a Signalman build accidentally depend on a
particular sibling checkout and would not be a usable application boundary.

Use this staged delivery:

1. Create the host as a private Genet workspace package. It may depend on
   unpublished Genet engine packages and `cambium-winit-a11y`.
2. Prove it with a private, `publish = false` Signalman desktop package pinned
   to an exact Genet revision or a released package set. Local development may
   override that pin in a documented developer-only configuration.
3. Before a generally installable Signalman desktop package, either publish the
   necessary Genet host seams with compatible versions or distribute a locked
   Genet runtime bundle. Do not claim a crates.io installation path until
   `cargo package` and a clean dependency resolution prove it.

The first implementation may use the exact-revision private route. It must not
quietly convert Retinue's default workspace build into a cross-checkout build.

## Work lanes

### G0. Reconcile Cambium's release record

**Genet seams**

- `components/cambium/docs/genet-compatibility.md`
- `components/cambium/CHANGELOG.md`
- `components/cambium/{cambium,cambium-winit,cambium-winit-a11y}/Cargo.toml`

Record the actual versions, publication flags, dependency source, and intended
install story. Inspect package metadata and perform the relevant clean package
or resolution check before changing release wording.

**Done when:** the docs match the manifests and a reader can tell which pieces
are public, private, or blocked by unpublished Genet dependencies.

**Stop rule:** if the public package graph does not resolve cleanly, retain the
private host route. Do not make a release repair part of the Signalman UI slice.

**Progress 2026-08-09: the record half is done.** Full registry survey plus
manifest inspection, reconciled into `genet-compatibility.md` (new
source-vs-registry table) and a CHANGELOG `Unreleased` entry. Findings:
meristem 0.1.1, sprigging 0.2.1, cambium 0.3.2, and cambium-nematic 0.3.1 are
all current on crates.io; `cambium-winit`'s hold rationale was dead (the
genet-layout dependency moved to `cambium-winit-a11y` on 2026-07-26, its own
manifest says so), its registry line was 0.1.0-installable with 0.2.0 yanked;
`cambium-winit-a11y` is never-publishable by design.

**Closed 2026-08-09, same day, owner-authorized.** Publishing surfaced one
more defect first: local `cambium` had grown the IME composition surface
after 0.3.2 shipped, so `cambium-winit`'s package verification failed against
the registry. `cambium` 0.3.3 published that state under its own number, then
`cambium-winit` 0.3.0 published and verified against it. **The registry graph
now resolves the input-mapped Cambium stack for the first time.** The private
host route remains correct for the desktop host itself (its a11y half rides
never-publishable Genet packages), but the packaging section's "released
package set" option is now real for the input-mapped layer.

### G1. Extract a single-root Cambium desktop host

**Proposed Genet package:**
`components/cambium/cambium-genet-winit-host` (`publish = false` initially).

This name is intentionally literal. It distinguishes the host from
`cambium-winit`, which must remain the small, published input-mapping package
with its existing dependency wall.

The host owns:

- `winit::ApplicationHandler` lifecycle, window creation, resize, DPI, redraw,
  close, and suspend/resume behavior;
- a `SurfaceHost`/`RenderCore` instance and one Cambium root;
- layout after rebuild, paint submission, and presentation;
- logical-coordinate hit testing before pointer dispatch;
- pointer capture, hover, wheel, keyboard, IME, focus traversal, and modifier
  routing into `GenetAppRunner`;
- `cambium-winit-a11y::A11yHost` synchronization and routing returned
  accessibility actions back through the retained DOM;
- a narrow app callback or action queue, so applications receive messages and
  decide their own state transitions.

It does not own an async runtime, a product `Application` trait, multi-window
policy, docking, navigation, persistence, or a generic command/task system.
Those abstractions have no second consumer yet.

Start with an explicit single-root constructor and action callback rather than
a universal application trait. Promote that API only after **woodshed and
Signalman** require the same hooks (see the corrected consumer order in the
Decision and G3).

**Receipts:**

- a deterministic retained-DOM test covering rebuild, focus, pointer capture,
  keyboard, and IME dispatch;
- a headed smoke example whose visible root redraws after resize and DPI change;
- an AccessKit projection/action regression through the existing a11y seam;
- a `genet-probe` semantic interaction receipt, not coordinate-only clicking.

**Stop rule:** if a consumer requires browser/tile/docking policy to consume the
host, that is evidence the host boundary is too high. Keep it experimental and
narrow it rather than importing that consumer's concepts.

**Progress 2026-08-09: the host exists.** `cambium-genet-winit-host` landed in
Genet (genet `246f0f1e7`, pushed), extracted from the woodshed-genet donor
rather than designed fresh: winit lifecycle with install-before-show
accessibility, SurfaceHost presentation, retained layout with scroll-plane
carry, the full paint pipeline with sprigging leaves and caret/selection
overlays, hit testing, hover/focus restyle and Enter/Leave routing,
click-to-caret with drag selection, visual caret movement, IME, wheel with the
shared scrollbar fade, and CSD edge-resize behind an option. Applications
supply plain closures (`HostHooks`: frame / after_dispatch / after_frame /
focused_text / key_intercept) with their own state in the closures'
environment; no application trait, exactly as scoped. `publish = false` (it
rides genet-layout and genet-winit-host), so consumers take it from genet.git
at a recorded immutable revision. Compiles clean with a headed smoke example
(`--example smoke`) proving the API from the consumer side. Receipts still
owed from the list above: the deterministic retained-DOM test, a headed run
record, the a11y regression, and the genet-probe semantic receipt. The
publication half of G0 also closed today: cambium 0.3.3 + cambium-winit 0.3.0
published after fixing the 0.3.2 version drift the first publish attempt
surfaced. Next moves: `signalman-desktop` (G2) can start on the host as-is;
the woodshed-genet migration is the named first dedup (its repo is clean;
isometry, cleromancy, and turnstone follow in their own windows).

**Closed 2026-08-09, later the same day.** The extraction at `246f0f1e7` had
four routing gaps and none of the receipts. Both are closed at genet
**`e4920aad6`**. Signalman's later tested consumer pin is `398e4af60`, which
also contains the accessible-label repair; `246f0f1e7` is never a consumer pin.

Routing: pointer Down/Move/Up now reach `on_pointer` with the *captured*
element's local coordinates, read from a new scroll-aware
`IncrementalLayout::painted_rect` (`absolute_rect` names an unscrolled box, so a
drag inside a scrolled list read a stale offset); wheel notches reach `on_wheel`
before the layout scrolls, and a handler's `prevent_default` suppresses that
default; Tab goes through `dispatch_key`, so a view can handle it and the
runner's own traversal is the cancellable default it was always meant to be;
the host's caret movement is recomputed from a pre-dispatch snapshot, so the
layout-aware visual move still wins over the field's logical one while the
field's handler genuinely runs first; AccessKit `Click` and `Focus` stay typed,
so a reader's virtual cursor no longer presses every control it passes over;
and `suspended` drops only the surface, which is why `HostOptions::netrender`
became a factory closure.

Receipts: a windowless `Harness` runs the same `Host` over the same routing, so
`tests/input_routing.rs` (10) and `tests/accessibility.rs` (5) exercise shipping
code rather than a copy. `about_to_wait`'s decision became `IdlePolicy`, so the
screen-reader wake is assertable end to end minus the OS adapter. The headed
smoke example self-drives a `genet-probe` scenario by role and label through a
new `HostPointer` queue and records in-process frame readbacks: 3 frames, 0
blank, 3 distinct digests, 2 distinct sizes across a resize. Full write-up in
genet `docs/2026-08-09_cambium_desktop_host_g1_receipt.md`.

**Two host defects the headed passes found afterwards**, both fixed:

- Injected text (`VK_PACKET`) was dropped, so nothing that types by injection —
  on-screen keyboards, remappers, assistive input tools — could type at all.
  genet `4c1474f9e`.
- Tab traversal is the wrong shape for a two-dimensional layout: woodshed's
  fretboard puts sixty focusable notes before its search field. **Holding Tab
  now steers focus with the arrow keys** over the laid-out geometry
  (`HostOptions::spatial_focus`, default on; a tap is unchanged). It belongs to
  the host because it needs the focusable set *and* the geometry, and no
  application has both. genet `a5e376c9a`.

**Two engine gaps found on the way**, both outside G1:

- Inline-level boxes share their line's fragment, so an inline-block `<button>`
  has no rect of its own — it cannot be resolved by a `genet-probe` selector
  and cannot be given accurate accessibility bounds. Every consumer already
  works around this by styling controls block-level; filed separately.
- The ARIA role map covered controls and tabs only, so `role="alert"` projected
  as a generic container and `role="progressbar"` announced with no value.
  Fixed in `e4920aad6` after signalman-desktop's accessibility receipt caught
  it.

### G2. Build Signalman's six-page owner flow

**Retinue package:** add a private `apps/signalman-desktop` package. It depends
on `signalman`, not on Linkboy internals, for presentation vocabulary. The
terminal `apps/signalman` binary remains buildable without a GUI runtime.

**Boundary correction 2026-08-12:** the delivered first face remains policy
safe, but it still names Linkboy presentation types and its worker calls
`execute_plan` directly. That does not meet the package boundary in the prior
paragraph. G5.2 moves the approved-install worker and its public update
vocabulary into Signalman; no desktop release claim is made before that cut.

The first desktop surface has exactly the public-flow pages already specified
for Linkboy:

1. Choose device
2. Choose firmware
3. Review changes
4. Prepare device
5. Install
6. Verify or recover

Every review field comes from `FirmwareReview`: package identity and version,
publisher, payload hash, license and source, origin, board revision, route and
helper provenance, write and preserved ranges, state impact, and recovery
instructions. Refusals and warnings remain visible states, never a disabled
button with missing explanation.

Use existing Cambium controls and DOM/CSS for selections, summary bodies,
warnings, progress, and recovery instructions. There is no demonstrated need
for a Sprigging leaf, a workflow-stepper component, or a new general-purpose
progress primitive. If a pattern later appears in a second product, promote it
through the component catalog with its semantics and interaction receipts.

The desktop process may run the blocking Linkboy executor on a dedicated worker
and send structured `FlashEvent` and final-result messages to the UI. It must
not move `GenetAppRunner`, DOM state, or view callbacks off the UI thread.
The worker is Signalman application code, not Cambium infrastructure.

**Retinue seams:**

- `Cargo.toml`: add the desktop package without changing default members until
  its Genet delivery dependency is reproducible.
- `apps/signalman/src/firmware.rs`: consume its existing projections first;
  add only minimal public action/event seams exposed by a real view gap.
- `apps/signalman-desktop/`: app state, Cambium views, CSS theme, host adapter,
  worker bridge, and tests.
- `design_docs/`: one executable receipt per hardware route after actual use.

**Receipts:**

- headless page-state tests prove selection, refusal text, immutable-plan review
  data, event progression, and recovery context;
- semantic tests activate controls by role/label and prove keyboard order;
- the application cannot construct, alter, or execute a `FlashPlan` except by
  invoking the owning Signalman/Linkboy flow;
- a manual screen-reader and keyboard pass records accessible names, focus
  order, and the visible transfer/recovery result.

**Progress 2026-08-09: built, with one receipt owed.** `apps/signalman-desktop`
exists, pinned to genet `398e4af60`. Full write-up in
`2026-08-09_signalman_desktop_g2_receipt.md`; the parts that change this plan:

**It is `exclude`d from the retinue workspace rather than a non-default
member.** Membership would still have forced this repository's root manifest to
carry genet's `[patch]` table (the vendored taffy / stylo_taffy / ipc-channel
forks — a patch does not inherit through a git dep), dragging the engine graph
into every ordinary retinue build including the MSRV job. Excluding it holds
the intent strictly: the default build, the lockfile, and CI are untouched.
`signalman` and `linkboy` still come in by relative path, which is not a
cross-checkout dependency.

Four seams were added to Signalman, each opened by a real view gap:
`survey_devices` + `DeviceCandidate`, `observe_device`, the projection helpers
(`refusal_lines` / `describe_event` / `event_progress`), and
`FirmwareInstaller::{receipt, plan, chosen_package}`.

Receipts done: 10 headless page-state and semantic-keyboard tests, 4
accessibility-projection tests, and a headed run that discovered this machine's
two real V4 boards and whose tree Windows' own UI Automation reads with every
control named, `aria-pressed` carried through, and focus reported.

**Live typing: resolved, and it was a real defect.** The first headed pass could
not type into the revision field. Tracing the host rather than guessing named
it: Windows delivers injected text as `VK_PACKET`, winit surfaces that as
`Key::Unidentified`, and the host dropped it. Not merely a test artifact —
on-screen keyboards, keyboard remappers, and assistive input tools all type
that way, so a person using one could not enter text into any Cambium
application. Fixed in genet `4c1474f9e` (`KeyPress` carries winit's `text`; an
unnamed key with text types as that character; a Ctrl/Super chord still does
not). "4.2" now types into the board-revision field by keyboard alone.

**The accessibility pass is now mostly automated.** "Needs a person" was too
broad: what the OS exposes is fully checkable, what a screen reader *announces*
is checkable with NVDA's testing driver, and only whether the announcements are
*good* is judgment. `testing/a11y-audit.ps1` does the first tier — it walks the
live UIA tree and asserts names, boxes, focus publication, tab order, and that
no focus stop is unnameable. It found four real defects on its first run
(signalman's nameless revision field, woodshed's glyph-named chrome buttons and
its unnamed drag surface, and an inline `<label>` costing the field its box),
all fixed. Both apps now audit **RESULT ok**.

**Owner judgement received 2026-08-19:** the owner confirmed that the screen
reader worked in the staged Windows pass. Together with the retained Narrator
traversal and automated UIA checks, that closes the manual quality prerequisite
for the supported Windows flow. It does not claim another operating system.

### G3. Prove the shared host before promoting it

Before the host is described as shared Cambium infrastructure, two real
consumers must use the same boundary without adding product-specific concepts
to it.

**Consumer one is woodshed-genet. Consumer two is signalman-desktop.** Pelt is
an optional later migration, not a promotion prerequisite. That ordering
replaces the original "a simple Pelt surface": the audit below found the
redundancy is wider than Pelt and named woodshed as the simplest first dedup,
and migrating a real product that was the host's own donor proves more than
adding a new example would — it deletes live duplication and exercises the
boundary as a whole application rather than as a surface.

**Audit note 2026-08-09: the redundancy is live and wider than Pelt.** The
host seam this plan extracts is hand-assembled today in at least four
products across three repositories: pelt's tile stack in-repo
(tile_surface.rs 1434 lines, tile_shell.rs 745), woodshed-genet
(main.rs 1679), cleromancy's native UI (native.rs 754 plus worker.rs 326),
and isometry-genet (host.rs 409 plus main.rs and input.rs), with turnstone's
panes riding the same runner. The external consumers already obtain the
cambium family from genet.git, so they can adopt a private `publish = false`
host package with no new source location. Their acceptance receipts still need
an immutable revision rather than a moving branch. Candidate
consumer #2 is therefore an existing assembler rather than a new Pelt
example, woodshed-genet looking simplest as a single-root app; that proves
the same boundary while deleting live duplication. Also note the extraction
does not start from zero: `genet-winit-host` already exists as the
platform-adapter component (`publish = false`), and the a11y crate already
composes it with `genet-layout`; G1 is the single-root composition over
those seams, not a new platform layer.

Promotion requires both consumers to use the same:

- root creation and redraw/resize lifecycle;
- native input and retained-DOM dispatch path;
- layout/paint/presentation seam;
- AccessKit synchronization and action dispatch;
- test harness for semantic interaction.

Only then choose whether to stabilize an application trait or publish a host
package. Meristem and the existing component catalog remain the cross-stack
starting point before that proof; a Serval or Xilem application does not gain
Cambium as a dependency merely for conceptual consistency.

**Progress 2026-08-09: both consumers landed.**

Woodshed's whole native-host assembly is gone: `main.rs` went from 1728 lines
to 211, and `genet-layout`, `genet-winit-host`, `netrender`, `paint_list_*`,
`cambium-winit`, and `cambium-winit-a11y` are no longer named by that
workspace. Both of its semantic scenario receipts (p4a occurrence identity, p4b
typed relations) pass unchanged, captures included. Write-up in woodshed
`design_docs/2026-08-09_cambium_desktop_host_migration.md`.

Signalman's desktop face is the second, built on the same boundary from
nothing. Between them the five criteria are all met by both — same `run` +
`HostOptions` lifecycle, entirely host-owned input and dispatch, the same
`relayout`/paint seam, the same install-before-show AccessKit sync with typed
Click/Focus routing, and the same semantic test harness (woodshed through
`genet-probe` scenarios, Signalman through the headless `Harness`, both over
`HostPointer`).

**The host needed one new API across both, and it is a routing seam rather than
product policy:** `HostPointer`, the queue by which an application asks the
host to deliver a pointer event to itself through the host's own hit test,
capture, and dispatch. A self-driving application must not re-roll that
routing, and both consumers need it for exactly that reason. Neither needed an
application trait, multi-window, docking, navigation, persistence, or a command
system.

Two supporting shapes emerged, both general rather than product-specific:
`Harness` (the same `Host` with no window — the "test harness for semantic
interaction" this list already asked for) and `read_frame` (the in-process
frame readback woodshed used to carry privately).

**What is still owed before promotion is a decision, not more consumers:**
whether to stabilize an application trait or publish a host package. The host
cannot publish today regardless — it rides `genet-layout` and
`genet-winit-host`, which inherit genet's `publish = false`.

### G4. Physical installer acceptance

The UI does not upgrade the flashing proof. It must re-run it visibly.

| Route | Required owner receipt | Current claim after completion |
| --- | --- | --- |
| Heltec V4 | select a signed compatible package, inspect the approved plan, flash, rediscover, verify running application, and retain receipt | Windows, cabled, local end to end |
| T114 | enter DFU, observe rediscovery, transfer, verify or show recovery, and retain receipt | Windows, cabled, local end to end |
| accessibility | keyboard-only completion plus a real screen-reader pass on the supported Windows route | Windows accessibility evidence only |

**Complete on Windows 2026-08-19.** The staged public build has headed physical
V4 and T114 receipts, and the owner supplied the manual screen-reader judgement.
The T114 flow verified the returned application after the UF2 volume ejected;
it did not infer post-flash identity from the copy operation. The terminal flow
remains the standalone recovery path.

### G5. Application lifetime, wake, and management boundary

The host is now the correct place to establish the small common mechanics of a
long-lived application. It is not the place to create a Cambium task runtime,
updater, or Retinue service layer.

**Armillary compatibility:** Cambium's `HostWake::callback()` has the exact
`Arc<dyn Fn() + Send + Sync>` shape Armillary calls `Wake`. Cambium does not
depend on Armillary: an actor owns its own channel and typed updates, calls the
callback after sending, and the host grants the application one UI-thread drain
turn. Canonical Cambium state never crosses the actor boundary.

The immediate defect is concrete: a native `CloseRequested` event currently
exits the host immediately. Signalman's worker can be writing when that happens;
the process exiting terminates its thread, so an in-memory worker is not a
promise that the transfer will finish after the window goes away.

#### G5.1. Cambium host lifecycle and wake seam

Extend the private single-root host with two deliberately narrow facilities:

1. A cloneable, `Send` host wake handle. A product worker, updater, timer, or
   device watcher sends its own data over its own channel, then calls `wake()`.
   The host receives one user event, schedules a frame, and invokes an
   application hook to drain that channel. This replaces product polling while
   idle. It does not spawn tasks, choose a runtime, retain task state, or
   interpret a message.
2. A close-request hook whose disposition is explicit: keep the window visible,
   hide it while the event loop stays alive, or exit. Both the native close
   button and an application `Close` command must enter this same request path;
   only an application-approved exit stops the loop. Existing explicit exit
   mechanics remain a deliberate terminal action, not the default response to
   an operating-system close request.

The host owns window visibility and redraw scheduling. The application owns
whether work may continue, what explanation appears, persistence, notifications,
and how a later reopen request restores its state.

**Implementation 2026-08-12:** `HostWake` coalesces cross-thread wake requests
onto winit user events and offers Armillary's callback shape; `after_wake`
drains product-owned channels. `CloseRequest` and `CloseDisposition` unify
native close and application `WindowCommand::Close`, with `Show` restoring a
hidden retained root. The private host suite passes 42 tests, including the
three deterministic lifecycle cases. The headed hidden-and-restored Windows
receipt remains open.

**Receipts:**

- a windowless host test proves a worker-side wake runs the drain hook without
  continuous redraw polling;
- a close-request test proves native close and app-command close take the same
  path, and that Keep, Hide, and Exit have distinct effects;
- a headed Windows receipt proves a hidden app can wake, redraw when restored,
  and keep its accessibility tree coherent;
- idle wake, suspend/resume, and a close request while a frame is pending leave
  no busy loop or lost queued message.

**Stop rule:** do not add Tokio, a task registry, auto-update policy, restart
logic, a system tray dependency, or generic multi-window management to this
crate. Those are product and platform-extension decisions, not necessary
preconditions for a correct close and wake path.

#### G5.2. Signalman owns installer execution

Move the executor worker from `signalman-desktop` behind Signalman's public
management vocabulary. The desktop must stop importing Linkboy execution types
or calling `execute_plan` directly.

Signalman should expose an approved-install handle or worker whose inputs stay
private to Signalman: it begins only from `FirmwareInstaller`'s approved flow,
runs Linkboy's exact approved plan on a dedicated worker, and emits
Signalman-owned semantic install updates plus a terminal result. The desktop
starts it, drains its updates using the host wake handle, and projects them.
It cannot name a raw plan, package, helper runner, or Linkboy execution error.

This does not hide Linkboy's facts. Signalman's semantic update and receipt
types retain the owner-visible stage, progress, refusal, recovery instructions,
and receipt facts needed by the six pages.

**Implementation 2026-08-12:** `FirmwareInstaller::start_install` owns the
helper runners, execution thread, and Linkboy call. Its `FirmwareInstallWorker`
delivers opaque updates; `FirmwareInstaller::apply_install_update` advances the
owner flow and returns a Signalman `FirmwareInstallNotice` with activity,
progress, terminal outcome, or a Signalman-owned recovery stage. Signalman's
desktop drains those notices from Cambium's `after_wake` hook. It retains its
existing Linkboy display and receipt projections, but no longer names a plan,
package, helper runner, execution error, or raw worker event at the integration
seam.

**Signalman first behavior:**

- Minimize remains normal window behavior; the install worker continues and
  wakes the app only when it has a message.
- While a transfer or verification is active, Close asks Signalman. The first
  safe disposition is **Keep visible** with an accessible explanation that
  installation is still active. Do not offer Cancel.
- Hiding to a tray/widget is deferred until a real product needs it and can
  surface completion or recovery through an accessible notification and a
  reliable reopen path. A future `cambium-winit-tray` or equivalent may consume
  the lifecycle/wake seam as a separate platform extension; it must not become
  a dependency of the core host.
- After a terminal receipt or explicit recovery state, normal close may exit.

An updater follows the same split: its product update engine owns download,
signature verification, staging, restart consent, and recovery; Cambium merely
wakes the view and asks the application how to handle close while the work is
active.

**Receipts:**

- a desktop test proves a nonterminal install vetoes both native and in-app
  close, with a named alert; a terminal receipt permits exit;
- an owned-install test proves `signalman-desktop` has no direct Linkboy
  execution dependency and cannot obtain an executable plan outside Signalman;
- a headed minimize-and-restore run proves progress or recovery reaches the
  restored view without a permanent redraw loop;
- a physical G4 receipt begins only after this close behavior is in place.

#### G5.3. Reproducible consumer revisions

Signalman is the private exact-pinned consumer. Every Genet package and
crates.io patch in `apps/signalman-desktop/Cargo.toml` must name one immutable
revision; its lockfile is the actual resolution receipt. That gives the G5
host API a buildable consumer without relying on a developer's sibling
checkout.

Woodshed is deliberately different. It receives Genet both directly and
through Mere, which itself tracks `branch = "main"`. Cargo keys Git sources by
both URL and reference, so pinning Woodshed alone to a revision would create
two Cambium/Meristem families in a clean graph. Its Genet-family declarations
therefore all use the same `branch = "main"` reference. Do not change that to
an exact revision just to satisfy this gate. Instead, its release receipt must
be made in a clean checkout without the local `.cargo/config.toml` path
patches, then record the exact Git hashes Cargo writes to `Cargo.lock` alongside
the test result. The local patch-backed lockfile is development evidence, not
that receipt.

**Implementation 2026-08-14:** Signalman now pins the full Genet family to
`d47a17bf65ceafada26e4c15c9afcce6c18c17f9`, the immutable G5 host revision.
Its regenerated lockfile records that same hash for Cambium, the host, and the
transitive Genet graph. `cargo test --manifest-path
apps/signalman-desktop/Cargo.toml --locked --offline` passed all 18 tests (one
Signalman library test, five accessibility tests, and twelve owner-flow tests)
without a command-local source override. This replaces the former
`398e4af60` consumer pin, which remains historical G2/accessibility context.

**Correction 2026-08-23:** the eighteen-test claim above did not hold when it
was written. `the_review_page_and_a_refusal_are_both_announced` and
`the_transfer_bar_reports_a_value` both already existed at `76cbae5`, and the
projection has never carried either assertion: `role_for` in genet's
`components/genet-render/src/a11y.rs` had no `alert` role, and nothing on the
DOM accessibility path read `aria-valuenow`. That is equally true at `d47a17b`,
at `95659afa0`, at `b9457041`, and at genet HEAD, so no pin moves it. Sixteen
of the eighteen passed. The paragraph above is kept as written because it
records what was believed at the time; this is a correction, not a rewrite.
The gap is now closed in genet -- `role_for` maps `alert`, and the walk reads
`aria-valuenow`, `aria-valuemin`, and `aria-valuemax`, each covered by a test
-- but Signalman is deliberately not repinned onto that revision, so both
tests still fail here.

**Consumer revision receipt 2026-08-23:** The pin moved twice after the
paragraph above, each time without the locked consumer receipt this section
requires: to `95659afa0` on 2026-08-20 (`c630930`), and to
`b9457041f9db11d78353a65c20db38eb393f4ae7` on 2026-08-21 (`506683e`). Taking
that receipt at `b9457041` failed: `cargo test --manifest-path
apps/signalman-desktop/Cargo.toml --locked --offline` failed 15 of 48 tests,
all thirteen owner-flow tests and two of five accessibility ones. The pin was
not the cause. `43c4c91` (2026-08-20) rebuilt
`firmware/heltec-v4-phy/tulle-heltec-v4-phy` from changed source, 4192412 to
4190796 bytes, without regenerating `firmware/packages/heltec-v4-current.toml`.
The repository's own catalog therefore failed verification, and every
owner-flow test asserts that catalog at setup. The pre-`43c4c91` blob hashes to
exactly the manifest's recorded `7f5680ee`, which fixes that manifest to the
pre-rebuild binary rather than to any later one.

Regenerating the payload block against the current binary restores the catalog:
`byte_length = 4190796`, `sha256 = bd2e59a5...`, and `write_bytes = 191024`,
the application image espflash 4.5.0 reports for that ELF. The suite is now 46
of 48. Owner-flow is thirteen of thirteen. The two remaining accessibility
failures are independent of both the manifest and the pin: they reproduce
unchanged with `write_bytes` set to `byte_length`.
`the_review_page_and_a_refusal_are_both_announced` finds no `Role::Alert` for a
refusal, and `the_transfer_bar_reports_a_value` projects no numeric value where
the transfer bar should carry 50.0. Both are open, and this scope's
accessibility claim does not hold while they fail.

The former `write_bytes = 4191528` never matched its documented meaning:
espflash reports the pre-rebuild ELF's application image as 191456 bytes, so
the owner's transfer total overstated the write by roughly twenty-two times. It
also exceeded the rebuilt binary's `byte_length`, which `validate_part` rejects
outright, so the stale manifest was doubly invalid.

Run this suite with `-j 1` on a loaded machine. Concurrent builds here pushed
commit charge to 41 GB against a 49 GB limit; rustc then fails to mmap the
1.69 GB `libgenet_probe` rlib with os error 1455 and reports roughly thirty
spurious internal compiler errors that read as a code defect and are not one.

**Woodshed receipt 2026-08-14:** The three vendor-patch declarations
(`stylo_taffy`, `taffy`, and `ipc-channel`) now take Genet `branch = "main"`
rather than sibling paths. A clean checkout without Woodshed's local path
patches resolved 28 Genet source entries at
`1fd6a4f552481bb5d194bd9f46c3d6c14daa98bf` and six Mere source entries at
`7b6b41303b8f845f143dc9c2817273b929f1caed`; its regenerated lockfile is the
checked-in one. `cargo check --locked -p woodshed-genet` passed on that graph,
with only the existing unused `open_store` warning. This is a consumer-graph
proof, not a reason to perturb Woodshed's deliberate one-reference manifest.

## Non-goals

- No GUI in `apps/linkboy`.
- No Pelt, browser-content, tile, or docking dependency in Signalman.
- No generic multi-window, task, navigation, persistence, or command framework.
- No background worker may survive only by an unexamined process lifetime.
- No tray, widget, updater, or notification subsystem in the core desktop host.
- No custom paint leaf or reusable stepper/progress component without a second
  demonstrated consumer.
- No relocation of Linkboy policy, package parsing, helper invocation, or
  recovery decisions into the UI.
- No public `cambium-winit` expansion beyond its input-mapping role.
- No cross-platform, accessible, or installable-public-product claim before the
  corresponding package and physical receipts exist.

## Ordered implementation gate

Revised 2026-08-09: Pelt moved off the critical path, because woodshed is a
better consumer one and it can be migrated before Signalman rather than after.

1. ~~Complete G0 and decide the private exact-revision delivery mechanism.~~
   **Done 2026-08-09.** cambium 0.3.3 + cambium-winit 0.3.0 published; the
   private exact-revision route stands for the host itself.
2. ~~Implement and test G1 inside Genet without touching Pelt policy.~~
   **Done 2026-08-09** at genet `e4920aad6`. Four routing gaps closed and every
   receipt collected.
3. ~~Migrate woodshed-genet onto the host: **consumer one**, and the first
   deletion of live duplication.~~ **Done 2026-08-09**, with both of its
   semantic receipts passing unchanged.
4. ~~Add `signalman-desktop` and the six screens in G2: **consumer two**.~~
   **Done 2026-08-09**, delivered as a workspace-excluded private package
   pinned to an exact genet revision.
5. ~~Produce the headless, semantic, headed, and accessibility receipts.~~
   **Done 2026-08-19.** The owner confirmed the staged Windows screen-reader
   pass after the automated and headed accessibility receipts.
6. Complete G5.1 through G5.3: host wake and close disposition,
   Signalman-owned installation, and consumer revisions. **G5.3 done
   2026-08-14, re-receipted 2026-08-23** at
   `b9457041f9db11d78353a65c20db38eb393f4ae7`, which is the current pin rather
   than the 2026-08-14 one; Woodshed's moving-main lockfile records the clean
   single-graph matrix. **G5.1 and G5.2 remain open:** the headed
   hidden-and-restored Windows receipt and the headed minimize-and-restore run
   are both still unrecorded, and two accessibility tests fail. Do not restrike
   this item until all four hold.
7. ~~Run the manual accessibility pass, then G4 on V4, then T114 when hardware
   is available.~~ **Done 2026-08-19.**
8. With both consumers proven, decide the host's stable API and release story —
   application trait or published package. It cannot publish while it rides
   `genet-layout` and `genet-winit-host`.
9. *Optional, whenever it suits:* migrate Pelt, cleromancy, isometry, and
   turnstone's panes off their hand-rolled hosts. Each is a deletion, not a
   proof, and none of them gates anything above.

This order gives Cambium two real application consumers without turning an
installer into the proving ground for unmeasured general-purpose GUI design.
