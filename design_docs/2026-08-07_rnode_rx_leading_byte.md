# RNode receive path: a spurious leading byte, and repeated delivery

**Date:** 2026-08-07. **Status:** characterised on hardware, not yet fixed.

## What was seen

An iPhone running RetiChat, paired over BLE to a Heltec T114 running stock RNode
1.86, announcing on the trunk profile (906.875 MHz, BW 250 kHz, SF8, CR 4/5).
A Heltec V4 running Merely firmware in its `rnode` personality, driven by a
minimal host that speaks the RNode protocol and prints every device-to-host
frame (`scratchpad/rnode_host.py`).

The board accepted every parameter and answered `RADIO_STATE -> ON`, so the
profile was applied and the receiver armed. Then, in 90 seconds:

```
STAT_RSSI  -6 dBm
STAT_SNR   raw=44
DATA  len=211
      hex=902100a4098f0c73c7616756a34e1760913a0900cda1d002c8e495...
DATA  len=211
      hex=c02100a4098f0c73c7616756a34e1760913a0900cda1d002c8e495...
DATA  len=211
      hex=002100a4098f0c73c7616756a34e1760913a0900cda1d002c8e495...
```

`a4098f0c73c7616756a34e1760913a09` is the phone's LXMF address. The announces
are arriving, and the radio path is sound: RSSI −6 dBm, SNR 44–48.

> **Resolved 2026-08-07, later the same night: the defect is not ours.** Controls
> between two V4s, both running this firmware in the `rnode` personality, showed
> the receive path byte-exact: a 21-byte payload, a 21-byte payload containing
> `0xC0` and `0xDB` (exercising KISS escaping), and a 210-byte payload matching
> the phone's own frame size, all arrived identical to what was sent, five times
> each. The spurious byte is in what RetiChat transmits. Two hypotheses were
> raised and killed along the way: IFAC (there is no such setting in RetiChat,
> and `public.testboydcounty` is an LXMF channel, not a network name) and
> preamble length (changed 16 -> 8 with no effect on the shift).
>
> The repeated-delivery claim below is **withdrawn**: a later run received two
> frames in 100 seconds, and the board-to-board control delivered exactly one
> frame per transmission. The bursts were RetiChat announcing repeatedly.
>
> What survives is the method: build a host that speaks the protocol and prints
> raw frames, then vary one thing at a time against a control. Length was the
> variable my first controls missed.

## Two defects

**1. A spurious leading byte.** Every `DATA` frame carries one extra byte in
front of the packet. Drop it and the remainder is a textbook RNS announce:

| Field | Value | Reading |
|---|---|---|
| header | `0x21` | IFAC clear, header type 1, destination SINGLE, packet type `01` = ANNOUNCE |
| hops | `0x00` | direct, not relayed |
| destination | `a4098f…3a09` | the phone |
| context | `0x00` | |

The extra byte varies per frame (`90`, `c0`, `00`, `20`, `10`, `50`, `f0`) and
always has a zero low nibble, so it is stale state rather than noise. With it
present, every packet is malformed and Reticulum discards all of them while
still counting the bytes. That is the whole mystery: hosts showed healthy `rx`
counters and an empty announce stream.

**2. Repeated delivery.** RetiChat announces at most once every five minutes,
yet 19 `DATA` frames arrived in 90 seconds, most byte-identical. The board is
re-delivering the same receive buffer.

Both point at one root cause: the receive buffer is neither consumed nor
cleared correctly between deliveries, so it re-fires with a stale byte ahead of
the payload.

## Why it was not caught earlier

The `rnode` channel had receipts against real RNS, but with **our own boards on
both ends of the air**. This is the first test with a *stock* RNode as the
transmitting peer. A defect in our receive path is invisible when the peer is
also ours and the same framing assumptions apply on both sides. The direct-PHY
lane carries its own framing, which may absorb a length that the raw RNode lane
cannot.

The lesson generalises: interop receipts need a foreign peer on the far side,
or they only prove self-consistency.

## Where to look

- `radio-hand`'s executive `receive` and how `Received::len` and the frame
  buffer relate (`channel.rs:280` slices `&frame[..received.len]`).
- Whether the SX126x read honours the reported RX buffer start pointer rather
  than assuming offset 0, in the vendored `lora-phy`.
- Whether the RX IRQ is cleared and the receiver re-armed once per frame.

Framing is **not** the suspect: `selvage::kiss::encode_pair_into` was read and
is correct, and `channel/rnode.rs` already emits the captured triplet in order
(`STAT_RSSI`, `STAT_SNR`, `DATA`) as separate frames, matching stock.

## Consequence for the 2026-08-11 demo

The phone beat (BLE pairing, RetiChat driving a stock RNode) is proven and does
not depend on this. What this blocks is a *stock client talking through Merely
firmware*. Fallback if the fix does not land in time: stock RNode firmware on a
V4 as well, so the phone-to-laptop path is stock-to-stock, with Merely firmware
carrying the two-board direct-PHY beat and the live channel switch.
