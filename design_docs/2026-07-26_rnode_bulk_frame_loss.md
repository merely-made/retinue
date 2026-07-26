# Stock RNode silently drops long frames: the 07-22 asymmetry, closed

**Date:** 2026-07-26

**Result:** the 2026-07-22 "T114 bulk TX is inconsistent" finding is real,
reproducible, and **not T114-specific**. With all three radios on stock RNode
1.86, a sender silently fails to put a large fraction of host-submitted long
frames on the air. Two independent witnesses prove the frames were never
transmitted. The host wrote every one of them and received no error.

## Bench

All three radios were flashed to stock RNode 1.86 for this session and
restored afterwards (see *Restore*, below).

- COM5: Heltec T114, RNode 1.86, `c2:c7:3c`
- COM6, COM7: Heltec WiFi LoRa 32 v4, RNode 1.86, `c3:c8:3f` (PA variant,
  provisioned 850-950 MHz)
- 915.000 MHz, SF8, CR 4/5, explicit header, CRC on, preamble 8, 7 dBm
- Same desk. RSSI -31 to -48 dBm, SNR 12-14 dB: a strong, clean channel.

## What was measured

`cargo run --features serial-async --example rnode_bulk_probe -- <sender> <receiver> [count] [frame_len] [bw_khz] [pace_ms] [dbm] [crc]`

| Run | Result |
|---|---|
| T114 -> v4, 200 x 243 B, BW 125 | 126/200 (37% lost) |
| v4 -> T114, 200 x 243 B, BW 125 | 172/200 (14% lost) |
| T114 -> v4, 60 x 50 B | 53/60 (12% lost) |
| pace airtime+400 ms / +800 ms / +2000 ms | 40% / 43% / 37% lost |
| TX power 0 dBm instead of 7 dBm | 40% lost |
| multi-threaded host runtime | 50% lost |
| single-frame round trip, both directions | always passes |

Four things the loss is **not** sensitive to: inter-frame pacing (tested from
180 ms to 2 s of added gap), transmit power, the host runtime flavour, and
direction (both directions lose, the T114 worse). One thing it is strongly
sensitive to: **frame length.** 12% at 50 B and 40% at 243 B both fit a
constant ~0.2% per-byte failure probability.

Every frame that does arrive is byte-exact. With corruption accounting on,
one run delivered 33 frames, 33 intact, 0 corrupted. So this is not a channel
that damages frames.

## The decisive test

`rnode_witness_probe` floods from one radio while two others listen
independently. If both witnesses miss the same sequence numbers, the sender
never transmitted them; if each misses a different set, the frames went out
and the loss is downstream.

```text
online: COM5 sends, COM6 and COM7 witness; 60 frames of 243 B at BW 125 kHz
heard by both: 38; only COM6: 0; only COM7: 0; neither: 22
VERDICT: the witnesses agree exactly. The missing frames were never transmitted.
```

Zero disagreements across 60 frames. The loss is entirely sender-side.

`RNodeSerialLink::send` resolves only once the frame has passed the airtime
gate **and its serial bytes have been written to the device**, and it returns
a typed error if anything refuses it. Every one of the 60 sends returned
`Ok`. So the host handed over every frame and was told nothing was wrong.

## What is not yet known

Which layer discards them. Two candidates, and this bench cannot separate
them without reading firmware source that the provenance rules bar:

1. **RNode firmware drops the frame** (queue, long-packet path, or radio
   state) without signalling the host.
2. **The device's serial intake overruns** and never sees the bytes the host
   wrote, which the host cannot detect either.

There is also a real possibility that this is **our** defect: if the RNode
host protocol carries a readiness or flow-control signal that `tulle` does
not honour, then pushing frames at it is our error, not the firmware's. The
first place to look is a gap already in the tree: `rnode.rs` decodes the
device's `ERROR` (0x90) frames into `last_error`, but `RNodeSerialLink` never
exposes them, so if the device has been complaining all along, nothing in the
host would show it. **Surfacing `last_error()` through the serial link is the
next diagnostic**, and it is worth doing regardless of what it reveals.

A related observation, unexplained: twice during the session the link went
completely silent (a smoke frame that should cross in under a second did not
arrive within 15 s), then recovered fully a minute later. That matches the
original 07-22 wording, "sometimes stopped arriving".

## Consequences

1. **The 07-22 receipt's framing is superseded.** It reads as a T114
   hardware or board quirk. It is neither: both board families lose frames as
   senders, and the T114 is merely worse. Any conclusion drawn from "the v4
   is the reliable bulk sender" should be re-examined.
2. **The direct-PHY lane is unaffected and is the healthy one.** The same
   three radios on Tulle direct-PHY firmware moved 16 KiB byte-exact in both
   directions the day before (`2026-07-25_t114_bulk_tx_asymmetry_probe.md`),
   including with the T114 as the bulk sender. Whatever this is, our own
   firmware does not have it.
3. **It weakens stock RNode as a product bearer for bulk traffic.** v1 sells
   stock certified hardware running stock RNode. Announces and short frames
   are fine; Reticulum-MTU bulk transfer over that lane loses a third of its
   frames with no host-visible error, and recovers only because Retinue's
   Resource layer retransmits. That is a real argument for the direct-PHY
   firmware direction, and a caveat to record before anyone promises
   throughput on stock radios.

## Restore

All three radios were returned to their prior firmware and verified:
both v4s reflashed with `espflash` from the workspace build, the T114 with
`adafruit-nrfutil` and `firmware/t114-phy/tulle-t114-phy-v10.zip`, then the
4 KiB direct-PHY Resource acceptance re-run byte-exact in both directions.

Two operational facts worth keeping:

- **The 1200-baud touch works on the T114 under RNode firmware.** Opening its
  port at 1200 baud with DTR low drops it into the Adafruit serial bootloader
  (`239A:0071`); no physical reset press is needed. This confirms the claim
  left unverified in `2026-07-25_rnode_direct_phy_rf_opacity.md`.
- **`rnodeconf -a` is drivable non-interactively** by piping its answers, and
  needs `--fw-version` alongside `--nocheck` or it refuses to resolve a
  cached image.
