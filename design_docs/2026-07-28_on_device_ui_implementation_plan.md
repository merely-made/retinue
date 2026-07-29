# On-device radio UI implementation plan

**Status:** U0 complete; U1 local pages, fitted button, menu, fault, and RF
receipts passed; U2 fitted display, pages, menu, activity LED, automated
hardware, and RF receipts passed; U3 automated, headed command, RF, named-page,
and expiry receipts passed; U4 real Retinue delivery and propagation projection
and fitted-panel receipts passed over the two-board RF path; U5 USB-first
display, wake, RF-continuity, and teardown receipts passed on both boards
(2026-07-29)
**Design authority:** `design_docs/2026-07-25_on_device_ui.md`
**Targets:** Heltec WiFi LoRa 32 V4 and Heltec Mesh Node T114
**First evidence path:** the connected boards over USB

## Goal

Put a truthful PANEL×LEDGER status face on the direct-PHY radios without
turning modem observations into Retinue claims. The first usable slice shows
local radio state, takes button input, reports faults, accepts an optional
bounded host snapshot, and leaves the existing direct-PHY RF path intact.

Power optimization is adjacent work. It supplies later current and energy
receipts, but it does not block the USB UI slices.

## Current boundary

The live firmware already provides:

- runtime PHY configuration
- complete-frame TX/RX over USB (and the V4 UART personality)
- TX acknowledgements
- RX RSSI/SNR
- SX1262 diagnostics on the T114
- bounded host queues and `PumpStatus` in `DirectPhySerialLink`
- V4 sleep-entry/block counters for bench diagnostics

It does not currently provide:

- a display or button board-support layer
- battery sensing
- structured firmware version reporting
- local frame counters retained for UI
- a host-to-device UI snapshot command
- BLE pairing, application OTA, or an update rollback state machine
- embedded Retinue identity, links, peers, routes, IFAC, or delivery state

Heltec documents a built-in 0.96-inch OLED on the V4. The T114 display is an
optional 1.14-inch TFT, so its BSP slice begins by confirming that the
connected board has the display fitted.

## Implementation ledger

**U0 complete.** `crates/radio-face` now contains the allocation-free status
model, expiring and versioned host snapshot codec, one-button and two-button
controllers, LED intents, and PANEL×LEDGER rendering over generic
`embedded-graphics` draw targets.

Receipts:

- 17 tests cover bounded text, privacy-preserving snapshot encoding, malformed
  wire input, settled 650/900 ms timing, capability-driven pages, wake
  consumption, menu behavior, fault preemption, LED-dark idle/sleep,
  worst-case glyph clipping, both panel bounds, and stable render hashes.
- All 24 normal/modal PNG receipts were generated under
  `target/radio-face-receipts` and visually inspected at native resolution.
- The library checks on the workspace's Rust 1.88 minimum and the
  `thumbv7em-none-eabihf` firmware target.
- Clippy passes with warnings denied for the Retinue-owned crate
  (`--no-deps`); the bounded graphics compatibility copy retains its own
  upstream lint policy.

U0 does not alter either firmware image or the direct-PHY wire protocol.

**U1 automated hardware pass.** The V4 default USB image now initializes the
OLED before the SX1262, renders firmware-owned status through `radio-face`,
tracks real configure/TX/RX transitions, drives the one-button controller and
LED intent, and keeps the UI task on a separate I2C peripheral from radio SPI.

Receipts:

- the production image reported
  `ui=ok; display=on; screen=status; button=0`
- a bench-only image rendered and cleared a FAULT face while USB status probes
  continued to answer; the production image was restored without the
  injection commands
- the final production image passed a byte-exact 4 KiB Retinue Resource in
  both directions against the T114
- the default USB release build, UART low-power check, and strict firmware
  Clippy passed
- application/partition usage is 156,400 / 16,384,000 bytes (0.95%)
- the fitted GPIO0 switch advanced through STATUS, POWER, RADIO, and TRAFFIC
  and opened the MENU ledger with a long press

DISPLAY OFF rendered `KEY TO WAKE`, powered the OLED down, and consumed the
first fitted-button press while restoring STATUS. The following USB probe
reported `display=on; screen=status; button=1`. See
`design_docs/2026-07-28_v4_on_device_ui_acceptance.md`.

