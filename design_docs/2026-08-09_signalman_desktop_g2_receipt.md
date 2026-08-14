# signalman-desktop — G2 receipt

**Date:** 2026-08-09
**Package:** `apps/signalman-desktop` (`publish = false`, excluded from the
retinue workspace)
**Plan:** `2026-08-09_signalman_cambium_desktop_scope.md`, lane G2

## Delivery

The package is **excluded** from the retinue workspace and roots its own. The
scope asks for "the desktop package without changing default members until its
Genet delivery dependency is reproducible"; membership-but-not-default would
still have forced this repository's root manifest to carry genet's `[patch]`
table — the vendored `taffy`, `stylo_taffy`, and `ipc-channel` forks, because a
patch does not inherit through a git dep — and that would drag the whole engine
graph into every ordinary retinue build, including the MSRV job that exists to
hold a promise about six published crates. Excluding it satisfies the intent
strictly: the default build, the lockfile, and CI are untouched.

```bash
cargo test --manifest-path apps/signalman-desktop/Cargo.toml
```

Genet comes in **at an exact revision, not a branch**: `398e4af60`. The
extraction commit `246f0f1e7` predates the four routing fixes; `e4920aad6`
adds the ARIA roles this application's accessibility receipt turned up; and
`398e4af60` also carries the accessible-label repair this face needs. Bump the
pin deliberately, with a receipt.

`signalman` and `linkboy` come in by relative path. That is not a cross-checkout
dependency — they are in this repository.

**No crates.io installation path is claimed.** The host rides genet packages
that inherit genet's `publish = false`; no version number changes that.

## Layer boundaries, enforced rather than asserted

