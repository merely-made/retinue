# Repository layers: replication, peer, mesh

**Status, 2026-11-25: layers, names and history decided; no code has moved.** This
replaces the repository boundaries Murm, P2mere and Eidetic grew into with
three layers: Replication in `p2mere`, Peer in `murm`, Mesh in `retinue`.
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
| Replication | Eidetic, Dramatis | Stickleback uses `muniment`, and `insigne` and `personae`. |
| Ten other repositories | Replication | The data model. |

P2mere has no family dependency today; Stickleback brings the two above.
Eidetic then stops depending on P2mere, because Stickleback is its only crate
that does.

The umbrella `p2mere` crate and `p2mere-blobs` are not carried forward: no
family crate depends on them.

Links: area repositories · draws on transport consolidation (boundary) · plans the replication, peer and mesh cuts · summary three repositories by layer; old repository names are not boundaries

### repository_layers D2 — names, and the fork stays its own repository

Question (2026-11-25): what are Replication and Mesh called, and should the
P2mere fork live in another family repository? Answer: "mesh is retinue.
p2mere is replication, or we should consider where that fork should live, if
there's another replication home (mere? graphshell? gemot?) etc. peer is
murm."

| Layer | Repository |
| --- | --- |
| Replication | `merely-made/p2mere` |
| Peer | `merely-made/murm` |
| Mesh | `merely-made/retinue` |

Mark left the other homes open. The agent read them the same day and found
none better. The reasons below are the agent's, drawn from the evidence
section.

- **Consumers do not see the home.** Every family manifest takes `p2mere-*`
  from crates.io by version, and none names `p2mere.git`. The home decides
  only where intakes and patches happen.
- **It is a live fork.** It is level with upstream's 2026-11-18 tip and takes
  intakes by plain merge. Inside another repository it becomes a subtree:
  merges need the subtree strategy, and GitHub's fork link goes.
- **Little of it is the family's.** Outside `net`, the fork differs from
  upstream by about 100 source lines. With `net` in Murm, what is left changes
  when upstream changes.
- **Mere, Graphshell and Gemot sit above Peer.** Each takes `murm-sync` from
  Murm, and Murm depends on Replication. A replication change would release
  from a repository that then needs Murm's update of it. All three are
  MPL-2.0 against the fork's MIT OR Apache-2.0, and Mere is 343 MB on GitHub
  against the fork's 6 MB.
- **Eidetic is the nearer candidate.** Its README names replication, it holds
  Stickleback, and the nine other consumers of `p2mere-core` already depend
  on it. It would still carry upstream's history as a subtree for nothing a
  consumer sees. It gives up Stickleback, and the word, instead.

Links: area repositories · refines D1 · summary Replication is p2mere, Peer is murm, Mesh is retinue; the fork stays its own repository

### repository_layers D3 — Retinue keeps the old history

Question (2026-11-25): which repository keeps Murm's inherited history:
`retinue`, `murm`, or neither? Answer: "Retinue keeps it".

The existing repository takes the name `retinue` back and keeps every commit.
`murm` is a new repository cut from `native/`.

Links: area repositories · refines D2 · summary the existing repository becomes retinue with its history; murm is cut fresh from native

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
- **Where the fork's own code is.** Against upstream's 2026-11-18 tip the
  fork changes 51 files, +2036 −413. `net` is 25 of them, +1527 −176. Source
  changes elsewhere: +79 −16 in `encryption`, +14 −8 in `core`, +5 −2 in the
  umbrella. `store`, `sync`, `stream`, `auth`, `spaces`, `discovery` and
  `blobs` differ only in manifests: package names, feature gates and
  dependency versions.
- **Where upstream works.** Of upstream's 93 commits since the 2026-10-07
  merge-base, the umbrella took 53, `spaces` 24, `store` 13, `net` and
  `stream` 8 each, `sync`, `encryption` and `auth` 5 each, `discovery` 3, and
  `core` and `blobs` 2 each. The family has put 22 commits into `net` since
  July.
- **How consumers reach it.** `p2mere-*` comes from crates.io by version in
  every family manifest. Stickleback comes from `eidetic.git` in Gemot,
  Isocosm and Murm; Isocosm keys it `murm-replication`. Stickleback itself
  uses `p2mere-core`, `-store` and `-encryption`, `muniment` from Eidetic, and
  `insigne` and `personae` from Dramatis.
- **Clone weight is inherited history.** Murm's tree is about 20 MB; GitHub
  reports 1.94 GB. `main` holds 66,072 commits from 50 roots. 54,283 of them
  are dated 2012 to 2025: Servo's history and other upstreams', which came in
  with code promoted from Mere and Genet. 3,260 commits touch today's
  `crates`, `native`, `apps`, `firmware` or `vendor`, and 114 touch `native`.
  The Servo roots are also in Genet's history, and most of the other roots in
  Mere's.
