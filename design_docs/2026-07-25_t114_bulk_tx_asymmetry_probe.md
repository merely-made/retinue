# T114 bulk-TX asymmetry probe under direct PHY

**Date:** 2026-07-25

> **Followed up and closed 2026-07-26** by
> [`2026-07-26_rnode_bulk_frame_loss.md`](2026-07-26_rnode_bulk_frame_loss.md).
> The suspicion recorded below was right that the defect lives in the stock
> RNode lane, but wrong that it is about the T114: with all three radios on
> RNode 1.86, **both** board families lose frames as senders, and two
> independent witnesses proved the missing frames are never transmitted at
> all. Read that receipt for the real characterisation.

**Result:** the asymmetry does not reproduce under Tulle direct-PHY firmware.
The 2026-07-22 finding (repeated 243-byte frames from the T114 sometimes
stopped arriving) was observed under stock RNode firmware 1.86 at BW 125 kHz.
Under direct PHY the T114 sends bulk symmetrically with the V4 at both
bandwidths and both payload sizes tested. The suspect is now the stock RNode
firmware's transmit path, not the T114 hardware, antenna, or RF direction.

## Bench

- COM6: Heltec WiFi LoRa 32 v4, Tulle direct-PHY USB firmware (`303a:1001`)
- COM10: Heltec T114, Tulle direct-PHY application (`1915:521f`)
- 906.875 MHz, SF8, CR 4/5, 17 dBm, sync `0x12`, preamble 16, explicit
  header, CRC on
- Retinue link MTU 255, resource request window 1
- Same desk; ports re-verified by USB VID before the runs

## Runs

```text
cargo run --features tulle-radio --example direct_phy_resource -- COM6 COM10 <len> [bw_khz] [timeout_s]
```

| Payload | BW | Publish (v4 sends bulk) | Fetch (T114 sends bulk) |
|---|---|---|---|
| 4096 | 250 kHz | passed | passed |
| 16384 | 250 kHz | passed | passed |
| 4096 | 125 kHz | passed | passed |
| 16384 | 125 kHz | timed out at the example's old 180 s deadline | not reached |
| 16384 | 125 kHz, 600 s deadline | passed in 221.0 s | passed in 219.6 s |

The single failure was a deadline artifact, not a stall: 16 KiB at BW 125,
window 1, MTU 255 simply needs ~220 s of strict half-duplex round trips. The
example gained optional bandwidth and transfer-timeout arguments and now
prints elapsed time per direction, so future runs report throughput instead
of hiding it.

## What this narrows

Three hypotheses existed for the 07-22 asymmetry: T114 hardware/RF, the
radio parameters, or stock RNode firmware. Direct PHY at the same SF and a
harsher bandwidth, moving 4x the receipt's payload through the T114's
transmitter, shows no asymmetry. Hardware and parameters are now unlikely;
the stock RNode 1.86 transmit path (or its serial/queue handling under
sustained load) is the remaining suspect.

Consequence for the product path: the direct-PHY lane, which is what Merely
firmware ships, does not carry the defect. The asymmetry matters only to the
stock-RNode compatibility lane. A targeted retest under RNode firmware
(repeated long frames, T114 as sender, both bandwidths) would close it fully;
that needs the T114 reflashed to RNode and is queued for a bench session, not
urgent.

## What this does not prove

Range, loss recovery, multi-hop, or anything about stock RNode behavior
itself. Throughput here (~74 B/s at BW 125) is the window-1 convergence
posture for strict half-duplex, not a capability claim.
