# retinue

An endpoint-scoped Rust implementation of the
[Reticulum](https://reticulum.network/) protocol: identity, announces, links,
resources, request/response, and a reliable byte stream, built for embedding as
a library. Qualified against RNS 1.5.4 within the recorded local interoperability scope.

**Status: working, wire-verified, pre-1.0.** Not the reference implementation,
and not yet hardened for adversarial deployment (see *Maturity* below). The plan
and wire notes live in [`design_docs/`](design_docs/).

## What works

Embedded `node::Node` callers can use `new_with_payload_limits` to cap incoming
packet size, local announce data, link payloads, outbound resources and inbound
resource part counts. Oversized input is refused before Node allocation or state
changes. `try_set_app_data` preserves the previous value on refusal. Table sizes
and freshness history remain separately configurable. Bound raw bytes before
packet decoding and bound the caller's action queues too. The combined-capacity
fixture builds with compression disabled; compressed-resource expansion requires
its own budget. See the repository's
[`MC4 plan`](../../design_docs/2026-09-10_murmuration_controller_plan.md).

The layers below have recorded black-box RNS checks. Limited source review
began later; each historical receipt retains its own evidence boundary. The committed byte fixtures under [`tests/fixtures/`](tests/fixtures/)
retain their recorded producer versions, including the historical RNS 1.3.8 corpus.
Signed-artifact fixtures separately cover 1.5.2 and 1.5.4; see the
[six-case comparison receipt](../../testing/receipts/rns-1.5.4-signed-artifact/README.md).
The [live mixed-runtime receipt](../../testing/receipts/rns-1.5.4-repin/README.md)
qualifies the RNS 1.5.4 pin:

- **Wire vocabulary** — identities, hashes, destination naming, the packet
  codec, announces, identity and ratchet tokens, and caller-persisted receive
  ratchet rotation/retention. Sans-io: pure functions over bytes, replayable
  against fixtures.
- **Links** — the handshake (ephemeral ECDH + the mode/MTU trailer), the link
  id derivation, encrypted link data, keepalives, and the request/response and
  resource contexts.
- **Resources** — the advertisement, windowed segmented transfer, and the
  hash-map/proof derivations, plus endpoint-level publish/fetch sessions with
  retry and timeout policy.
- **Reliable streaming** — RNS `Channel`/`Buffer` framing with a dynamic send
  window, plus link-proof acknowledgement, wired into an `AsyncRead + AsyncWrite`
  stream. Opt-in over the best-effort stream (which is right for TCP), for lossy
  media. See [`src/reliable.rs`](src/reliable.rs).
- **An allocation-free floor** — `command`, `identity`, `hash`, and `capacity`
  build with the `alloc` feature off, so a core-only firmware image can verify
  a signed command without carrying a heap. Everything else needs `alloc`,
  which is on by default.
- **The endpoint runtime** — a tokio shell (behind the `tokio` feature, on by
  default) that attaches interfaces, routes inbound packets, opens and accepts
  links, and surfaces them as streams. Turn the feature off and the codec,
  framing, and reliability machinery still stand alone.
- **Interface access codes** — network-name/passphrase identity derivation,
  1–64-byte codes, outbound masking, and inbound verification at the carrier
  boundary. TCP, raw interfaces, routed egress, and Tulle share the same
  sans-I/O codec. A pinned RNS 1.5.4 gate passes in both directions.
- **Transport-node routing** — opt-in (`enable_routing`). The default posture is
  endpoint-scoped — a retinue accompanies a peer — but a node can forward
  announces and link traffic between its interfaces when asked to.
- **Link-less asymmetric packets** — outbound encryption selects the current
  ratchet from a validated announce; registered destinations receive against
  retained epochs, with explicit rotation and versioned caller-owned snapshots.
  Current and retained epochs pass endpoint tests, a transport hop, and stock
  RNS 1.5.4 in both crypto directions.

## Maturity

Honest about what is *not* done, so nobody deploys it expecting more:

- **Interfaces**: TCP, the raw interface seam, and an optional Tulle packet-radio
  pump are implemented. RNode serial and direct-PHY USB framing remain owned by
  Tulle. A headed endpoint exchange through two RNode 1.86 radios now covers a
  2 KiB reliable stream and a 4 KiB Resource byte-exactly. A second headed
  exchange through the Tulle direct-PHY pair covers 4 KiB Resource publish and
  fetch in both endpoint directions; see
  `design_docs/2026-07-23_direct_phy_resource_acceptance.md`. UDP is not implemented.
- **Radio MTU**: link MTU, reliable in-flight window, setup retry interval, and
  Resource request window are caller settings. Reliable chunks, Resource parts,
  advertisements, and hashmap updates derive their size from the selected link
  MTU. The open headed profile uses MTU 255; its eight-byte IFAC profile uses
  MTU 247. Both use one frame/part per half-duplex turn. Raw interface owners
  can set a complete-frame cap when attaching;
  Tulle installs its radio cap synchronously. Every outbound queue applies that
  cap after transport addressing, and link-less sends receive no queue receipt
  unless at least one selected interface can carry the encrypted frame.
  IFAC bytes count against the same cap; a caller advertising a link MTU over
  a fixed-size carrier must subtract its configured code length (for example,
  255 physical becomes 247 logical with the usual eight-byte code). That exact
  IFAC Resource boundary is covered by both an in-memory carrier regression
  and a headed Outrider propagation receipt.
  Automatic link-MTU negotiation is not implemented.
- **Routing**: route expiry, announce-rate budgeting, owned-destination path
  responses, and transport forwarding are implemented. Open-network hardening
  and announce-cache responses on behalf of other nodes remain outstanding.
- **Spec parity**: IFAC-protected interfaces and ratcheted single packets are
  implemented. Automatic ratchet clock/entropy/persistence policy remains with
  the host, which must rotate and save `RatchetStore`.
- **Reliable interop**: both link directions use the captured IDENTIFY exchange,
  including bounded retransmission under loss. The complete reliable and Resource
  exchange through the Tulle radio pump passed on 2026-07-22; see
  `design_docs/2026-07-22_tulle_headed_acceptance.md`.

The runtime has had a first hardening pass (OS-CSPRNG link entropy, link-setup
DoS and leak fixes, bounded network intake, cancellable teardown), but has not
been audited. Treat it as pre-1.0.

## Provenance

Implemented from the public-domain Reticulum protocol specification and manual,
and the MIT-licensed Beechat `reticulum` crate. The Python reference
implementation supplied the recorded black-box oracle results. Limited comparative
source review began September 26; through October 4 the owner reported no RNS
implementation code copied or translated. From October 5 the crate is under the
Reticulum License and RNS source may be adapted into it; adaptations are listed
in [NOTICE](NOTICE). Wire notes: `design_docs/2026-07-13_rns_wire_format_reference.md`.
Not affiliated with the Reticulum project.

One seam has a third input. `src/artifact.rs` and `src/msgpack.rs` implement the
RNS signed-artifact envelope, whose layout was read from
[Prns](https://github.com/KenAKAFrosty/Prns) (MIT OR Apache-2.0, MIT elected).
Prns is now withdrawn as a donor and trusted independent reference. Old captures
used donor-selected inputs; the current fixtures use project-selected inputs and
separate `rnid` 1.5.2/1.5.4 captures. Observed compatibility does not resolve
historical provenance. See [NOTICE](NOTICE) and
`design_docs/2026-08-10_prns_donor_ledger.md`.

## License

Licensed under the Reticulum License ([LICENSE](LICENSE)) from 2026-10-05.
Releases through 0.2.0 were published under MPL-2.0 and keep those terms.

The Reticulum License is the license of the reference implementation. It is
permissive (use, copy, modify, distribute, sell) with two conditions. The
software must not be used in any system that can purposefully harm people,
and it must not be used to create AI or machine-learning training data. Keep
the notice in copies. It applies to this crate wherever the crate is linked;
code around it keeps its own license. It is not OSI-approved and has no SPDX
identifier, so license gates need an explicit exception
(`LicenseRef-Reticulum`). Why: `design_docs/2026-10-05_reticulum_license_adoption.md`.

Contributions are accepted under the same terms.
