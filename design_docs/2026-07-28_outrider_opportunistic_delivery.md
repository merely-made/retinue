# Outrider opportunistic delivery acceptance

**Date:** 2026-07-28  
**Stock baseline:** LXMF 0.9.6 / RNS 1.4.2  
**Result:** Retinue R9 and un-stamped Outrider opportunistic delivery pass in
both stock directions. Cost-8 delivery also passes in both directions over the
Tulle direct-PHY board pair.

## Boundary captured

RNS ratcheted single packets use the ordinary single/Data header and token
layout. The ephemeral X25519 secret is combined with the destination's
advertised ratchet public key, while HKDF keeps the destination identity hash
as salt. The packet carries no ratchet id. Receivers try retained private
epochs until one authenticates.

LXMF's decrypted opportunistic plaintext is not the complete direct-delivery
object:

```text
source(16) || signature(64) || MessagePack payload
```

The missing destination is already present in the Reticulum packet header.
Prepending it reconstructs the ordinary signed LXMF object. Message-id and
signature rules are shared with direct delivery rather than translated into
a second codec.

Fixed captures:

- `crates/retinue/tests/fixtures/ratchet_packet.json`
- `crates/outrider/tests/fixtures/lxmf_0_9_6_opportunistic.json`

## Executable receipts

- Retinue decrypts the stock ratchet packet and reports the retained ratchet
  id.
- Stock RNS decrypts the deterministic Retinue packet with
  `enforce_ratchets=True`.
- Endpoint tests send through current and retained epochs, reject a missing
  advertised ratchet, and enforce `ENCRYPTED_MDU`.
- Outrider rebuilds the fixed opportunistic plaintext byte-exactly, verifies
  a cost-8 stamped in-process delivery, and crosses a Retinue transport node.
- `oracle/interop_opportunistic_send.py`: stock to Outrider passed, including
  title, content, message id, source signature, and retained ratchet id.
- `oracle/interop_opportunistic_receive.py`: Outrider to stock passed; stock's
  delivery callback agreed on title, content, and message id.

## Ownership

`RatchetStore` is sans-I/O state. The host supplies time, entropy, and durable
storage, then passes the current snapshot into the endpoint. Rotation defaults
match stock: 512 retained epochs, 30-minute rotation, 30-day expiry.
Snapshots contain private keys and must be protected like identity material.

## Remaining evidence

The live stock exchanges above were un-stamped. Cost-8 generation and
enforcement pass in Outrider's executable test, but a stock cost-8
opportunistic sender did not emit during its bounded oracle run. That exact
combination remains open.

Cost-8 opportunistic delivery passed in both directions between the Heltec V4
and T114 over Tulle direct PHY. Both signed LXMF objects fit one 243-byte
ratcheted RF packet. Receipt:
`2026-07-28_outrider_direct_phy_opportunistic.md`.

That run also reproduced the interface-MTU gap: Retinue admits link-less
packets against its 500-byte protocol MTU, while direct PHY carries at most
255 bytes. A larger valid opportunistic object was initially accepted into the
endpoint queue and then refused by the radio driver. Per-interface frame-limit
admission now closes that gap: the same 291-byte packet is refused before the
queue, both drivers remain live, and fitting packets still cross.
