# T114 direct PHY

Independent Embassy firmware for the Heltec T114's nRF52840 and SX1262. Pin
assignments and the DIO2 antenna-switch and DIO3 TCXO wiring come from Heltec's
published [Rev. 2.1 schematic](https://resource.heltec.cn/download/Mesh_Node_T114/schematic/MeshNode-T114_V2.1.pdf).
The radio uses Meshtastic LongFast modulation at 906.875 MHz, 17 dBm, and the
documented sync word `0x2B` (`0x24B4` in the SX1262 registers).

The same image drives the optional 240×135 ST7789 TFT through its separate
write-only pins and the nRF52840's SPIM0 peripheral. It renders the shared
`radio-face` STATUS, POWER, RADIO, TRAFFIC, MENU, and FAULT surfaces from a
4,050-byte monochrome framebuffer, expanding one 480-byte RGB565 scanline at a
time. The radio remains on its independent software SPI bus. SPIM3 is not used:
it previously produced a false-online SX1262 on this board.

Heltec's Rev. 2.1 schematic and maintained board variant disagree about the
user-switch pin. The firmware safely listens to both otherwise-unused
candidates, P1.11 and P1.10, and `ui\n` reports which fitted path was seen.
The green activity/fault LED on P1.03 is active low.

The SX1262 uses a software mode-0 SPI bus on the schematic pins. The original
SPIM3 integration accepted commands but read the sync-word registers as
`0x0000` and never asserted TX-done. The software bus reads back `0x24B4` and
has passed RF acceptance in both directions.

The USB CDC protocol carries opaque radio packets and a runtime radio profile:

- host transmit: `01 <length:u16-le> <packet>`
- radio receive: `81 <length:u16-le> <rssi:i16-le> <snr:i16-le> <packet>`
- transmit result: `82 <result:u8> <length:u16-le>` where result zero is success
- host configure: `02 <frequency:u32-le> <bandwidth:u32-le> <sf:u8> <cr-denominator:u8> <preamble:u16-le> <sync:u8> <flags:u8> <power:i8>`
- configure result: `83 <result:u8>` where result zero is success
- SX1262 diagnostic: `84 <irq:u16-le> <errors:u16-le> <sync-msb:u8> <sync-lsb:u8>`
- host UI snapshot: `03 <lowercase-hex radio-face payload> 00`
- UI snapshot result: `85 <result:u8>` where zero is accepted, one malformed,
  two unsupported version, and three oversized

CDC transfers may split a command at any byte. The shared bounded parser
reassembles transmit, configure, and snapshot commands. The snapshot body is
zero-free and zero-delimited so a truncated outer snapshot ends at the next
wake boundary instead of consuming the following TX/config command. The
firmware chunks receive events into 64-byte USB packets. `status\n` and
`sync\n` remain available as human-readable probes. `radio\n` emits the binary
diagnostic event. `ui\n` reports display task, current surface, and observed
button path, plus `host=none|pending|fresh`; `tft=write-only` is explicit
because a successful SPI transfer cannot detect whether the optional panel is
populated. `bootloader\n` enters the board's serial-only DFU mode without a
physical double-reset.

`lxmf\n` and `lxmf stamp\n` are the board's own account of whether it can read
LXMF, rather than an inference from the fact that outrider linked. Each decodes
or scores a stock LXMF 0.9.6 artefact baked into flash and checks it against the
answer the pinned oracle gave, so a divergence is loud on the board and carries
the id it computed. They are two probes because the stamp is slow enough that a
host reading to the first newline would leave before it finished. Verified
2026-08-07 on v45, COM10:

```text
lxmf codec ok title=5 content=4 fields=8 took=183us heap=120
lxmf stamp ok value=14 rounds=1000 took=1868ms heap=0
```

Two figures there are worth carrying forward. The stamp costs **zero heap**,
because scoring streams each round through the hash instead of materialising a
256 KB workblock the board could never hold. And it costs **1.87 s of CPU per
stamp**, which bounds what a board can be asked to weigh: message-cost stamps
run 3,000 rounds, so roughly 5.6 s each, and nothing here can check inbound
proof-of-work at any rate.

Build and package for both supported bootloader paths:

```text
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf
cargo objcopy -p tulle-t114-phy --release --target thumbv7em-none-eabihf -- -O binary firmware/t114-phy/tulle-t114-phy-v15.bin
adafruit-nrfutil dfu genpkg --dev-type 0x52 --application firmware/t114-phy/tulle-t114-phy-v15.bin --application-version 15 --sd-req 0xFFFE firmware/t114-phy/tulle-t114-phy-v15.zip
python path/to/uf2conv.py -c -b 0x26000 -f 0xADA52840 -o firmware/t114-phy/tulle-t114-phy-v15.uf2 firmware/t114-phy/tulle-t114-phy-v15.bin
```

The Heltec bootloader's documented path is to double-press reset and copy the
UF2 onto the `HT-n5262` drive. Serial DFU also accepts the ZIP. The application
address is `0x26000` for the board's S140 v6 bootloader. The same SoftDevice
layout reserves RAM below `0x20006000`; the linker script keeps Embassy state
above that boundary.

On 2026-07-23 the application enumerated as USB VID/PID `1915:521f` on COM10
and read back sync registers `0x24B4`. A headed Sennet receipt against the
Heltec v4 direct-PHY target on COM6 passed encrypted text in both directions:
T114 to v4 at -14 dBm / 6.0 dB SNR, then v4 to T114 at -84 dBm / 5.0 dB SNR.

On 2026-07-28 the corrected v12 production image reported
`ui=ok; display=on; screen=status; button=none; tft=write-only`, retained clean
SX1262 diagnostics, and passed a byte-exact 4 KiB Retinue Resource in both
directions against the V4. The fitted panel visibly rendered the STATUS face
after the SPIM0 correction, and the fitted button traversed every page and
opened the menu. A headed Resource produced the intended brief green-LED double
pulse. Exact P1.10-versus-P1.11 switch identification remains board
bookkeeping; the dual-listened input behavior is accepted. See
`design_docs/2026-07-28_t114_on_device_ui_acceptance.md`.

The v15 production image added host-snapshot state to `ui\n`. With a fresh
named fixture, the fitted button traversed STATUS, POWER, RADIO, TRAFFIC,
IDENTITY, LINKS, and PEERS. After a five-second fixture expired, the host-only
pages disappeared and TRAFFIC wrapped directly to STATUS.
