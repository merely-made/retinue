# Heltec WiFi LoRa 32 v4 direct PHY

Direct SX1262 firmware for the ESP32-S3 Heltec WiFi LoRa 32 v4. The radio pin
mapping is taken from Heltec's published v4.2 schematic:
<https://resource.heltec.cn/download/WiFi_LoRa_32_V4/Schematic/WiFi_LoRa_32_V4.2.pdf>.

The firmware uses the same USB framing and LongFast profile documented by the
T114 target. Build it with the Espressif Rust toolchain:

```text
. $HOME/export-esp.ps1
cargo +esp build -p tulle-heltec-v4-phy --release --target xtensa-esp32s3-none-elf -Zbuild-std=core
```

## On-device status face

The default USB image drives the fitted 128×64 OLED through `radio-face`:

- GPIO17: OLED SDA
- GPIO18: OLED SCL
- GPIO21: OLED reset
- GPIO0: active-low user button
- GPIO35: active-high white status LED
- GPIO36: active-low Vext enable

The local page cycle is STATUS, POWER, RADIO, and TRAFFIC. A short button press
advances the page. A press of at least 650 ms opens the one-button menu.
Healthy idle leaves the LED off; real TX/RX produces two short pulses. Display
off is a UI state and does not claim that the USB personality entered CPU
sleep.

`ui\n` is a human-readable bench probe. It reports OLED initialization,
display power, the last rendered face, and whether a physical button event has
been observed:

```text
ui=ok; display=on; screen=status; button=0; host=none
```

The direct-PHY control stream also accepts an optional versioned host
projection as `03 <lowercase-hex radio-face payload> 00` and acknowledges it
with `85 <result>`. The snapshot is decoded only at the firmware/UI edge and
expires relative to receipt time. Tulle treats its payload as opaque bytes.

The optional `ui-bench` build feature adds `fault\n` and `clear\n` commands for
on-board FAULT-face acceptance. These commands are absent from the default
image.

Hardware acceptance passed on 2026-07-22 at 906.875 MHz against a stock
LongFast node. The firmware received a 49-byte stock frame and transmitted a
47-byte Sennet transport frame which the stock node decrypted and delivered to
its client stream.

The first on-device UI hardware acceptance is recorded in
`design_docs/2026-07-28_v4_on_device_ui_acceptance.md`.
