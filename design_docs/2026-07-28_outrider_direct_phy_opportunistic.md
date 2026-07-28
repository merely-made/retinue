# Outrider opportunistic delivery over Tulle direct PHY

**Date:** 2026-07-28

**Result:** passed

## Bench

- COM6: Heltec WiFi LoRa 32 V4 running Tulle direct-PHY firmware
- COM10: Heltec T114 running Tulle direct-PHY firmware
- 906.875 MHz, BW 250 kHz, SF8, CR 4/5, 17 dBm
- sync word `0x12`, preamble 16, explicit header, CRC enabled
- cost-8 LXMF delivery stamps

## Command

```text
cargo run -p outrider --example direct_phy_opportunistic --features tulle-radio -- COM6 COM10 250 60
```

## Receipt

```text
radios online: COM6=left, COM10=right
discovery: ratcheted cost-8 lxmf.delivery announces crossed RF
carrier admission: refused 291-byte packet before the 255-byte RF queue
left to right: 150 signed LXMF bytes, 243-byte ratcheted RF packet, cost-8 stamp passed
right to left: 151 signed LXMF bytes, 243-byte ratcheted RF packet, cost-8 stamp passed
OUTRIDER DIRECT-PHY OPPORTUNISTIC HEADED PASSED
```

Each receiver reconstructed the destination-elided LXMF object, resolved the
source from its validated announce, and verified the source identity,
signature, message id, title, content, retained ratchet id, and cost-8 stamp.
Neither direction opened a link or Resource session.

## Carrier boundary closed

Tulle direct PHY carries at most 255 bytes in one RF frame. Retinue's ordinary
protocol MTU is 500 bytes, so `ENCRYPTED_MDU` alone is not a sufficient
admission check for a link-less packet on this interface.

The first headed candidate was a valid 205-byte signed LXMF object. Removing
its 16-byte destination left 189 plaintext bytes; PKCS#7 padding and the
ratcheted token expanded the complete Reticulum packet to 291 bytes. The
original run queued it and the direct-PHY driver refused it.

Retinue interfaces now carry an explicit complete-frame limit. Tulle installs
the radio's limit synchronously when its driver future is constructed, and all
endpoint queues apply it after any transport header is added. `send_single`
returns `InvalidInput` if no candidate interface can carry the encrypted
frame, instead of issuing a queue receipt.

The headed rerun refused the same 291-byte packet before the RF queue, kept
both drivers alive, then delivered compact 243-byte messages in both
directions. Per-interface admission is complete. Link-MTU negotiation remains
a separate caller policy.