**U2 automated hardware pass.** The T114 production image now drives the
optional ST7789 over SPIM0 while the SX1262 remains on its independent software
SPI bus. SPIM3 was rejected after the first physical display check because it
repeated the board's earlier false-online behavior: transfers completed but
the fitted panel remained black. A 4,050-byte monochrome framebuffer is
expanded through one 480-byte RGB565 scanline, rather than retaining a
64.8 KiB full-color frame.

Receipts:

- the production image reported
  `ui=ok; display=on; screen=status; button=none; tft=write-only`
- the firmware listens to both published user-switch candidates, P1.11 and
  P1.10, and reports the first fitted path exercised
- a bench-only image rendered and cleared a FAULT surface while USB status and
  SX1262 diagnostic probes continued to answer; production was restored and
  rejected the bench command
- the corrected v12 production image passed a byte-exact 4 KiB Retinue Resource
  in both directions against the V4, in 22.6 and 29.4 seconds
- the fitted panel visibly rendered the STATUS face after the SPIM0 correction
- short presses traversed every local panel and a long press opened the MENU
  ledger
- a headed 512-byte Resource in both directions produced the intended brief
  green-LED double pulse
- locked target check, strict firmware Clippy, and release build passed
- the production binary is 66,242 bytes; linked static RAM is 184 bytes of
  data plus 9,512 bytes of BSS

The ST7789 connection is write-only, so visual observation supplied the fitted
panel receipt. The fitted switch path and activity LED were also observed, so
U2 is complete. The dual-listened switch remains unidentified as P1.10 versus
P1.11, but that board-fact ambiguity does not block behavior. See
`design_docs/2026-07-28_t114_on_device_ui_acceptance.md`.

**U3 complete.** The direct-PHY stream now carries a
versioned `radio-face` snapshot as an opaque, separately acknowledged control
command. Tulle owns framing and acknowledgement but no display-domain types.
Both board adapters record receipt time and remove stale host facts.

Receipts:

- byte-split, outer-truncation recovery, oversize recovery, acknowledgement
  timeout recovery, and UART wake-prefix tests passed
- future-version and truncated payloads were rejected on both production
  boards, and a following valid snapshot was accepted
- minimal and named fixtures were accepted on both production boards
- the final v14 images passed a byte-exact 4 KiB Resource in both directions,
  in 27.9 and 25.7 seconds
- strict host/firmware Clippy, Rust 1.88 host checks, locked release builds, and
  unchanged Sennet/Tucket direct-PHY example checks passed
- the T114 binary is 73,506 bytes and the V4 application is 165,968 bytes
- the T114 v15 diagnostic reported `host=fresh`, then the fitted button
  traversed STATUS, POWER, RADIO, TRAFFIC, IDENTITY, LINKS, and PEERS
- after the five-second fixture expired, the fitted button returned to the
  four local pages and TRAFFIC wrapped directly to STATUS

See `design_docs/2026-07-28_host_snapshot_acceptance.md`.

**U4 complete.** `crates/outrider/examples/direct_phy_ui.rs` opens the real
Tulle carrier through Retinue, publishes projections through a cloneable
UI-only control handle, and derives every host fact from the live endpoint or
an authenticated Outrider receipt.

Receipts:

- a minimal open-interface projection and a named IFAC projection were
  accepted by both production boards
- an unannounced destination produced the host event `DELIVERY FAILED`
  without raising a firmware radio fault
- a cost-8 direct message crossed COM6 to COM10 as Data and was authenticated
  before `DIRECT DELIVERED` was published
- a 286-byte cost-8 propagation batch crossed the same IFAC carrier as a
  Resource, inserted exactly one store entry, was fetched again, and was
  authenticated before `PROP FETCHED` was published
- the admitted Resource session supplied the real link/interface fact and the
  endpoint supplied the bounded queue-depth observation
- the headed run exposed and fixed a real MTU bug: `set_link_mtu(247)` had
  silently clamped to 255, causing eight-byte IFAC Resource parts to exceed
  the 255-byte carrier cap and be refused by queue admission
