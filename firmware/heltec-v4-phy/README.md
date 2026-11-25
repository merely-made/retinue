# Heltec WiFi LoRa 32 v4 direct PHY

Direct SX1262 firmware for the ESP32-S3 Heltec WiFi LoRa 32 v4. The radio pin
mapping is taken from Heltec's published v4.2 schematic:
<https://resource.heltec.cn/download/WiFi_LoRa_32_V4/Schematic/WiFi_LoRa_32_V4.2.pdf>.

The firmware uses the same USB framing and LongFast profile documented by the
T114 target.

USB startup is quiet: hosts send `status` after opening to obtain the boot and
identity report. This also applies to status-only recovery after radio/control
initialization failures. Waiting to transmit a banner before reading USB can
block modem startup while the host port is closed across a software reset.
The UART low-power image retains its startup announcement.

An uncommissioned board also watches for a physical first-owner gesture for
20 seconds after boot. When both control journals are blank, a read-only USB
query (`status`, `name`, `region`, `channel`, `sync` or `ui`) selects ordinary
service immediately; the request is retained and answered by the normal
parser. First-owner frames retain the full physical-gesture requirement, and
pending/corrupt state cannot use this shortcut. Use the first-owner client
before opening an ordinary modem client when commissioning on that boot.

Build it with the Espressif Rust toolchain:

```text
. $HOME/export-esp.ps1
cargo +esp build -p tulle-heltec-v4-phy --release --target xtensa-esp32s3-none-elf -Zbuild-std=core
```

The image is not kept in the repository. A flashable package (image plus its
manifest) is produced by the staging step described in
[the packages README](../packages/README.md), which runs this build from HEAD with
`RETINUE_FIRMWARE_REVISION` and `SOURCE_DATE_EPOCH` set.

## Node build

The `node` feature builds the USB image as a standalone Reticulum transport
node (boot line `image=v4-node-usb`):

```text
cargo +esp build -p tulle-heltec-v4-phy --release --features node --target xtensa-esp32s3-none-elf -Zbuild-std=core,alloc
```

Only this build links an allocator (radio-hand's tracking heap over a fixed
96 KiB array, provisional until a measured run), the shared node channel
(`Node<32, 8, 4>` behind `radio_hand::channel::node::NodeChannel`) and
Retinue's allocating core. The modem image carries none of them. `node`
cannot be combined with `host-uart-low-power` or `rf-sleep-proof`.

`channel node` stores the node choice and reboots; the modem image still
answers `channel node unavailable on this board`. When the stored channel asks
for the node, boot runs WN1 control recovery first, then reserves this boot's
announce lease in flash before receive is first armed, builds the node from the
stored identity, and enters the node loop, which never returns. A failed lease
or a refused node keeps the modem for repair, with `timebase=fault
FALLBACK=modem` on the boot line and a TIMEBASE fault on the screen. `channel
modem` and `channel rnode` stay available as recovery channels.

Relaying follows the control configuration in force. A claimed board relays
announces and packets as the `relay_announces` and `relay_packets` of the last
configuration it applied say (boot recovery, provisional apply, revert or
expiry); an unclaimed board relays both. A change applies from the next
packet, with routes and carried links kept, and the first-hop allowance
follows the profile then running. The hop limit and route and link lifetimes
stay at the transit defaults whatever the configuration's `max_hops` says. Only the node build
accepts the two flags in a candidate or first-write configuration; the modem
image still refuses them, and a modem image booted on a board whose committed
configuration has either flag on fails WN1 control recovery into the
status-only boot with fault 12, whose status line names the cause and the
fix: `control runtime boot failed: committed config relays; flash the node
image`.

The node needs nothing on USB and writes no banner. Every host write in the
node loop is bounded at 100 ms; one that runs out stops host writes until the
host next sends bytes, so radio service, beats and wakes never wait on a host.
The loop answers the shared probes (`status`, `sync`, `gnss`, `timebase`,
`ui`, `region`, `channel`, `name`, `air`), `heap` with the T114's fields and
the failed allocations since boot (`heap=<used>/<size> highwater=<peak>
free=<free> failed=<count>`), and the node channel's own
`node`, `face` and `replay` lines. The V4 node does not echo completed inbound
resources back on their link (the T114 does): a peer that opens a link cannot
spend a wall relay's airtime that way. It also refuses every resource
addressed to it at the offer, before any reassembly (inbound limit 0; the T114
keeps Retinue's default). On a claimed board the node loop answers
the unsigned control status request and serves signed commands with the modem
loop's carrier and quiet window; an unconfirmed provisional change rolls back
at its deadline with no host frame.

## On-device status face

`name` reports the public device label. `name set SB1` saves up to 16 printable
ASCII characters and reboots; `name clear` clears the label. Leading/trailing
spaces are refused. The boot and default status titles show the saved name,
and USB `status` includes it. Setting the same name again does not write flash
or reboot. The name is separate from relay identity and RF profile; older
settings upgrade without changing the identity, region or channel.

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

The `resident-protocols` image, its contact store and the `memory-bench`
`protocol memory` command were withdrawn on 2026-11-14: foreign LoRa meshes are
reached through a stock companion radio, which holds the keys. The [October 16 physical receipt](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/receipts/protocol-parity-memory/physical/README.md)
records the exact images, identified boards, failures and application/state
restoration. The heap campaign passes on COM6; COM7's post-flash USB failure is
separate. The tight main-stack sample is not accepted worst-case headroom.
