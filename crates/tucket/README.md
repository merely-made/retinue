# tucket

MeshCore interop for the [retinue](https://github.com/merely-made/retinue) radio
family: node management and text interop with MeshCore mesh networks, on the
shared [tulle](https://github.com/merely-made/retinue) radio layer.

A tucket is a trumpet flourish announcing a single arrival.

## Embedded core and capacities

The default library is `#![no_std]` with `alloc`. The `hardware` feature is
only for the headed serial/Tulle example and does not change the core protocol
API. It is the board owner's job to provide an allocator and choose concrete
capacities from its target memory budget.

`Node::with_capacity` takes a `NodeCapacity { contacts, dedup }`. Both tables
allocate once at construction and do not grow while a frame is handled. A new
advert when the contact table is full is refused by `try_add_contact` with
`CapacityError::ContactsFull`; it never evicts an unrelated contact. Dedup
uses exactly `dedup * 8` bytes for packet hashes. Contact backing storage is
at most `contacts * size_of::<(u8, Contact)>()`, plus at most 64 path bytes for
each learned direct route and allocator bookkeeping. `Contact` is deliberately
private, so firmware should measure the final target image rather than relying
on a host ABI size.

Pending texts remain caller-owned, because the caller also owns retry timing,
TX completion, and departure assessment. `PendingTexts::new(n)` is a small
bounded collection if one is useful: it limits only that caller's outstanding
sends, never the number of installed Tucket instances. One `PendingText` has a
text body of at most `MAX_TEXT_BYTES` (171 UTF-8 bytes) and stores at most four
ACK values, matching the four attempts the current wire control field supports.
`try_begin_text` accepts a borrowed `str`, validates it before copying, and
returns `TextTooLong`, `InvalidRetryPolicy`, or `UnknownContact`; `PendingTexts::push`
returns `PendingFull` rather than growing. The compatibility `begin_text`
wrapper still returns `Option`. `TextMessage::try_encode`,
`GroupText::try_encode`, and `try_encrypt_then_mac` provide the same
pre-allocation refusal at their lower-level boundaries; compatibility `encode`
helpers reject an oversize input before allocating.
`TextMessage::try_plain`, `GroupText::try_new`, `AdvertData::try_chat`, and
`Node::try_advert_frame` are the checked borrowed-input constructors for those
same wire fields.

At the radio boundary, `PacketRef::decode` validates a raw frame without an
allocation and `PacketRef::encode_into` writes into caller storage without a
partial result. `Node::on_frame` uses that borrowed parse before it materializes
an owned packet. One accepted input can create at most one app event and two
outbound frames. The conservative working-set formula for a single text is:

```text
resident = dedup * 8 + contact backing + learned-route bytes + pending collection
text working bytes <= plaintext(5 + 171) + encrypted blob(2 + 176)
frame bytes <= header/transport/path(70) + payload(184) = 254
```

The formula excludes allocator metadata, stack frames, crypto implementation
scratch space, and board radio buffers. Measure those on the selected firmware
target before advertising an installed Tucket capability.

**Status:** authenticated adverts, flood text and acknowledgements, forwarding,
and reciprocal direct-path learning are implemented. A successful flooded
exchange teaches both endpoints a route; later text and acknowledgements select
the learned hop path. Private text sends now expose a caller-timed retry state:
attempt numbers are authenticated, delayed acknowledgements remain valid, and
the default fourth transmission clears a failed direct path and floods to learn
a replacement. Attempt count and flood fallback are settings. The in-memory
three-node acceptance covers discovery, reply, direct delivery, and fallback
through a repeater.

The headed acceptance passes against the official MeshCore companion v1.15.0
firmware on a Heltec WiFi LoRa 32 v4. Current structured chat adverts were
imported and exchanged over RF; a stock-origin flood established reciprocal
paths, then Tucket and stock MeshCore each selected that direct route for text
and received the other's acknowledgement.

A second headed acceptance passes through an official MeshCore repeater v1.16.0
on a Heltec T114. Tucket and the stock companion were each given the repeater's
one-hop source route. Encrypted text and acknowledgements crossed the named
relay in both directions. The hardware receipt is in
[`design_docs/2026-07-22_meshcore_relay_headed.md`](design_docs/2026-07-22_meshcore_relay_headed.md).

With the `hardware` feature, `meshcore_headed` configures an official MeshCore
companion through its serial API while Tucket uses Tulle direct-PHY on the
other radio. The acceptance requires authenticated adverts and encrypted text
in both directions, then checks that both implementations select the reciprocal
direct route and acknowledge it over RF.

An optional fourth argument is a repeater's one-byte hash in hexadecimal:

```text
cargo run --features hardware --example meshcore_headed -- COM6 COM8 915000000 ab
```

In that mode the harness installs the same one-hop source route on Tucket and
the stock companion. Each endpoint ignores the other's original RF
transmission because the repeater is the named next hop. The acceptance passes
only after encrypted text and its acknowledgement cross that repeater in both
directions.

## License

Licensed under the Mozilla Public License, Version 2.0 ([LICENSE](LICENSE)).

MPL-2.0 is file-level copyleft: you may use this crate in a larger work under
any license, including a proprietary one, but modifications to *these files*
must be published under the MPL. It is GPL-compatible, so it combines into the
GPLv3 firmware images this project ships.

Portions were ported from the upstream [MeshCore](https://github.com/ripplebiz/MeshCore)
project, which is MIT licensed. MIT permits relicensing a derivative work and
requires the original notice be retained; it is reproduced in [NOTICE](NOTICE)
and applies to those portions.