- **Licences.** The Reticulum License covers `retinue`, `outrider`, `tulle`
  and `radio-hand`, all in Mesh. P2mere's crates keep upstream's MIT OR
  Apache-2.0. Murm's native crates are MPL-2.0.
- **Not redundant.** The Reticulum carrier is an adapter over Retinue's
  `Endpoint`. `truchman` is one SAM client under two I2P carriers. Retinue's
  `Node` and `Endpoint` routing became one core on 2026-11-19
  ([shared transport core](2026-11-18_shared_transport_core_plan.md)).

## What the Murm cut touches

Read 2026-11-25 from `origin/main`. None of it has been done.

- **Inside the repository.** Four path dependencies cross the line.
  `mere-transport` and `murm-i2p` take `truchman`. `native`'s workspace takes
  `retinue`: optional in `mere-transport`, required by `murm` and `murm-i2p`.
  Signalman's optional `comms` feature takes `mere-comms`.
- **Native's history.** 114 commits touch `native/`, and they already carry
  the Mere ancestry under that path (`native/source-import.json`). Cut as a
  subdirectory, the new repository keeps all of it.
- **Consumers.** Eleven repositories name `murm.git`. Eight name only Peer
  crates and change nothing. Three name Mesh crates there: Graphshell
  (`retinue`), Turnstone (`retinue`, twice) and merelyllc.com (`radio-mirror`,
  `radio-face`, `embedded-graphics` and `embedded-graphics-core`, six lines).
- **The `retinue.git` patch tables.** Lines that send Peer crates to
  `murm.git` are dead today and correct after the cut. Lines that send Mesh
  crates there break the build once the new repository exists: Turnstone's
  `retinue` and merelyllc.com's four.
- **Cargo still clones Retinue.** Cargo fetches every git source in a graph
  to resolve it, optional or not. While Murm takes `retinue` and `truchman`
  from `retinue.git`, each Peer consumer clones the 1.94 GB anyway. From
  crates.io it does not. `truchman` 0.1.1 there is current. `retinue` 0.3.0
  there is from 2026-10-06, 250 commits back, before the shared transport
  core and the RNS 1.6.1 stamp policy that `mere-transport` now uses.
- **One `retinue` per graph.** Signalman with `comms` on would see Murm's
  `retinue` beside its own, and so would Graphshell and Turnstone, which
  pair `retinue` with `transport/reticulum`. Each needs one source for it.
- **Locks.** Mere (two) and merelyllc.com (one) commit `Cargo.lock`. Every
  other checkout holds a local lock whose Murm revisions will not exist in
  the new repository, and needs `cargo update` for the Peer packages.
- **Local.** The worktree `Code/worktrees/murm-knot-sync-close` hangs off
  this clone. Mere's `.cargo/config.toml.example` and
  `support/standards/transport_catalog.toml` name `../murm/crates/...` for
  Mesh crates.
- **Docs.** Ten plans here are Peer's by name, this one among them.

## Open

- **Taking the names.** `merely-made/retinue` redirects to Murm today, and a
  repository created under that name ends the redirect. Eight repositories
  carry `[patch."https://github.com/merely-made/retinue.git"]` tables
  (cleromancy, distillery, esp, gemot, inker, isocosm, merelyllc.com,
  turnstone). They are dead now and apply again once Mesh holds the name.
  Graphshell takes `retinue` from `murm.git`.
- **Dropped crates and intakes.** Upstream puts most of its work into the
  umbrella crate. Deleting the umbrella, `blobs`, `net` and `discovery` from
  the fork makes every intake that touches them a delete conflict. Left as
  upstream ships them, outside the workspace and unpublished, they keep
  intakes plain merges, and the fork's `net` can return to upstream's text.
- **`net` stops following upstream.** Once it is Murm's, upstream's changes
  to it arrive by hand or not at all.
- **One session layer.** `StreamSync` has no live mode, and `JoinedSpace`
  still runs `p2mere-net`'s iroh-bound log sync. One layer with iroh as one
  carrier would retire the `net` patches above.
- **Order.** `p2mere-net` can leave P2mere only once Graphshell, Isocosm and
  Knot-editor stop naming it directly (consolidation T2–T5). Stickleback
  keeps its crate name, so its move changes a git URL in Gemot's, Isocosm's
  and Murm's manifests and no Rust path.
- **Not yet judged.** Whether Gemot's `verify_site_log` duplicates
  Stickleback's editions. Whether Signalman's own message layer (about 1k
  lines, plus a 246-line adapter) duplicates `mere-comms`.
- **History.** D3 leaves the 1.94 GB with Retinue. A later cut that follows
  Mesh's paths can still shed it.

Cleared 2026-11-25: the `repos/retinue` symlink (Mere `268c7a1f4`), the local
Pipit clone, `merely-made/pipit` (archived), the Murm and Retinue Cargo git
caches, and Eidetic's unused `mere-transport` workspace dependency (Eidetic
`16430c3`).
