# radio-mirror: radio-face in the browser

**Status, 2026-10-01:** M1 landed on the lane branch, not on `main`. Every
done-condition below has a receipt. Forks F1-F8 are open for Mark; the code
carries a provisional, reversible choice for each.

**Authority:** phase S8 of mer3ly's
`docs/2026-09-30_graphshell_site_canvas_plan.md`, which carries the rulings.
This plan cites them and does not restate them:
- Ruling 1 (the site consumes or creates stack capabilities, no exceptions);
- Ruling 6 (a browser realization of `radio-face` replaces `radio-simulator.js`
  and the lab's mock screen);
- Ruling 8 (real firmware views; no firmware UI change);
- Ruling 16 (a new retinue crate; the framebuffer layer kept separable);
- Ruling 23 (the name `radio-mirror`, unpublished).

radio-face's intent is in the [on-device UI plan](2026-07-28_on_device_ui_implementation_plan.md).
The trace-to-status mapping is S7's (`retinue-sim`), not this plan's.

## Phase M1: the crate

`crates/radio-mirror`, a workspace member and default member, `publish = false`.

| Module | Holds | Depends on |
| --- | --- | --- |
| `framebuffer` | `RgbaFramebuffer<C>`: a `DrawTarget` for any `C: PixelColor + Into<Rgb888>`, storing opaque RGBA8; optional PNG writer | `embedded-graphics` only (+ `png` behind a feature) |
| `mirror` | `Mirror`: the firmware's `Controller`, `PressClassifier` and `render` over the framebuffer; stateless `render_rgba` and `render_png`; palettes | radio-face |
| `names` | kebab-case names for surfaces, screens, events and actions | radio-face |
| `input` (feature `json`, default) | JSON documents for `LocalStatus` and `HostSnapshot` | serde, serde_json |
| `web` (wasm32 only) | `wasm-bindgen` exports: `RadioMirror` and `render_screen` | wasm-bindgen `=0.2.127` |

Done-conditions, each met (receipts in Findings):
1. A wasm32 release build, with `wasm-bindgen --target web` producing a module.
2. Pixels identical to `render_receipts` for all 12 of its screens on both surfaces.
3. All seven simulator pages render through `Controller` navigation, and so do
   the TRAFFIC page and its ticker events.
4. Press sequences through the mirror match `Controller` driven directly.
5. A native PNG function for build-time, no-script images.
6. radio-face, both firmwares and radio-hand unaffected; clippy clean.

## Findings (2026-10-01, retinue `66bc578`)

**The golden is the example's own output.** `tests/golden/` holds the 24 PNGs
that `cargo run -p radio-face --example render_receipts` writes, unedited
(about 105 KB). `tests/golden.rs` checks two things against them: the
mirror's pixels, and the PNG bytes that `render_png` writes. Both match.
A control test shows the comparison catches a palette swap and a one-frame
`tx_frames` change. The JSON fixtures restate `render_receipts::fixture()`.

**Both boards render monochrome.** V4 `ui.rs:336` and T114 `ui.rs:358` pass a
`BinaryColor` theme, and the T114 flush maps On to `0xFFFF`. The amber TFT palette
exists only in `render_receipts`. The mirror defaults to mono (`mono_theme`)
and offers the receipt palette (`receipt_theme`) explicitly. See F5.

**Both fitted boards use the one-button profile** (`ui.rs:216`, `:247`). The
simulator's two-button face has no current board behind it.

**Board glue is not in radio-face.** Each firmware's `ui.rs` loop does four things itself:
- sets `local.display_on` from the controller;
- sets `last_wake = Button` on a press;
- drops an expired host snapshot (`fresh_host`);
- shows `Screen::DisplayOff` before the panel goes dark.

The mirror repeats the first three (`Mirror::press`, `age_host`), about ten
lines, and leaves panel power to the consumer (`panel_lit`). That glue is
visible: after a press, POWER's ticker reads WAKE BUTTON, as a board's does.
See F6.

**Host documents go through the radio's own wire.** `input::host_from_json`
round-trips the snapshot through `encode_snapshot`/`decode_snapshot`. A
document a real host could not send fails with the wire's error, for example
`PrivacyViolation` for a named node under `minimal`. A snapshot shown in the
browser is therefore one the radio would have accepted. `set_host_wire` takes
the raw bytes directly.

**Remote derives pin the JSON to radio-face's types.** The `input` definitions
are serde `remote` mirrors. A field added to radio-face fails this crate's
build instead of drifting silently. Unknown JSON fields, text that is too long,
and non-ASCII text are all refused.

**The simulator's content is invented, beyond what S8's inventory recorded.**
Its menu is BACK, VERIFY, DISPLAY OFF. The firmware's menu is Brightness,
Detail, Verify (with a named host), Display off, Reboot, Back. Its TRAFFIC
page shows RSSI and SNR rows; the firmware shows TX/RX, HOST QUEUE, LAST RX
and LAST TX. The mirror shows the firmware's.

**Payload** (`opt-level = "s"`, fat LTO and `codegen-units = 1`, all
inherited from the workspace release profile, which cannot be overridden per
package for LTO):

| File | raw | gzip -9 |
| --- | --- | --- |
| `radio_mirror_bg.wasm` (default features) | 340,881 | 91,593 |
| `radio_mirror_bg.wasm` (`--no-default-features`, no JSON) | 129,149 | 41,332 |
| `radio_mirror.js` | 17,546 | 3,896 |

JSON accounts for 62% of the raw wasm and 55% of the gzipped wasm. See F1.

**Firmware links fail inside `.claude/worktrees/`, at HEAD too.** Cargo's
config discovery walks up from the nested worktree and also finds the main
checkout's `.cargo/config.toml`. It merges both `rustflags` arrays, so the
link sees `-Tlink.x` (T114) or `-Tlinkall.x` (V4) twice and fails. With
`RUSTFLAGS` set to a single copy, both link. This is an environment artifact
that affects any lane building firmware from a nested worktree, not a code
fault.

## Forks (open; the provisional choice is marked)

- **F1, the status input interface.** (a) *Provisional:* JSON documents, as
  above, plus raw wire bytes for the host. Costs 211,732 B raw / 50,261 B gzip.
  (b) Typed `wasm-bindgen` setters, about 40, with no serde. Smaller, but
  chattier, and gives no file format for build-time fixtures. (c) Wire bytes
  for the host only; `LocalStatus` has no codec, and adding one would change
  radio-face. S7's trace format may decide this. A schema tag in the documents
  (`radio-mirror.local/v1`) is not added yet.
- **F2, the canvas API.** *Provisional:* `rgba()` returns a `Uint8ClampedArray`
  for `new ImageData(…)`, with no `web-sys`. The alternative draws to a passed
  `CanvasRenderingContext2d`, which needs `web-sys` features and saves one JS
  copy (32 KB OLED, 130 KB TFT per frame).
- **F3, the wasm-bindgen version.** `=0.2.127`, matching the installed CLI and
  Cambium's web host. mer3ly's repo-graph pins `=0.2.126`. Separate modules on
  one page do not conflict, so I found no reason to differ.
- **F4, the framebuffer split.** *Provisional:* a module now. It depends only
  on `embedded-graphics`, so splitting it out is a file move. The alternative
  is a crate now, which needs a name from the naming ledger.
- **F5, the default palette.** *Provisional:* mono, which is what the boards
  show. The receipt palette is opt-in (`set_palette([])`).
- **F6, the board glue.** *Provisional:* the mirror repeats it. The
  alternative lifts it into radio-face, where both firmwares and the mirror
  would share one copy. That is a radio-face and firmware change, outside S8.
- **F7, accessibility.** A canvas is opaque, and radio-face draws text as
  pixels, so the mirror cannot say what the screen reads. The options are a
  radio-face text projection (a radio-face change) or alt text the consumer
  authors. The JS simulator builds its `aria-label` from its own rows today.
- **F8, CI.** radio-mirror is a default member, so CI's native build, test,
  clippy and doc steps cover it. Nothing in CI builds wasm32. A
  `cargo build -p radio-mirror --target wasm32-unknown-unknown` step would
  guard it.

## Progress

- 2026-10-01: M1 built on the lane branch. 17 tests pass with all features.
  The wasm module loads in Node 24 (V8) through the generated JS, and 18
  screens there match the goldens pixel for pixel. Not yet run in a browser
  page. Forks F1-F8 returned to Mark.