- The application depends on `signalman` for vocabulary. Linkboy's types appear
  (they are what Signalman's projections carry) but no policy, planning, or
  execution function is called from the face.
- It **cannot** construct, alter, or execute a `FlashPlan`. `FlashPlan`'s
  fields are private, its only constructor is Linkboy's planner, and the only
  way to obtain one here is `FirmwareInstaller::begin_install` — the flow's own
  gate, which refuses before approval. A test asserts that refusal.
- Views are pure functions of state. Anything that opens a serial port or
  starts a thread is a `Request` a handler records and the application loop
  fulfils. That is also what lets the whole six-page flow run headless.

### Seams added to Signalman

Four, each opened by a real view gap rather than speculatively:

- `survey_devices()` / `DeviceCandidate` — a chooser needs a device list, and
  the face must not reach past Signalman into Linkboy's port survey for it.
- `observe_device()` — the same observation construction `linkboy plan` and
  `linkboy flash` perform, including the ESP ROM discovery pass without which a
  V4 plan is refused for missing processor and flash facts.
- `refusal_lines()` / `describe_event()` / `event_progress()` — a refusal
  rendered as separate visible lines, and events as owner-readable text.
- `FirmwareInstaller::{receipt, plan, chosen_package}` — `FirmwareView` carries
  the receipt's *result*, which decides a page but cannot show what was written
  to which board.

## The six pages

1. **Choose device** — every port and what answered on it, a board-revision
   field, Rescan, Use this device.
2. **Choose firmware** — catalogued packages with their verification state; a
   catalog that will not verify is a visible state, not a panic.
3. **Review changes** — every `FirmwareReview` field: package identity and
   version, publisher, payload SHA-256, license, source, origin, board and
   revision, route, helper with its version/license/source, write and preserved
   ranges as explicit hex, state impact spelled out, and both recovery texts.
4. **Prepare device** — the package's own before-write instructions, repeated
   where they still matter.
5. **Install** — a progress bar with a real value, and an event log.
6. **Verify or recover** — the receipt (what is running, which package, which
   hash, which board) or the recovery context (which stage, whether writing had
   started, last known port, and the package's after-failure instructions).

Refusals are visible text throughout. There is no disabled control with a
missing explanation anywhere in the flow.

No Sprigging leaf, no workflow-stepper component, and no new general-purpose
progress primitive: ordinary Cambium controls and CSS, per the scope.

## The worker bridge

The blocking Linkboy executor runs on a dedicated thread and sends structured
`FlashEvent`s home over an `mpsc` channel. The `GenetAppRunner`, the DOM, and
the view callbacks never leave the UI thread; the channel carries owned copies
of the approved plan and package, so what executes is what was reviewed. A
closed channel does not abandon a transfer mid-write. The host knows nothing
about any of this — it is a `frame` hook that drains a channel.

## Receipts

### Headless page-state tests — `tests/owner_flow.rs`, 11 cases, passing

Driven through `cambium_genet_winit_host::Harness`: the same `Host` the binary
uses, constructed without a window, so a click in a test goes through the same
hit test, dispatch, and focus rules a click in the app does.

Covered: the opening page lists what answered; a missing board revision refuses
in words *and says why*; the review page shows every plan fact (each asserted
against the flow's own projection, so a dropped field fails); approval reaches
Prepare with the before-write instructions; events progress the install page,
with the write line replacing its predecessor rather than stacking one entry
per chunk; a recovery event ends on the recovery page with stage, instructions,
and last known port; the active-install close policy refuses both native and
app-command close with a visible alert; and the face cannot execute a plan the
flow did not give it.

### Semantic keyboard tests — same file

Controls activate by role and label through the `genet-probe` resolver, never
by coordinate. Tab reaches the device row, the revision field, then both
actions in page order; Shift+Tab walks back, wrapping; Enter activates the
focused control with no pointer involved. Typing reaches the revision field
through the host's `focused_text` seam.

### Accessibility — `tests/accessibility.rs`, 4 cases, passing

Asserted against the projection the AccessKit adapter is handed: every control
has a name (and none is announced nameless), the revision field projects as a
text input, focus is reported so a reader's virtual cursor follows the
keyboard, the refusal projects as an **alert**, the review page's facts are all
reachable as text, and the transfer bar is a progress indicator carrying a
value.

Two of those failed on first run and turned up a real engine gap: genet's ARIA
role map covered controls and tabs only, so `role="alert"` projected as a
generic container and `role="progressbar"` announced with no value at all — the
fake-spinner failure in reverse. Fixed upstream in genet `e4920aad6`
(announcement and value-bearing roles, `aria-valuenow/min/max`, and
`aria-pressed` → toggled), with its own regression test.

### Headed run, 2026-08-09, Windows

The binary opens a real window, discovers real hardware, and installs the
accessibility adapter:

```text
[cambium-winit] accessibility Installed, 28 nodes projected
```

The device page listed this machine's actual ports — two Heltec V4 boards
(COM6, COM7, both region US915, channel rnode) and one silent port (COM10).

**Windows' own UI Automation** reads the tree as:

```text
Button  Use this device
Button  Rescan
Edit    (focused, value empty)
Button  COM10 — silent (not running, or in use)      toggle_state off
Button  COM7 — HeltecV4, region US915, channel rnode toggle_state off
Button  COM6 — HeltecV4, region US915, channel rnode toggle_state off
```

That is the OS reading the tree, not the in-process projection: every control
named, the `aria-pressed` selection state carried through, and focus reported.

Live keyboard traversal works — three Tabs moved the visible focus ring down
the device rows and a fourth reached the revision field, with UIA's
`has_focused` following.

Two visual defects were found and fixed by this run: the revision field did not
render (the sheet styled a class that `text_field`'s bare `<input>` never
carries), and the trail double-numbered ("1. 1. Choose device") because the
`<ol>` already supplies the marker.

## Open, and not claimed

**Live character typing was not confirmed.** Synthetic OS keystrokes reached
the app for named keys (Tab traversed) but no character landed in the focused
field, and the automation tool's own click coordinates then dropped focus. In-
process, `key_char` through the real host path inserts correctly, so this is
either winit's mapping of *synthesized* keystrokes or a real defect, and the
available tooling cannot tell them apart — which is the exact unreliability
that made woodshed abandon SendKeys for a self-drive lane in the first place.

**Resolved the same day, and it was a real defect.** Tracing the host rather
than guessing named it: Windows delivers injected text as `VK_PACKET`, winit
surfaces that as `Key::Unidentified`, and the host dropped it. Not merely a
test artifact — on-screen keyboards, keyboard remappers, and assistive input
tools all type that way. Fixed in genet; "4.2" now types into the revision
field by keyboard alone.

## The accessibility pass, automated

The claim that a screen-reader pass "needs a person" was too broad. Three
things get lumped together under that name, and only the third needs one:

1. **What the OS exposes** — roles, names, values, focus, bounds in the live UI
   Automation tree. Fully automatable.
2. **What a screen reader announces** — NVDA composes speech from that tree
   with its own heuristics. Automatable too, with NVDA's testing driver; not
   built here.
3. **Whether the announcements are any good** — wording, verbosity, whether the
   flow makes sense to someone who cannot see it. Design judgment. This is the
   part that needs a person.

Tier 1 is now `testing/a11y-audit.ps1`: launch the app, walk the live UIA tree,
assert the contract every assistive technology reads. It checks the tree
published at all, that every control has a name, that no name is a glyph a
reader cannot say, that every control has a box a virtual cursor can land on,
that focus is published and Tab moves it, that every focus stop announces as
something nameable, and that the app kept the foreground for the sample (or the
whole reading is fiction).

It found four real defects on first run, none of which the in-process tests
could see:

- signalman's revision field was nameless — `text_field` emits a bare `<input>`
  with no id, so `<label for>` pointed at nothing and a reader said "edit,
  blank". Fixed by wrapping the field in its `<label>`, and by teaching genet
  to take a control's name from the label wrapping it (genet `398e4af60`).
- woodshed's chrome buttons announced as "dash", "white square", and
  "multiplication sign".
- woodshed's window drag surface was a focus stop announcing as "group" with
  nothing to do — genet now honours `aria-hidden` (`4d2ca0211`).
- an inline `<label>` put its `<input>` back in a shared line fragment, so the
  field lost its box again. The same inline-box gap noted below.

Both applications now audit **RESULT ok**: signalman 9 controls, focus order
COM6 → COM7 → COM10 → Board revision → Rescan → Use this device; woodshed 72
controls, focus order Woodshed → Minimize → Maximize → Close → Stage →
Rehearsal.

**Still owed: tier 3.** Nobody has listened to a screen reader read this flow
and judged whether it makes sense. That is a G4 prerequisite and a person's
job. Tier 2 (capturing NVDA's actual speech) is buildable if it turns out to be
worth the dependency.

## Hardware acceptance (G4) not started

No board was flashed and no ROM discovery was run against the user's radios.
G4 is its own lane and needs owner receipts per route.
