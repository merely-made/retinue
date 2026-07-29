# Heltec T114 on-device UI acceptance

**Date:** 2026-07-28
**Status:** passed
**Plan rung:** U2 in `2026-07-28_on_device_ui_implementation_plan.md`

## Hardware and board facts

- application identity: USB VID/PID `1915:521f`, serial
  `TULLE-T114-01`
- serial bootloader identity during the flash: `239a:0071`
- frequency: 906.875 MHz
- bandwidth: 250 kHz
- direct-PHY profile: SF8, CR 4/5, sync `0x12`, 17 dBm

COM10 and COM4 were observations during this receipt, not persistent
identities.

Heltec documents the optional panel as a 1.14-inch, 240×135, four-wire SPI
ST7789V2. The Rev. 2.1 board schematic assigns:

- TFT power/reset/DC/MOSI/SCK/CS/backlight:
  P0.03/P0.02/P0.12/P1.09/P1.08/P0.11/P0.15
- green LED: P1.03, active low
- SX1262 NSS/SCK/MOSI/MISO/reset/busy/DIO1:
  P0.24/P0.19/P0.22/P0.23/P0.25/P0.17/P0.20

The schematic names P1.11 for the user switch while the maintained T114 board
variant names P1.10. Both are otherwise unused by this image, so the button
task listens to both and records which one was exercised.

The TFT has no readback line. A successful SPI transfer cannot distinguish a
fitted panel from an unpopulated board option, so fitted-display evidence must
be visual rather than invented from the bus result.

## Image and memory

The default production image was built and packaged as:

```text
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf --locked
llvm-objcopy -O binary C:\t\graphshell-target\thumbv7em-none-eabihf\release\tulle-t114-phy firmware\t114-phy\tulle-t114-phy-v11.bin
adafruit-nrfutil dfu genpkg --dev-type 0x52 --application firmware\t114-phy\tulle-t114-phy-v11.bin --application-version 11 --sd-req 0xFFFE firmware\t114-phy\tulle-t114-phy-v11.zip
```

Serial DFU reported `Device programmed`.

Production footprint:

```text
binary        66,274 bytes
text          66,070 bytes
data             184 bytes
bss            9,512 bytes
```

The UI retains a 4,050-byte one-bit framebuffer and expands one 480-byte
RGB565 scanline at a time. A full 240×135 RGB565 framebuffer would require
64,800 bytes.

## Local UI receipt

The production image reported:

```text
ui=ok; display=on; screen=status; button=none; tft=write-only
tulle/t114 phy online; sx1262 online; spi=software; irq=poll; sync=2b reg=24b4; longfast=906875000
```

`ui=ok` means the ST7789 initialization and latest full frame were transferred
without an nRF SPIM error. `tft=write-only` prevents that result from being
misread as fitted-panel detection.

The first physical observation found the backlight on but the panel black.
That falsified the initial SPIM3 implementation despite its successful driver
result. SPIM3 had already produced a false-online SX1262 on this board, so the
display was moved to the independent SPIM0 peripheral at 8 MHz.

## Fault receipt and production restore

A temporary `--features ui-bench` image accepted local fault injection:

```text
ui bench fault set
ui=ok; display=on; screen=fault; button=none; tft=write-only
tulle/t114 phy online; sx1262 online; spi=software; irq=poll; sync=2b reg=24b4; longfast=906875000
84 00 00 00 00 24 b4
ui bench fault cleared
ui=ok; display=on; screen=status; button=none; tft=write-only
```

The default v11 production image was then restored. Sending `fault\n` returned
the ordinary unknown-command TX error `82 03 00 00`, proving the bench command
surface was absent. The subsequent SPIM0 correction is the v12 production
image and has the same default command surface.

The real TX-timeout path publishes `TX TIMEOUT` to the same fault state before
emitting its existing SX1262 diagnostic event to the host.

## RF non-regression

The corrected v12 production image passed the existing bidirectional 4 KiB
Retinue Resource receipt:

```text
radios online: COM6=client, COM10=server
discovery: resource destination announced over direct PHY
publish: client to server 4096 bytes passed in 22.6s
fetch: server to client 4096 bytes passed in 29.4s
RETINUE DIRECT-PHY RESOURCE HEADED PASSED
```

This exercised production UI redraws alongside real host configuration, TX
acknowledgements, RX RSSI/SNR, and Resource retransmission.

## Build receipts

- locked `thumbv7em-none-eabihf` check: passed
- firmware Clippy with Retinue-owned warnings denied: passed
- locked production release build: passed
- production binary reproduced byte-for-byte after the bench build

## Physical receipt

The fitted panel visibly rendered the STATUS face after the v12 SPIM0
correction. This closes fitted-display detection and the black-screen
regression.

Short presses advanced through every local panel, and a long press opened the
MENU ledger.

A headed 512-byte Resource in both directions produced the intended brief
green-LED double pulse while publish and fetch completed in 4.9 and 5.2
seconds.

The later ad hoc `ui\n` probe did not return after the headed RF harness, so it
did not identify P1.10 versus P1.11. Physical navigation proves that one of the
dual-listened switch paths is fitted. Exact pin identification is board
bookkeeping, not an open U2 behavior gate.

## Host snapshot receipt

The v15 production image exposed snapshot handoff state in `ui\n`. A fresh
named fixture produced:

```text
ui=ok; display=on; screen=traffic; button=p1.10; host=fresh; tft=write-only
```

The fitted button traversed:

```text
STATUS -> POWER -> RADIO -> TRAFFIC -> IDENTITY -> LINKS -> PEERS
```

After the named refresher was stopped and a five-second expiry fixture elapsed,
the fitted button traversed only:

```text
STATUS -> POWER -> RADIO -> TRAFFIC -> STATUS
```

This closes the U3 physical gate. The named host projection changes the live
page registry, and stale identity, link, and peer facts leave the display when
their receipt-relative validity ends.

The flashed v15 binary is 73,666 bytes; its serial-DFU ZIP is 74,542 bytes.

## Sources

- [Heltec Mesh Node T114 product page](https://heltec.org/project/mesh-node-t114/)
- [Heltec Mesh Node T114 Rev. 2.1 schematic](https://resource.heltec.cn/download/Mesh_Node_T114/schematic/MeshNode-T114_V2.1.pdf)
- [Heltec 1.14-inch TFT specification](https://resource.heltec.cn/download/Mesh_Node_T114/1.14inch%20LH114T-IF03%20VER%20C.pdf)
- [Maintained T114 board variant](https://github.com/meshtastic/firmware/blob/develop/variants/nrf52840/heltec_mesh_node_t114/variant.h)
