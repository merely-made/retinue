# Outrider direct delivery over Tulle direct PHY

**Date:** 2026-07-28

**Result:** passed

## Bench

- COM6: Heltec WiFi LoRa 32 V4 running Tulle direct-PHY firmware
- COM10: Heltec T114 running Tulle direct-PHY firmware
- 906.875 MHz, BW 250 kHz, SF8, CR 4/5, 17 dBm
- sync word `0x12`, preamble 16, explicit header, CRC enabled
- Retinue link MTU 255
- Resource request window 1
- cost-8 LXMF delivery stamps

## Command

```text
cargo run -p outrider --example direct_phy_delivery --features tulle-radio -- COM6 COM10 250 180
```

## Receipt

```text
radios online: COM6=left, COM10=right
discovery: cost-8 lxmf.delivery announces crossed RF
left to right small: 18 bytes via Data, authenticated, cost-8 stamp passed in 6.8s
right to left small: 18 bytes via Data, authenticated, cost-8 stamp passed in 9.6s
left to right large: 4096 bytes via Resource, authenticated, cost-8 stamp passed in 40.6s
right to left large: 4096 bytes via Resource, authenticated, cost-8 stamp passed in 43.7s
OUTRIDER DIRECT-PHY DELIVERY HEADED PASSED
```

Each receiver verified the source identity and signature, message id, title,
content, selected transport, and cost-8 stamp.

## Boundary found

The first large-message attempt expired because Outrider could not pass
carrier-specific timing into Retinue's automatic Data-or-Resource helper.
`Endpoint::send_payload_with_config` and Outrider's matching direct-delivery
configuration seams now carry the same explicit retry, timeout, and
half-duplex request-window policy already used by the lower-level Resource
bench acceptance.

Delivery announces are sequenced and independent sessions are spaced by two
seconds. Back-to-back announces collided on the shared half-duplex carrier,
and immediately reversing a completed session could start a link request
before the previous close cleared the radio queue. These are explicit bench
conditions rather than hidden retries.

This closes Outrider founding gate 7. It proves direct, one-hop LXMF delivery
through the native product radio stack. It does not claim range, routed
forwarding, loss recovery, stock-RNode RF interoperability, or propagation
over RF.
