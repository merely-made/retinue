# On-device UI: the PANEL×LEDGER face

**Status:** accepted direction (2026-07-25), corrected for implementation
(2026-07-28)
**Prototype:** interactive simulator and mockups in the "UI design for retinue
radios" design project (Radio Simulator / Firmware UI Directions).
**Execution plan:** `design_docs/2026-07-28_on_device_ui_implementation_plan.md`.

The prototype remains the visual and interaction reference. Its always-present
node identity/peer pages, editable preset, pairing, OTA, breathing sleep LED,
and bench-style power counters predate the corrections below and are not
implementation authority.

## What the on-device UI is

A glanceable status surface, not an app. PANEL×LEDGER is the shared visual
language for the V4 OLED, the optional T114 TFT, and screenless radios using
the LED dialect.

Every value names its authority:

- **LOCAL** values come from the board firmware: board and firmware identity,
  radio initialization, the applied PHY profile, raw frame counts, last
  RSSI/SNR, local faults, display state, and locally available power facts.
- **HOST** values come from an attached Retinue, Sennet, or MeshCore process:
  node identity, links, peers, routes, protocol queues, IFAC state, and
  delivery or propagation events.
- Unavailable values render as an em-dash. A page whose subject is entirely
  unavailable is left out of the page cycle.

The current direct-PHY images are modems. They do not infer Retinue identity,
peers, routes, or links from opaque radio frames. A future embedded Retinue
node can fill those fields locally without changing the renderer.

Text entry and unbounded configuration live on the connected host.

## Visual system

- **Layout:** persistent header strip, body, and optional event ticker.
  - Strip: pixel icon and screen title at left, local radio/host health and
    battery or power glyph at right.
  - Body: 2×2 label-over-value gauges for numeric subjects, ledger rows for
    lists.
  - Ticker: one bounded event line, such as `RX 243B · -97 · 6DB` or
    `DIRECT DELIVERED 12:41`. Event text states whether it came from LOCAL or
    HOST when the source would otherwise be ambiguous.
- **Five-line rule:** at most five lines on 128×64, including strip and
  ticker. Lists get at most three rows; overflow renders `+N MORE`.
- **Type roles:** one blocky bitmap face for names and labels, one condensed
  bitmap face for values. The implementation chooses fonts by measured glyph
  bounds at 128×64 and 240×135. The web fonts are mood references, not build
  dependencies.
- **Emphasis:** inverse video for selection and a steady fault banner. Urgent
  state may blink the LED; the whole display does not blink.
- **Color:** monochrome first. The T114 TFT may tint chrome by personality,
  while preserving contrast and layout.

## Page registry

The page cycle is capability-driven rather than fixed.

### Available from direct-PHY firmware

1. **STATUS**: board, firmware, modem personality, uptime, radio state, and
   host attachment. This replaces the fictional node identity shown by the
   current simulator when no node snapshot exists.
2. **POWER**: power source, battery/voltage if measured, display state, and
   sleep policy. The UART low-power build may also show the last wake source
   and current blocker. Raw `NAPS` and `HELD AWAKE` counters remain host
   diagnostics, not primary user values.
3. **RADIO**: applied frequency, SF/BW/CR, TX power, sync word, and profile
   name when the host supplied one.
4. **TRAFFIC**: raw TX/RX frame counts, last RX RSSI/SNR, last transmit result,
   and a bounded local event ticker.

### Added only when a Retinue host supplies node truth

5. **IDENTITY**: node name, address tail, role, and node uptime.
6. **LINKS**: admitted-link count and state. Link state means a Retinue link,
   not merely an attached USB cable or a received LoRa frame.
7. **PEERS**: up to three names, direct/via state, and age.

IFAC state is a host-supplied interface fact. The panel may show `IFAC ON` or
an interface label, but never key material. Direct delivery and propagation
appear as bounded HOST events; message content stays on the host.

## Modal faces

