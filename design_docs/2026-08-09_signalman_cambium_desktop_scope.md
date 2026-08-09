# Cambium adoption and upgrade scope for Signalman

**Date:** 2026-08-09  
**Status:** G0, G1, G2, and G3 complete; G4 and the manual accessibility pass
open. Consumer order corrected — woodshed first, Signalman second, Pelt
optional.

## Decision

Use Cambium for Signalman's native desktop face. Do not put a GUI in Linkboy
and do not make Retinue invent a second component toolkit.

This is a Cambium adoption and upgrade plan. Cambium is the GUI toolkit;
Signalman is the first concrete application integration used to expose and
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

- `cambium 0.3.2` owns application view composition and the retained
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
by branch like the a11y crate. Compiles clean with a headed smoke example
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
**`e4920aad6`** — the revision consumers pin, not `246f0f1e7`.

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
exists, pinned to genet `e4920aad6`. Full write-up in
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

**Still owed:** somebody listening to a screen reader read this flow and
judging whether it makes sense. That remains a G4 prerequisite.

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
panes riding the same runner. All the external ones already consume the
cambium family from genet.git by branch, so they can adopt a private
`publish = false` host package with no new delivery mechanism. Candidate
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

Retain the terminal flow until both graphical routes have physical receipts.
Do not infer post-flash application identity from a helper's successful exit.

## Non-goals

- No GUI in `apps/linkboy`.
- No Pelt, browser-content, tile, or docking dependency in Signalman.
- No generic multi-window, task, navigation, persistence, or command framework.
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
   **Done**, except the manual screen-reader and keyboard pass, which needs a
   person and is a G4 prerequisite.
6. Run the manual accessibility pass, then G4 on V4, then T114 when hardware
   is available.
7. With both consumers proven, decide the host's stable API and release story —
   application trait or published package. It cannot publish while it rides
   `genet-layout` and `genet-winit-host`.
8. *Optional, whenever it suits:* migrate Pelt, cleromancy, isometry, and
   turnstone's panes off their hand-rolled hosts. Each is a deletion, not a
   proof, and none of them gates anything above.

This order gives Cambium two real application consumers without turning an
installer into the proving ground for unmeasured general-purpose GUI design.
