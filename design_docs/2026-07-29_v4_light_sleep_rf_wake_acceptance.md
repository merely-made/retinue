# Heltec V4 Light-sleep and direct-PHY RF-wake acceptance

**Status (2026-07-29): partially accepted on hardware.** Corrected ESP32-S3
Light-sleep resumes without resetting, the SX1262 remains usable across repeated
timer wakes, and one direct RF wake was proved end to end. Repeated RF-triggered
sleep continuity remains open. Current and energy remain unmeasured.

## Hardware and profile

- COM6: Heltec WiFi LoRa 32 V4, ESP32-S3 MAC `44:1b:f6:6a:fb:28`
- COM10: Heltec Mesh Node T114 acting as the USB-controlled RF witness
- direct-PHY profile: 906.875 MHz, SF11, BW250, CR 4/5, sync `0x2b`
- V4 build: `host-uart-low-power,rf-sleep-proof`
- safety timer: 5 seconds
- witness challenge delay: 1.5 seconds

The proof receipt carries the challenge nonce, sleep-enabled state, DIO1 wake
registration count, accepted RF-frame count, completed sleep count, last sleep
duration, and reset reason. The host rejects a reply with the wrong nonce or a
non-advancing receive count.

## Accepted evidence

### Corrected Light-sleep primitive

The crates.io `esp-hal` 1.1.1 RTC configuration reproduced an ESP32-S3 reset on
entry to Light-sleep. The failure matches upstream issue 5620. Retinue now
applies the five RTC bias and regulator fields from the merged upstream fix
locally until a released dependency contains it.

A timer-only hardware run then completed three receive, reply, sleep, and resume
cycles without resetting:

```text
cycle 1: enabled=false armed=3 sleep=0 last=0us rx=1 reset=0x01
cycle 3: enabled=true armed=7 sleep=9 last=43us rx=3 reset=0x01
TULLE V4 LIGHT-SLEEP RF WAKE PROOF PASSED: 3/3
```

The short-duration accounting bug visible in that early receipt was corrected:
returns below 1 ms are now counted as blocked attempts, not completed sleeps.
This run proves corrected resume and retained LoRa operation. It is not an
energy measurement.

### One direct RF wake

With DIO1 GPIO wake and the 5-second safety timer armed, the witness transmitted
after 1.5 seconds:

```text
cycle 1/3: enabled=false armed=3 sleep=0 last=0us rx=1 reset=0x15
cycle 2/3: enabled=true armed=7 sleep=1 last=1550499us rx=2 reset=0x15
cycle 3/3: enabled=true armed=11 sleep=1 last=1550499us rx=3 reset=0x15
Error: cycle 3: sleep counter did not advance (1 -> 1)
```

Cycle 2 slept for 1.550499 seconds, well before the 5-second timer. It then
received the matching challenge and transmitted the validated reply. UART wake
was excluded from this proof image, so the early wake was DIO1/RF rather than
host traffic.

This closes the narrow question "can an arriving direct-PHY frame wake the V4
and complete a receive/reply exchange?" It does not close repeated operation.

## Open findings

1. **Repeated RF-triggered sleep continuity.** The third challenge was received
   and answered while the V4 was awake, but the completed-sleep counter stayed
   at one. Clearing the GPIO wake enable after each DIO1 wait, clearing the GPIO
   interrupt, rejecting sleep while DIO1 is physically high, and adding a
   post-TX settle delay did not produce a second counted sleep. The settle delay
   was removed after the failed trial.
2. **Display resume.** The OLED text became garbled during a repeated
   timer-sleep run. The display needs an explicit post-wake restoration proof
   before the UI and Light-sleep paths can ship together.
3. **UART wake.** The low-power UART framing compiles and has host tests, but
   cold UART wake has not been exercised on physical UART wiring.
4. **Current and energy.** USB-only behavior proves functional wake paths, not
   current, peak current, average current, or energy. Those V0/V2 acceptance
   claims still require inline measurement equipment.

## Present boundary

The free proof establishes a working corrected sleep/resume primitive, retained
radio operation across timer wakes, and a real direct-PHY RF wake. The next
firmware task is to determine why DIO1 is not followed by another completed
sleep, then run a longer repeated-RF proof. The measurement task remains
separate and waits for a current profiler.

Upstream references:

- <https://github.com/esp-rs/esp-hal/issues/5620>
- <https://github.com/esp-rs/esp-hal/pull/5777>
- <https://github.com/esp-rs/esp-hal/commit/9487850>
