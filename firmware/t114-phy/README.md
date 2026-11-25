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

`name` reports the public device label. `name set T114` saves a label of up to
16 printable ASCII characters and reboots; `name clear` clears it. Names cannot
start or end with a space. The label appears on the boot and default status
screens and in `status`, independently of relay identity, host identity and
radio profile. Setting the same name again does not write flash or reboot.
Older settings upgrade without changing the identity, region or channel.

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

Build it with:

```text
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf
```

Images are not kept in the repository (protocol_parity D4). The staging step in
[the packages README](../packages/README.md) runs this build from HEAD with
`RETINUE_FIRMWARE_REVISION` set, converts the ELF with `rust-objcopy -O binary`, and
encodes the UF2 with `linkboy make-uf2 BIN UF2 0x26000 0xADA52840`, Linkboy's
in-repository equivalent of Adafruit's `uf2conv.py`. The one retained built image is
`tulle-t114-phy-v47.zip`, the serial-DFU recovery package named by
[t114-v47.toml](../packages/t114-v47.toml): a recovery image must exist before anything
can be built.

Native-node builds reserve 65,536 announce ordinals per boot by default. An owner building
for a different reboot/erase-cadence tradeoff can set the decimal Cargo build environment
variable `RETINUE_ANNOUNCE_LEASE_ORDINALS`; the build rejects zero and values outside the
40-bit wire range. The selected lease is compiled into the board policy. Changing it does
not alter the reservation record format.

The September 30 current package (now built from
[its template](../packages/t114-current.template.toml)) identified
source `711816a8124bd58cd8cc314ec4de5009be7df166` and image `t114-native`
in live status. Its [physical follow-up](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/firmware-refresh/follow-up/README.md)
records installation, retained identity/settings sequence, a 1024-byte native
resource exchange, guarded admission and signed announce ordinals advancing
across soft resets. Same-image UF2 recovery and physical power-cut acceptance
remain open. Those bytes are no longer in the tree; a staged build from a later
commit is a different image. Older UI receipts below describe earlier artifacts.

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
`Code/testing/murm/design_docs/2026-07-28_t114_on_device_ui_acceptance.md`.

The v15 production image added host-snapshot state to `ui\n`. With a fresh
named fixture, the fitted button traversed STATUS, POWER, RADIO, TRAFFIC,
IDENTITY, LINKS, and PEERS. After a five-second fixture expired, the host-only
pages disappeared and TRAFFIC wrapped directly to STATUS.

The October 16 [physical memory continuation](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/protocol-parity-memory/physical/README.md)
measured an attended `memory-bench` image with synthetic retained Retinue,
Sennet and Tucket state. That feature, the Tucket-only `tucket-endpoint` service
and its contact store were withdrawn on 2026-11-14 with the resident protocols;
the receipt stays as history. The contact flash pair in `memory.x` stays reserved.

For the measured large serial DFU transfer, the inherited eight-second watchdog
outlasted a software reset and interrupted the helper's erase wait. The tiny
[`dfu_watchdog_handoff` example](examples/dfu_watchdog_handoff.rs) provides an
attended application-only bridge: wait for the inherited watchdog reset, then
request serial DFU without restarting WDT. Use it only with an identified board
and a verified restoration application ready. It changes neither the bootloader
nor durable state and does not replace the current package/recovery procedure.