- **BOOT**: wordmark, board/firmware line, and checks in actual initialization
  order. The display must initialize early enough to report a later radio
  failure.
- **VERIFY**: identicon and full fingerprint, only when a trusted node
  snapshot supplied it. Any key exits.
- **MENU**: brightness, display off, status detail, and reboot. Entries appear
  only when implemented.
- **PROFILE**: read-only applied PHY settings in the first release. The host
  currently configures the radio at attachment, so the device must not also
  claim preset authority. A later request/accept handshake can enable editing.
- **FAULT**: persistent identity chrome and a bounded firmware-owned error,
  such as `FAULT · SX1262 INIT`. It preempts normal pages until recovery.
- **DISPLAY OFF**: a brief `DISPLAY OFF · KEY TO WAKE` face, then the panel
  turns off. This is distinct from CPU sleep. USB builds cannot preserve
  their host link through ESP32-S3 Light-sleep; the UART personality sleeps
  automatically when its gate permits.
- **PAIR** and **OTA**: reserved until BLE pairing and verified update/rollback
  contracts exist. They are not placeholder menu entries.

## Button grammar

Press classification is shared code with fixed thresholds:

- long press: at least 650 ms
- two-button chord: at least 900 ms
- any press wakes an off display and is consumed

Two-button boards:

- A short: next page
- B short: previous page
- A long: verify, when available
- B long: display off
- A+B chord: menu
- In a menu, A moves, B selects, and B long goes back

One-button boards:

- A short: next page
- A long: open the menu
- In a menu, A short moves and A long selects
- `BACK`, `VERIFY`, and `DISPLAY OFF` are explicit menu rows

This removes the conflicting earlier suggestion that a one-button radio could
both verify identity and enter a useful menu with the same hold.

## LED dialect

The LED and screen consume the same event/state model, but the LED policy is
power-aware:

- off: healthy idle or healthy sleep
- two short pulses: a frame was received or transmitted
- slow pulse: a user-requested pairing/update operation is active
- repeating three-pulse fault: host attention required

A screenless radio reports healthy status on demand after a button press.
Healthy sleep does not breathe continuously.

## Per-personality adaptation

The renderer combines `LocalStatus` with an optional `HostSnapshot`.

- **PHY** (direct-PHY modem): local radio and host-link truth; node-only pages
  are absent until a host snapshot arrives.
- **RND** (RNode modem): host state and firmware version; peer and route truth
  remain host-supplied.
- **RET** (embedded or hosted Retinue node): identity, admitted links, peers,
  queue state, IFAC state, and bounded delivery/propagation events.
- **MCR** (MeshCore relay): repeat count, zone, and relay backlog supplied by
  its owner.
- **SNT** (Sennet): channel utilization and node count supplied by its owner.

Cross-protocol UI structs contain display facts, not Retinue, LXMF, Sennet, or
MeshCore domain types.

## Implementation shape

The shared `radio-face` crate is `no_std` and owns:

- `LocalStatus`, optional `HostSnapshot`, capabilities, and bounded events
- page selection and modal state
- one-button and two-button input reducers
- LED intents
- PANEL×LEDGER rendering over `embedded-graphics` draw targets
- fixed-size host snapshot encoding used by the direct-PHY control channel

Board firmware owns pins, display drivers, clocks, battery sensing, and local
status production. Tulle owns host transport and delivery of optional UI
snapshots. Protocol adapters decide which host facts they disclose.

Rendering is event-driven with an optional 1 Hz clock tick. The V4 OLED may
use a 1 KB monochrome framebuffer. The T114 path must not require a full
240×135 RGB framebuffer.

## Settled details

- TRAFFIC and PEERS may redraw the last bounded event after display wake.
- Names are truncated by measured glyph width, not a fixed character count.
- Status detail is host-configurable (`minimal` or `named`); `minimal` omits
  peer names and the full identity face.
- The USB-first implementation can prove display, input, local state, host
  snapshots, and RF non-regression. Current and energy claims remain gated on
  a current profiler.
