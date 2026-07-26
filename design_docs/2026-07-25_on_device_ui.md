# On-device UI: the PANEL×LEDGER face

**Status:** accepted (design pass, 2026-07-25)
**Prototype:** interactive simulator + mockups in the "UI design for retinue radios"
design project (Radio Simulator / Firmware UI Directions).

## What the on-device UI is

A glanceable status surface, not an app. Every glyph on the panel is a value
the firmware actually owns — link state, last announce heard and from whom,
peer count, queue depth, battery, channel/preset. No placebo: if a value
doesn't exist yet (pre-provisioning, no host), the gauge renders an em-dash.
No text entry on the device, ever; input lives on the connected host (phone,
laptop). The same state machine drives screen, LED, or both, so screenless
radios (Qi-back puck) speak the LED dialect alone.

## Visual system

- **Layout:** persistent header strip + body + optional event ticker.
  - Strip: pixel icon + screen title (left) · link chevron + battery glyph (right).
    No page counter — the icon is the "where am I".
  - Body: 2×2 label-over-value gauges for numeric subjects; ledger rows for
    list subjects (peers). A screen is gauges unless its subject is a list.
  - Ticker (bottom, ruled off): one live event line with timestamp
    ("RX 243B FROM ESQUIRE 12:41"). Earns its line by being live.
- **Five-line rule:** max 5 lines on 128×64 including strip and ticker.
  Lists get at most 3 rows; overflow renders "+N MORE" in the ticker, never a
  6th row.
- **Type roles:** blocky pixel face for names/labels (Silkscreen-class),
  condensed tall face for values (VT323-class). Two bitmap fonts total.
- **Emphasis:** inverse video only — menu selection, fault banner. No other
  emphasis mechanism exists.
- **Color:** monochrome design; on color panels (T114) the accent tints
  chrome per personality, layout unchanged.

## Faces

Short A/B cycles: IDENTITY → POWER → RADIO → TRAFFIC → PEERS.

1. **IDENTITY** — name, addr tail, role, uptime.
2. **POWER** — batt %, volt (label flips to USB PWR at 5.00), naps
   (light-sleep count), held-awake count.
3. **RADIO** — freq, SF·BW, TX pwr, preset name (selvage profile).
4. **TRAFFIC** — TX/RX counters, queue depth ("3 HELD"), airtime %/h,
   last-RX RSSI·SNR. Ticker: last frame event.
5. **PEERS** — up to 3 rows: NAME · ^DIRECT|VIA hop · age. Ticker:
   "+N MORE · HEARD name HH:MM".

Modal faces (not in the cycle):

- **BOOT** — wordmark + board/firmware line + init checks in real order
  ("RADIO OK · KEYS OK · HOST —"). No strip: identity isn't loaded yet, so
  nothing pretends.
- **PROVISION** — strip reads "RET·NEW · PROVISION VIA HOST"; gauges show
  em-dashes for name/addr, KEY NONE, HOST state. Keys are minted from host.
- **VERIFY** (hold A) — identicon + full 16-byte fingerprint in two groups
  of 4×4 hex, "COMPARE IN PERSON". Any key exits. Identicon and hash both
  derive from the key.
- **MENU** (hold A+B ≈900ms) — PRESET / BRIGHTNESS / PAIR HOST / REBOOT.
  A moves down, B selects, hold B backs out. Bounded choices only.
- **PRESET** — selvage profiles by name with SF/dBm; current marked "·IN
  USE"; footer states the regional cap ("REGION CAPS TX + AIRTIME").
- **PAIR** — BLE numeric comparison: 6-digit code, "SAME CODE ON YOUR
  PHONE?", A YES · B NO. The one time buttons answer a question.
- **OTA** — "PHY V10 > V11", segmented progress bar, "58% · 121/208 KB ·
  KEEP POWER". Verify-then-reboot; on failure boot the old image and say so.
- **FAULT** — strip persists (the panel never lies about who it is), inverse
  banner blinks the firmware's own error string ("FAULT · SX1262 INIT
  FAILED"), retry countdown, "SEE HOST LOG". Preempts the cycle until
  cleared.
- **SLEEP** — face shows ~2s ("KEYS OR RADIO WAKE · DISPLAY OFF IN 2S"),
  then the panel is truly off. A lit screen while "sleeping" is a placebo.
  Wake replays the face you left.

## Button grammar

Two buttons (A right, B left). One-button radios keep A; B's verbs move into
the menu.

- A short: next screen · B short: previous screen
- A hold (≥650ms): identity fingerprint
- B hold: sleep now / wake
- A+B hold (≈900ms): menu
- Any press wakes the display; the wake press is consumed (doesn't page).
- In MENU: A down, B select, hold B back. In PAIR: A yes, B no.

## LED dialect (single LED; whole UI for screenless boards)

- slow breathe — asleep, healthy
- two quick blinks — frame received
- solid — link up / host attached
- three-pulse pattern — fault, needs host

## Per-personality adaptation

One strip slot carries the personality's vital sign; one gauge may swap per
face. Everything else is identical.

- **RET (native node):** link state · peers/queue as designed.
- **RND (RNode modem):** vital sign = HOST OK + fw version; TRAFFIC leads
  with host throughput; PEERS face hidden if the host owns the peer table.
- **MCR (MeshCore relay):** vital sign = repeat count + zone; TRAFFIC leads
  with repeats; queue = relay backlog.
- **SNT (sennet):** vital sign = channel util % + node count.

Each personality fills a shared `Status` struct from what it actually knows;
missing fields render em-dashes.

## Implementation sketch (retinue-face crate)

- `embedded-graphics` over trait draw targets: ssd1306/sh1106 (128×64 mono,
  1KB framebuffer) and mipidsi (T114 135×240 color). Same drawing code.
- Two `MonoFont` bitmap fonts (~2–4KB flash); icons are tiny sprites.
- `enum Face { Boot, Cycle(Page), Verify, Menu, Preset, Pair, Ota, Fault,
  Sleep }` — the simulator's logic class is near-pseudocode for it.
- Redraw on event or 1Hz tick, not a render loop. Sleep = display-off
  command + LED PWM. Brightness maps to panel contrast.
- Input: two GPIOs, debounce + press-length classification (short <650ms /
  long / both-held) as one embassy task.
- Est. 1–2K lines total, shared across boards.

## Open questions

- Whether TRAFFIC/PEERS tickers persist last event across sleep (currently:
  yes, redrawn from Status).
- Peer-name truncation width on 128×64 (currently 8 chars before the ledger
  columns collide).
- Whether OTA face needs a distinct LED pattern (currently reuses solid).