- the exact 247-byte logical plus eight-byte IFAC boundary now has an
  in-memory Tulle carrier regression
- a staged headed run held each real snapshot for 20 seconds; the fitted T114
  visibly showed the current host-event ticker and `IFAC ON`

See `design_docs/2026-07-29_retinue_host_projection_acceptance.md`.

## Authority model

| Fact | Owner now | First UI behavior |
| --- | --- | --- |
| board, firmware, uptime | firmware | show locally |
| radio init/fault | firmware | boot check or fault preemption |
| applied frequency/SF/BW/CR/power/sync | firmware | read-only RADIO page |
| raw TX/RX, last RSSI/SNR, TX result | firmware | TRAFFIC page |
| USB/UART attachment | firmware/Tulle | STATUS page |
| battery/voltage | board BSP | em-dash until measured |
| display off | UI controller | local menu action |
| CPU Light-sleep | V4 UART power gate | report policy/state, never equate with display off |
| node identity/fingerprint | Retinue host or embedded node | optional host page |
| links, peers, routes | Retinue host or embedded node | optional host pages |
| Tulle queue depth and pump fault | Tulle host | optional host fields |
| IFAC enabled/interface label | Retinue host | optional host field |
| direct delivery/propagation event | Outrider/Retinue host | bounded ticker event |
| PHY preset choice | host today | read-only until authority handshake exists |

`HostSnapshot` is an explicitly lossy display projection. It is not a second
Retinue model and cannot be used to reconstruct protocol state.

## Proposed seams

### New shared crate: `crates/radio-face`

`radio-face` is `no_std`, allocation-free in firmware, and protocol-neutral.

- `status.rs`
  - `LocalStatus`
  - `HostSnapshot`
  - `Capability`
  - bounded strings and `UiEvent`
- `controller.rs`
  - page registry
  - modal precedence
  - display timeout
  - input reducer for one or two buttons
  - LED intent
- `render.rs`
  - common PANEL×LEDGER primitives
  - `Oled128x64` and `Tft240x135` layout metrics
  - actual `MonoFont` assets
- `wire.rs`
  - versioned, length-delimited host snapshot command
  - reject unknown versions, impossible lengths, and invalid UTF-8
  - fixed maximum payload, with no secret key material

Golden render tests use `embedded-graphics-simulator` or an equivalent
off-target draw target under `std`; the firmware library remains `no_std`.

### Direct-PHY host/control path

- `crates/selvage/src/lib.rs`
  - reserve the UI snapshot command marker and keep it distinct from wake,
    TX, and profile configuration
- `crates/tulle/src/direct_phy.rs`
  - encode the UI command and test split/invalid input
- `crates/tulle/src/direct_phy_serial.rs`
  - add a cloneable `DirectPhyControl` handle before the radio is moved into
    an interface driver
  - coalesce UI snapshots so stale display work cannot build a queue beside RF
    traffic
  - run every write through the existing wake-aware `write_command`
  - expose `publish_ui_snapshot` on the control handle without widening
    `RadioLink`
- `crates/tulle/examples/direct_phy_ui.rs`
  - query/inject local states over USB
  - publish minimal and named host snapshots
  - inject and clear test faults only in a test build

The snapshot stays optional. Sennet, Tucket, and Retinue users of
`DirectPhySerialLink` continue to work without depending on Retinue UI types.
An attaching host chooses a bounded validity interval; the board expires the
snapshot when that interval passes. Snapshot refresh is rate-limited and
coalesced because display state never outranks radio traffic.

### V4 firmware

- `firmware/heltec-v4-phy/src/board.rs`
  - verified OLED, button, LED, display-power, and battery pins
- `firmware/heltec-v4-phy/src/ui.rs`
  - display driver
  - local status producer
  - controller/render task
- `firmware/heltec-v4-phy/src/main.rs`
  - initialize the display before SX1262 setup
  - report real boot stages and faults
  - update local status at configure/TX/RX boundaries
  - accept the UI snapshot command without disturbing frame parsing

