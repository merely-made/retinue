# RNode and direct PHY cannot hear each other: RF opacity finding

**Date:** 2026-07-25

**Result:** stock RNode 1.86 and Tulle direct-PHY firmware do not cross RF in
either direction at matched nominal parameters. The planned closure of the
07-22 T114 bulk-TX asymmetry (T114 reflashed to RNode, direct-PHY V4 as the
listener) is blocked on this; it needs a second RNode, which means reflashing
a V4.

## Bench

- COM5: Heltec T114, reflashed tonight to stock RNode 1.86 over the tulle
  app's `bootloader` command + serial DFU. EEPROM provisioning from 07-21
  survived the interlude intact (signature validated, host-controlled mode).
- COM6: Heltec WiFi LoRa 32 v4, Tulle direct-PHY USB firmware.
- Matched on both sides: 915.000 MHz, BW 125 kHz, SF8, CR 4/5, explicit
  header, CRC on, preamble 8, 7 dBm.

## What was swept

The new `rnode_bulk_probe` example (tulle, `serial-async`) sends a smoke frame
before its flood and refuses to proceed if it does not cross. Swept:

- forward (RNode transmits, direct-PHY listens): sync `0x12`, `0x34`, `0x14`,
  `0x24`, `0x2b`, `0x44`, `0xf4`; then `0x12` and `0x34` with inverted IQ.
- reverse (direct-PHY transmits, RNode listens): sync `0x12`, `0x34`, `0x14`,
  `0x24`.

Every combination failed. The reverse direction failing too says this is not
the T114 transmit stall recurring: the two firmwares genuinely do not share a
demodulation configuration.

## Reading

The direct-PHY profile encodes a one-byte sync word as SX126x registers
`((n>>4)<<4|4, (n&0xf)<<4|4)`, which covers the standard private (`0x12` →
`0x1424`) and public (`0x34` → `0x3444`) values and Meshtastic's (`0x2b` →
`0x24b4`, proven against stock nodes). RNode's radio protocol exposes
frequency, bandwidth, SF, CR, and power, and no sync-word control (recorded
2026-07-22), so whatever RNode programs is invisible from the host side.
Candidates for the mismatch: sync register values outside the nibble-encoding
space, an LDRO policy difference, or another fixed register choice. The GPL
firmware source is not readable under the provenance rules, so the answer has
to come from a black-box sweep with a wider receiver knob (arbitrary two-byte
sync + LDRO override in the direct-PHY firmware) or from an SDR capture.
Neither is tonight's work.

## Consequences

1. **The stock-RNode lane and the direct-PHY lane are separate collision
   domains in practice.** Tulle's airtime gate still arbitrates each node's
   own transmissions, but an RNode-based node and a direct-PHY node cannot
   carry one Reticulum network over the same channel today. The v1 product
   posture (stock RNode + host retinue) is unaffected; mixed fleets are.
2. **The 07-22 asymmetry retest needs two RNodes.** The T114 is already
   flashed and provisioned; the probe harness is written and committed. The
   remaining step is reflashing one V4 to RNode 1.86 for a session, running
   `rnode_bulk_probe` at BW 125 and 250, then restoring the V4.
3. **Restoring the T114 to direct-PHY** when wanted: the RNode nRF52 build
   sits on the Adafruit core, so a 1200-baud serial touch should re-enter the
   bootloader without a reset press; then serial DFU with
   `firmware/t114-phy/tulle-t114-phy-v10.zip`. Unverified claim; if the touch
   does not work, it is one physical double-tap of the reset button.

## Probe instrument

`cargo run --features serial-async -p tulle --example rnode_bulk_probe -- <rnode_port> <phy_port> [count] [frame_len] [bw_khz] [sync_hex] [invert 0|1] [rev]`

Smoke-gated flood with sequence tags, inter-arrival stall detection (20 s
silence), loss ranges, and weakest-RSSI reporting. The `rev` mode sends one
frame the other way to separate "sender is silent" from "receiver is deaf".
