# Repository layers: replication, peer, mesh

**Status, 2026-11-25: decided; no code has moved.** This replaces the
repository boundaries Murm, P2mere and Eidetic grew into with three layers.
It owns the target layout and the evidence behind it. The
[transport consolidation plan](2026-10-28_transport_consolidation_plan.md)
still owns the open work that moves family code onto Murm's endpoints.

**For:** Mark, 2026-11-25, asked which domains Murm could be split along:
"Here we are trying to redraw the lines to improve things. Not make the old
ones into canon." He also said redundancy "can be judged, not just a tag".
Repository names and current edges are the cost list, not the boundaries.

## Decisions

### repository_layers D1 — three repositories by layer

Question (2026-11-25): three layers (replication, peer, mesh), two (replication
inside peer), four (mesh split into Reticulum and devices), or redraw the code
inside the current repositories first? Answer: "Three layers (Recommended)".

| Layer | What it owns | Members | Rust lines |
| --- | --- | --- | --- |
| Replication | What a signed log is, how it is stored, processed, verified and reconciled. No carrier. | `p2mere-core`, `-store`, `-sync` (the protocol), `-stream`, `-encryption`, `-auth`, `-spaces`; `stickleback` from Eidetic | ~63k |
| Peer | Carriers, sessions, admission, release-log carriage, conversations | `mere-transport`; `p2mere-net` and `p2mere-discovery` as the iroh carrier and gossip; `murm-sync` as the one session layer; `murm-webrtc`, `murm-i2p`, `retained-repair`, `transport-conformance`; `murm-resolve`, `murm-holder`; `murm`, `mere-comms`, `murm-ocapn` | ~66k |
| Mesh | Reticulum, LXMF, radios, boards, device apps | `retinue`, `outrider`, `pipit`, `retinue-sim`, `truchman`, oracles, fuzz; `tulle`, `selvage`, `seneschal`, `radio-hand`, `radio-face`, `radio-mirror`, `postilion`, `station-custody`; firmware, vendor, `linkboy`, `signalman`, `signalman-desktop`, benches, `iroh-control` | ~200k, plus 36k vendored |

Edges between layers:

| From | To | Why |
| --- | --- | --- |
| Peer | Replication | Sessions run the log model. |
| Peer | Mesh | Optional: the Reticulum carrier, and `truchman` for I2P. |
| Mesh | Peer | Signalman's optional `comms` feature (`mere-comms`). |
| Ten other repositories | Replication | The data model. |

The umbrella `p2mere` crate and `p2mere-blobs` are not carried forward: no
family crate depends on them.

Links: area repositories · draws on transport consolidation (boundary) · plans the replication, peer and mesh cuts · summary three repositories by layer; old repository names are not boundaries

## Evidence, 2026-11-25

Read from `origin/main` across `Code/repos`.

- **Who uses P2mere.** `core` and `store`: distillery, eidetic, esp, gemot,
  graphshell, isocosm, knot-editor, mere, murm, turnstone. `sync`: gemot,
  isocosm, murm. `encryption`: eidetic, gemot, murm. `net`: murm, graphshell,
  isocosm, knot-editor. `spaces` and `auth`: gemot. `stream`: eidetic. The
  umbrella crate (16.3k lines) and `blobs`: none. Only `net` (15.8k lines) and
  the umbrella reach iroh.
- **Two session layers for one protocol.** `p2mere-net` drives log sync over
  iroh with its own gossip, discovery and address book. `murm-sync` wraps it
  (`JoinedSpace`) and also drives the same `TopicLogSync` over any admitted
  stream (`StreamSync`), catch-up only. Most P2mere patches to `net` exist so
  Murm can sit beside it: admitted external ALPN handlers, a store-agnostic
  address book, the shutdown drain, per-instance sync ids. Release-log D8
  ranks coherence across carriers other than iroh as stack-defining.
- **The log domain is split four ways.** P2mere holds the model and the sync
  protocol. Stickleback holds validated storage, causal processing and
  `verify_edition_chain`, and is the only Eidetic crate that depends on
  P2mere. Gemot's `site-model` verifies site logs (`verify_site_log`). Murm
  holds resolve and holder.
- **Coupling.** In the month to 2026-11-25:
  - 2 of the 72 commits touching `native/` also touched anything else.
  - 9 of the 54 commits touching `retinue` or `outrider` also touched radio
    or app crates.
  - Murm took P2mere from 0.8 to 0.12.1 in three weeks.
  - Of P2mere's 67 commits since 2026-11-03, the umbrella crate took 31,
    `spaces` 17, `net` 15, `store` 12, and `sync` and `encryption` 9 each.
- **Clone weight is history.** Murm's tree is about 20 MB; its pack is
  1.87 GiB.
- **Licences.** The Reticulum License covers `retinue`, `outrider`, `tulle`
  and `radio-hand`, all in Mesh. P2mere's crates keep upstream's MIT OR
  Apache-2.0. Murm's native crates are MPL-2.0.
- **Not redundant.** The Reticulum carrier is an adapter over Retinue's
  `Endpoint`. `truchman` is one SAM client under two I2P carriers. Retinue's
  `Node` and `Endpoint` routing became one core on 2026-11-19
  ([shared transport core](2026-11-18_shared_transport_core_plan.md)).

## Open

- **Names.** Peer fits `murm`: seven of the nine crates it takes from
  `native/` are `murm-*`, the
  `murm` crate on crates.io describes it, and consumers' `murm.git` URLs
  already reach it. Replication and Mesh are unnamed. Mesh could take
  `retinue`: its published crates already list `merely-made/retinue`, and that
  URL redirects to Murm today. Eight repositories still carry
  `[patch."https://github.com/merely-made/retinue.git"]` tables (cleromancy,
  distillery, esp, gemot, inker, isocosm, merelyllc.com, turnstone). They are
  dead now and live again if Mesh takes that name.
- **One session layer.** `StreamSync` has no live mode, and `JoinedSpace`
  still runs `p2mere-net`'s iroh-bound log sync. One layer with iroh as one
  carrier would retire the `net` patches above.
- **Order.** `p2mere-net` can leave P2mere only once Graphshell, Isocosm and
  Knot-editor stop naming it directly (consolidation T2–T5). Moving
  Stickleback out of Eidetic changes the `stickleback::` paths its consumers
  import.
- **Not yet judged.** Whether Gemot's `verify_site_log` duplicates
  Stickleback's editions. Whether Signalman's own message layer (about 1k
  lines, plus a 246-line adapter) duplicates `mere-comms`.
- **History.** A repository cut with filtered history keeps only its own
  paths' history. The current 1.87 GiB stays with whichever repository keeps
  Murm's full history.

Cleared 2026-11-25: the `repos/retinue` symlink (Mere `268c7a1f4`), the local
Pipit clone, `merely-made/pipit` (archived), and the Murm and Retinue Cargo
git caches.
