# sennet

An independent, permissively licensed mesh radio protocol implementation in
the [retinue](https://github.com/merely-made/retinue) family, targeting
interoperability with existing LoRa messaging meshes, on the shared
[tulle](https://github.com/merely-made/retinue) radio layer.

Sennet is an independent implementation, developed from wire observation and
public documentation. It is not affiliated with or endorsed by any existing
mesh project.

A sennet is a ceremonial fanfare for a procession.

**Status:** the client serial deframer, schema-free protobuf reader, direct
capture fixtures, LoRa transport header/AES-128/256-CTR layer, application
envelope, port-1 UTF-8 text path, node-number/name reader, caller-persisted
source/packet-ID allocator, and bounded managed-flood relay core are
implemented. The node-info reader maps the observed numeric lookup key to user
ID, long name, and short name. Its field assignments are regression-tested
against two one-variable changes captured from stock firmware. `NodeDirectory`
retains those records and resolves a received packet into source, destination,
and text without hiding unknown endpoints. The relay filters one configured
channel, deduplicates `(source, packet_id)`, preserves ciphertext and nonce
identity, and returns a configurable delay window to its caller. A Sennet text
packet built through that API was transmitted by Tulle direct-PHY firmware on
COM6, accepted and rebroadcast by a stock node on COM7, and returned through
COM7's client API. The exact RF packet and client receipt are regression
fixtures. The direct-PHY implementation has also passed encrypted text in both
directions between a Heltec v4 on COM6 and the independent T114 firmware on
COM10.

With the `hardware` feature, `direct_phy_text` drives that same path through
Tulle's reusable Rust serial link. It advances and flushes a versioned packet-ID
state file before transmitting. The protocol layer constructs and opens
packets; Tulle owns USB framing, pacing, and radio metrics.

## Embedded allocation limits

The default protocol core is `no_std + alloc`. Its errors implement
`core::error::Error`, which keeps host error conversion available without
changing embedded dependencies. USB examples require `hardware`.

Sennet refuses inputs before retaining unbounded state. A transport payload is
at most 237 bytes and an application payload is at most 232 bytes, leaving the
worst-case envelope overhead. `encode_text`, `ApplicationEnvelope::encode`,
and stream `encode` return explicit errors when those limits do not fit.

`NodeDirectory::with_config(NodeDirectoryConfig)` bounds retained records and
the ID, long-name, and short-name bytes copied for each new record. Defaults
are 32 records with 32, 64, and 16 byte fields. It returns
`NodeInfoError::FieldTooLong` before copying a field and
`NodeInfoError::DirectoryFull` for an unseen record beyond capacity. Its owned
string byte budget is at most
`capacity * (id_limit + long_name_limit + short_name_limit)`; map allocation
metadata is allocator-dependent.

`ManagedFloodConfig::seen_capacity` retains at most that many `(source,
packet_id)` pairs. The logical identity storage is two eight-byte copies per
entry, one ordered eviction queue and one lookup set: `16 * seen_capacity`
bytes plus collection metadata. A relayed packet temporarily owns at most a
237-byte payload and a 253-byte encoded frame.

`DeframerConfig` bounds retained partial input at `4 + max_payload` bytes.
Its defaults are 516 bytes and four completed payloads per `push`; output is at
most `4 * 512` payload bytes per call, owned by the caller's output vector.
When that quota fills, `StreamError::OutputFull { consumed, .. }` reports the
retry offset while retaining the next complete frame. Drain the output and call
`push` again with `bytes[consumed..]`; valid coalesced frames are not dropped.

## Retained text-leaf instance

`instance::SennetInstance` composes one channel, packet-ID allocator, managed
flood duplicate history, and node directory into retained state. Its initial
mode is text leaf: it recognizes duplicates but never creates relay work or
claims acknowledgement behavior. A board owns activation tags, radio I/O, and
the physical-output queue.

The constructor accepts a caller-durable `PacketIdLease` with an exclusive
`[start, end)` source interval and a restored `next` ID. It cannot reset or
derive source IDs. `PacketIdReservation` extends only the contiguous end of
that interval and requires `ReservationProof::durable_ack()` after a verified
durable write, or the explicitly named `trusted_caller()` boundary. The proof
does not perform persistence or authenticate a caller.

There is one bounded pending outbound text. `take_outbound` transfers its frame
and expiry deadline to the board queue with an operation ID; completion, expiry,
and explicit loss retain that packet identity in the event. `discard_pending`
requires an explicit `LossPermission` and cannot revoke an in-flight board
operation: the board must settle or fence that physical work. `assess_pause(now, return_by)` returns
`Ready`, `Busy { retry_at }`, or `RequiresLoss { pending }` without mutating
state. Queue, output, receive, and directory ingestion refuse while paused.

`direct_phy_pair` is the two-independent-radio headed receipt:

```text
cargo run --features hardware --example direct_phy_pair -- COM6 COM10 LEFT_STATE RIGHT_STATE
```

Reconstruction follows controlled radio-bench experiments, with raw captures
and the scope of each claim recorded in [`PROVENANCE.md`](PROVENANCE.md).
Unexplored fields remain numbered rather than acquiring speculative names.

The source crates are MPL-2.0, like the rest of the retinue family: file-level
copyleft, which keeps changes to these files open while letting differently
licensed code link them. Downstream combined firmware may be distributed under
GPLv3 with its corresponding source and required notices; GPL-derived
implementation code does not enter this crate graph.

## License

Licensed under the Mozilla Public License, Version 2.0 ([LICENSE](LICENSE)).

This is a deliberate choice. Sennet is an independent implementation, built
clean-room (see [PROVENANCE.md](PROVENANCE.md)) — it contains no third-party
protocol code and needs no one's permission to exist. But a *permissive*
independent implementation would be an easy route around the copyleft the
protocol's own authors chose, letting anyone take the work without giving
improvements back. MPL-2.0 keeps that door shut: build whatever you like on top,
under any license, but improvements to *this implementation* stay published.

MPL is GPL-compatible, so it also combines into the GPLv3 firmware images this
project ships.