The default USB build is the first hardware target. The UART low-power build
follows only after display I/O and the existing sleep gate have an explicit
interaction policy. Turning the display off must release its peripheral
activity before the gate can sleep.

### T114 firmware

- `firmware/t114-phy/src/board.rs`
  - confirm fitted TFT, controller, pins, rotation, button, backlight, and LED
- `firmware/t114-phy/src/ui.rs`
  - direct or line-buffered 240×135 renderer, avoiding a full RGB framebuffer
- `firmware/t114-phy/src/main.rs`
  - the same local-status transitions and snapshot command as the V4

Display traffic must be scheduled outside SX1262 transactions. If the display
and radio share a bus, the BSP must use one explicit bus owner rather than
independent drivers.

### Retinue host projection

A small adapter, initially in the headed example rather than `Endpoint`, maps
observable host state to `HostSnapshot`:

- interface/pump state and queue depth
- IFAC active/inactive, never keys
- local identity name/address/fingerprint when detail policy permits
- admitted link and peer summaries
- bounded delivery events (`DIRECT DELIVERED`, `PROP STORED`,
  `PROP FETCHED`, `FAILED`)

The endpoint core does not learn about screens. Promotion into a reusable host
adapter happens only after the headed example proves a second consumer or
stable repeated use.

## Execution order and receipts

### U0. Renderer and controller proof

Build `radio-face` without touching either firmware main loop.

Done when:

- all normal and modal faces render at 128×64 and 240×135
- golden images use the actual bitmap fonts
- the 128×64 output obeys the five-line rule with worst-case values
- long strings truncate by glyph width and never panic
- one-button and two-button event traces cover wake consumption, menu
  navigation, back, fault preemption, and display timeout
- healthy idle and sleep produce `LedIntent::Off`
- `cargo test -p radio-face` and `cargo clippy -p radio-face` pass

Stop if the fonts cannot keep the header plus four useful body rows legible at
128×64. Rework the layout before adding board drivers.

### U1. V4 local USB face

Add the V4 OLED, button, and LED BSP. Show only firmware-owned values.

Done when:

- the connected V4 shows BOOT, STATUS, RADIO, TRAFFIC, and FAULT
- displayed profile values change after a real USB configure command
- an RF receive updates frame count and exact RSSI/SNR
- a transmit updates count/result only after the firmware TX result exists
- an SX1262 initialization failure remains readable while USB status probes
  still answer
- short/long/chord input traces match the controller tests
- display off consumes the wake press and does not claim CPU sleep
- the existing direct-PHY byte and Resource receipts pass again

Stop if OLED I/O changes the radio profile, delays DIO1 service enough to lose
frames, or consumes pins used by the current host/radio path.

### U2. T114 local USB face

Port the same state and controller to the optional TFT.

Done when:

- fitted-display observation and BSP facts are recorded
- the connected T114 shows the same local states in 240×135 landscape
- screen update does not require a full RGB framebuffer
- TX timeout diagnostics still reach the host and the FAULT face
- bidirectional V4↔T114 direct-PHY frame and Resource receipts pass
- firmware flash/RAM usage is recorded

If the connected T114 lacks its optional display, complete its LED/button
adapter and retain the TFT renderer as an off-target receipt. That is a
hardware variant, not a failed UI architecture.

### U3. Versioned host snapshot

Add the optional direct-PHY control command and headed injector.

Done when:

- missing host state leaves node-only pages absent
- a minimal snapshot adds host health without names
- a named Retinue snapshot adds IDENTITY/LINKS/PEERS
- stale snapshots expire to unavailable rather than persisting as truth
- malformed, oversized, truncated, and future-version snapshots are rejected
  without desynchronizing the following TX/config command
- UART wake-prefix tests cover snapshot commands too
- Sennet and Tucket direct-PHY examples compile unchanged

Stop if the snapshot requires protocol domain types in `radio-face`, Tulle, or
firmware. Project display facts at the host edge instead.

### U4. Retinue headed receipt

Wire a real Retinue host projection in an example or acceptance harness.

Done when the device visibly distinguishes:

- modem online from Retinue link admitted
- open interface from IFAC-enabled interface
- direct delivery from propagation storage/fetch
- local radio fault from host delivery failure
- named and minimal detail policies

