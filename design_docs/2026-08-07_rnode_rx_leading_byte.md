# A leading byte from a stock peer, and the end of the RF opacity question

**Date:** 2026-08-07. **Status:** diagnosed. Our side is clear; the defect is in
the peer. One prior finding is retired as an artefact.

## The bench

An iPhone running RetiChat, paired over BLE to a Heltec T114 running stock RNode
1.86, announcing on the trunk profile: 906.875 MHz, BW 250 kHz, SF8, CR 4/5.
A Heltec V4 running Merely firmware in its `rnode` personality, driven by a
minimal host that speaks the RNode protocol and prints every device-to-host
frame verbatim (`testing/rnode_host.py`).

## What arrives

The board accepts every parameter and answers `RADIO_STATE -> ON`, so the
profile applied and the receiver armed. Then:

```
STAT_RSSI  -6 dBm
STAT_SNR   raw=44
DATA  len=211
      hex=902100a4098f0c73c7616756a34e1760913a0900cda1d002c8e495...
DATA  len=211
      hex=c02100a4098f0c73c7616756a34e1760913a0900cda1d002c8e495...
```

`a4098f0c73c7616756a34e1760913a09` is the phone's LXMF address. Every frame
carries **one spurious byte** before the packet. Drop it and the remainder is a
textbook RNS announce:

| Field | Value | Reading |
|---|---|---|
| header | `0x21` | IFAC clear, header type 1, destination SINGLE, packet type `01` = ANNOUNCE |
| hops | `0x00` | direct, not relayed |
| destination | `a4098f…3a09` | the phone |
| context | `0x00` | |

The byte varies per frame (`90`, `c0`, `00`, `20`, `10`, `50`, `f0`, `e0`) with
a zero low nibble. With it present every packet is malformed, so Reticulum
discards all of them **while still counting the bytes** — which is why every
host showed a healthy `rx` counter and an empty announce stream, and why this
took a night to find.

## Our receive path is not the cause

Three controls, two V4s both running this firmware in the `rnode` personality,
one variable changed at a time. Sent versus received, five transmissions each:

| Payload | Result |
|---|---|
| 21 bytes | byte-exact |
| 21 bytes containing `0xC0` and `0xDB` | byte-exact (KISS escaping exercised) |
| 210 bytes, matching the phone's frame size | byte-exact |

Length was the variable the first control missed, and the only one that could
plausibly have hidden a fault at 210 bytes.

**It is also not a demodulation offset**, which was the attractive theory: at
SF8 a symbol is one byte, so a one-symbol lock error shifts a frame by exactly
one byte and leaves a preamble fragment in front, which is what the bytes look
like. Ruled out twice over. The vendored driver has rejected `PayloadCrcError`
since the 2026-08-05 fix and these frames pass, and a CRC covers the payload, so
a shifted demodulation would fail it. Separately, changing our preamble from 16
symbols to 8 to match stock left the shift unchanged.

Two other hypotheses died on the way, recorded so nobody re-derives them:

- **IFAC.** RetiChat has no network-name or passphrase setting, and
  `public.testboydcounty` is an LXMF *channel* (a conversation), not an
  interface network name. Setting one on our side actively broke the link.
- **Repeated delivery.** 19 near-identical frames in 90 seconds looked like a
  re-delivery bug, but board-to-board delivered exactly one frame per
  transmission, and a later run saw two frames in 100 seconds. The bursts were
  the peer announcing repeatedly.

## What it leaves: the peer prepends a byte

The frames pass CRC, so the bytes are exactly what was transmitted. Something
between RetiChat's LXMF stack and the air puts one byte in front of every
packet. A packet malformed that way is undeliverable to **any** Reticulum node,
which is consistent with the app's reviews (3.2 stars; users reporting messages
that never arrive). Worth reporting upstream to New Endian with this capture.

## What it incidentally proves: stock RNode and this firmware do cross

`2026-07-25_rnode_direct_phy_rf_opacity.md` concluded the two firmwares "do not
cross RF", after sweeping seven sync words and inverted IQ in both directions.
**They cross.** Valid announces at −6 dBm, repeatedly, across several sessions.

That sweep predates the SX126x CRC fix, when the driver handed CRC-failed
packets up as good frames. Its probe decides "crossed" by whether a known smoke
frame arrives *intact*, which answers no in that world whether or not the radios
hear each other. The sweep could not distinguish opacity from corruption, and
"never crossed" is exactly what the CRC bug produces. **Anything else concluded
from `rnode_bulk_probe` before 2026-08-05 deserves the same suspicion.**

## Method, which is the transferable part

Every wrong turn tonight came from reasoning about counters; the answer came
from a host that speaks the protocol and prints raw frames, plus controls
varying one thing at a time. Two habits are worth keeping:

- **Interop receipts need a foreign peer.** The `rnode` channel had receipts
  against real RNS, but with our own boards on both ends of the air, where any
  shared assumption cancels. That is self-consistency, not interoperability.
- **A green counter is not a working path.** `rx` counted bytes for hours while
  nothing decoded. Instrument the layer where the decision is made.

## Consequence for the 2026-08-11 demo

The phone beat — BLE pairing, RetiChat driving a stock RNode — is proven and
independent of this. What is not available is *RetiChat as a messaging endpoint
over LoRa*, which was never ours to fix. The messaging beat belongs to our own
stack, board to board, which is proven.
