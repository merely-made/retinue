# V1 and V2: the low-power UART personality

**Status (updated 2026-07-29): implemented, compile-verified, and partly
hardware-proved.** A free functional proof established corrected ESP32-S3
Light-sleep/resume, retained radio operation across timer wakes, and one
direct-PHY DIO1 wake with a validated RF reply. Repeated RF-triggered sleep,
physical UART wake, display restoration, current, and energy remain open. See
`design_docs/2026-07-29_v4_light_sleep_rf_wake_acceptance.md` for the receipt.

Plan: `mere/design_docs/mere_docs/implementation_strategy/2026-07-24_low_power_managed_network_plan.md`.

## What was built

**V1, the UART host personality.** The ESP32-S3's USB Serial/JTAG peripheral stops its clocks
in Light-sleep and the host may fail to re-enumerate it on wake, so the low-power work needs a
host link that survives sleeping.

- Two mutually exclusive firmware features: `host-usb` (default, unchanged) and
  `host-uart-low-power` (UART0 on the header, GPIO44 RX / GPIO43 TX, 115200).
- The host I/O helpers are generic over `embedded_io_async::{Read, Write}`, so both
  personalities run one direct-PHY implementation rather than two that drift.
- `selvage::WAKE_BYTE` (`0x00`, statically asserted not to collide with `CMD_TX` or
  `CMD_CONFIG`). A UART wake consumes the edges that triggered it, so a command sent cold
  arrives truncated — worse than no command, because it desynchronises the parser for
  everything after. The host sends a run of wake bytes, waits, then writes the real command.
  The firmware discards that byte **only at a frame boundary**, since the same value is legal
  inside a length field or payload.
- Host side: `DirectPhySerialConfig.wake: Option<WakeSequence>`, preset
  `DirectPhySerialConfig::low_power_uart()`, and a single `write_command` path every command
  goes through so no site can forget the wake.

**V2, guarded Light-sleep.** `firmware/heltec-v4-phy/src/power.rs`.

- The idle hook (`esp_rtos::start_with_idle_hook`) sleeps only when a gate is open, otherwise
  falls back to `wait_for_interrupt`.
- The gate is a **counter, not a flag**, so nested holds compose and an inner release cannot
  re-open a gate an outer hold still needs. `Awake` is an RAII hold: it cannot be leaked on an
  early return or an error path.
- The gate is closed around radio setup, and around everything after the idle `select` —
  every path that touches SPI, the radio, or the host link. It is open at exactly one point:
  the `select` where both sides are merely waiting and the radio is listening on its own.
- **It is also closed whenever a command is half-parsed** (`usb_command_len > 0`). Sleeping
  mid-command would let the wake eat the continuation bytes and corrupt the frame.
- Wake sources: `GpioWakeupSource` (DIO1 on GPIO14, the radio) and
  `Uart0WakeupSource` (the host).
- The module compiles in **both** builds: under `host-usb` it is a no-op stub, so the shared
  command loop takes identical holds either way and the two builds cannot drift.
- Diagnostics: a `sleep\n` host command returns `EVENT_DIAGNOSTIC` plus two little-endian
  `u32`s — sleep entries, and idles blocked by the gate. The USB build answers zeros rather
  than nothing, so the bench can tell "not sleeping" from "wrong firmware flashed".

## What is actually established

- Both V4 personalities and the T114 firmware **compile** (xtensa and thumbv7em).
- 277 host tests pass, clippy and fmt clean, both retinue feature configs green.
- Three host tests cover the wake protocol: the preamble precedes status, configure, and
  transmit without bleeding into the command; the USB configuration emits no prefix; a
  firmware event delivered a byte at a time still reassembles.

That was the pre-bench boundary. The 2026-07-29 hardware receipt now proves a
functional sleep/resume path and one direct RF wake. It still does not prove
that the board draws less current.

## What the bench must settle

In rough order of what would invalidate the design fastest:

1. **Does `lora.rx()` hold an Embassy timer?** This is the sharpest risk. The gate is open
   during the idle `select`, which is where `lora.rx()` is pending. The runtime's time source
   does not advance in Light-sleep, so if the driver has a timer armed there, sleeping stalls
   it. `RxMode::Continuous` should await the DIO1 interrupt with no timeout, which is why it
   was chosen — but that is reasoning, not a measurement. Symptom: receive silently stops
   after the first sleep.
2. **Is the UART wake threshold right?** `UART_WAKE_THRESHOLD = 3` rising edges. A `0x00`
   byte on an idle-high line produces one rising edge (its stop bit), so the 8-byte default
   preamble should give roughly five bytes of margin past the threshold. Verify a cold command
   is never truncated; raise the preamble before lowering the threshold.
3. **Does DIO1 wake the chip via `GpioWakeupSource`?** If not, packets are missed while
   asleep, which the 1000-frame proof below is designed to catch.
4. **Single core?** The idle hook parks the RTC behind a critical section, which serialises
   access, but a second core running its own idle hook while one sleeps is untested. Confirm
   `esp_rtos::start_with_idle_hook` brings up one core here.

## Bench procedure (V0 baseline, then V2)

V0 has no code change; it is the control the V2 claims are measured against. Use one
consistent supply path and record, for every run: supply and voltage, board and firmware
revision, firmware commit, radio profile, steady and peak current, observation interval, and
energy over it.

Runs: current firmware in continuous receive; SX1262 standby; UART firmware with sleep
disabled; UART firmware sleeping with the radio receiving; a representative packet workload.

Then the V2 proofs: 100 consecutive UART wake/configure cycles; 1,000 received RF frames with
no loss attributable to sleeping; 100 transmissions whose first complete UART command survives;
the existing bidirectional 4 KiB Resource proof; quiet continuous-RX current and
representative-workload energy; and the default USB build on the control board as a
regression check.

**Done when** quiet continuous RX falls at least 5x on the same supply path; whole-board
continuous RX is at or below 12 mA or the receipt names the remaining consumer; RSSI/SNR stay
comparable to the awake control; no wake cycle corrupts a host command; and the default USB
firmware is unchanged.

Ports are observations, not identities — re-query by USB VID after every flash
(`239A` = nRF/T114, `303A` = ESP32/V4).