Use the existing two-board RF path. The receipt must include at least one real
direct delivery and one real propagation event, not injected UI strings.

### U5. Display power and field behavior

This is the first meter-gated UI slice.

USB-first work can still verify display-off commands, LED-dark policy, button
wake, and RF continuity. Current and energy claims wait for instrumentation.

Done when:

- OLED off, TFT backlight off, LED off, and UART Light-sleep are measured
  separately
- display refresh does not keep the V4 sleep gate blocked
- a radio wake can update state without leaving the display permanently on
- quiet and representative-workload energy is recorded against the
  display-less firmware baseline

**USB-first progress.** The fitted V4 menu powered the OLED down after the
`KEY TO WAKE` face, and the next button press was consumed while restoring
STATUS. A first continuity attempt exposed a host-control pitfall: asserting
DTR on native ESP32-S3 USB reset the board at attachment or teardown and could
make RF appear to wake the screen.

`DirectPhySerialConfig` now exposes DTR and RTS policy. Defaults retain DTR for
nRF CDC and keep RTS deasserted for ESP32 safety; the V4 harness can keep DTR
deasserted. With that policy, a 256-byte Resource passed byte-exact in both
directions while the V4 display was off. After RF, a 45-second postflight,
serial teardown, and a new DTR-low diagnostic attachment, the V4 still reported
`display=off; screen=display-off; button=1`.

The fitted T114 menu then powered down its TFT backlight from TRAFFIC. One
button press restored TRAFFIC without advancing. A final Resource passed in
both directions while both displays were off; after RF and teardown, fresh
diagnostics still reported DISPLAY OFF on both boards.

See `design_docs/2026-07-29_display_power_field_acceptance.md`.

## Deferred product states

These remain out of the first implementation:

- editable PHY presets, until host/device authority and rejection are explicit
- BLE pairing, until a real BLE management channel performs numeric comparison
- OTA, until each target has image verification, rollback, and progress facts
- native provisioning, identity, peer, and route ownership on the modem
- message text on the radio display

Their renderer states may be golden-tested after U0, but they do not enter
firmware menus until the owning feature exists.

## Pitfalls and checks

- **USB sleep contradiction:** display off and CPU Light-sleep are separate
  states.
- **Dual profile authority:** the host owns configuration today; the panel is
  read-only.
- **Opaque-frame temptation:** receiving a Retinue packet is not proof of a
  peer, route, or admitted link.
- **Optional T114 display:** record the actual fitted hardware before choosing
  its BSP.
- **Font optimism:** browser fonts and 4× mockups are not evidence of
  128×64 legibility.
- **TFT RAM:** avoid a 64.8 KB RGB565 framebuffer unless measured memory headroom
  justifies it.
- **Radio starvation:** render on events, coalesce updates, and keep display
  transfers outside radio-critical sections.
- **Privacy:** host policy controls names/fingerprint; minimal mode is always
  available.
- **Wire recovery:** every new command needs fragmentation and resynchronization
  tests because CDC may split at any byte.

## Next implementation move

The USB-first UI plan is complete. Return to U5 only with instrumentation for
separate OLED, TFT backlight, LED, UART Light-sleep, and representative-workload
current/energy receipts. Do not infer those claims from USB behavior.

## References

- `design_docs/2026-07-23_direct_phy_resource_acceptance.md`
- `design_docs/2026-07-24_low_power_uart_personality.md`
- `design_docs/2026-07-28_ifac_interop.md`
- `design_docs/2026-07-28_outrider_direct_phy_delivery.md`
- `design_docs/2026-07-28_outrider_propagation_persistence.md`
- `design_docs/2026-07-28_host_snapshot_acceptance.md`
- `design_docs/2026-07-29_retinue_host_projection_acceptance.md`
- `design_docs/2026-07-29_display_power_field_acceptance.md`
- [Heltec WiFi LoRa 32 documentation](https://docs.heltec.org/en/node/esp32/wifi_lora_32/index.html)
- [Heltec Mesh Node T114 documentation](https://docs.heltec.org/en/node/nrf/mesh_node_t114/index.html)
