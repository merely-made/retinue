# Node link lifecycle and retinue 0.2.0

**Status (2026-10-02):** in progress. The verified work is merged on the
integration branch `site-canvas-integration`. Lanes L-A and L-B are building
the remaining phases. Nothing is on `main`, published, or repinned in mere yet.

## Purpose and authority

The `retinue-sim` harness (S7) and the
corroboration that followed turned up defects in `Node`'s link lifecycle. Mark ruled
on each. The rulings live in mer3ly's
`docs/2026-09-30_graphshell_site_canvas_plan.md` (Rulings 27-29, 39-61); this
doc cites them and does not restate them. It owns the code changes those rulings
make to the published `retinue` crate, the 0.2.0 version, and the downstream
repin.

Sibling plans: [retinue-sim](2026-10-01_retinue_sim_plan.md) (S7) and
[radio-mirror](2026-10-01_radio_mirror_plan.md) (S8).

## Phases

Each phase lands as its own commit or commits with tests. Done means the
phase's tests pass, the gate below passes twice, and both firmwares build.

| Phase | Ruling | Change | State |
|---|---|---|---|
| N1 | 27 | Every `Node` learns routes; only transit nodes forward | merged (`6894afc`) |
| N2 | 29 | `open_link(..., now)` addresses via `next_hop`, never via an expired route; version 0.2.0 | merged (`457911b`) |
| N3 | 28 | Unanswered link requests expire at `6000 x (relays + 2)` ms | merged (`978b360`) |
| N4 | 28, 45 | A node drops link data it sent itself when a relay echoes it back (`send(&mut self)`) | merged (`e2f82d3`) |
| N5 | 28 | A node drops an echo of its own announce before touching any state | merged (`d131367`) |
| N6 | 43 | `Endpoint` admission clocks become `tokio::time::Instant`; the ingress burst test runs on a paused clock | merged (`0093694`) |
| N7 | 46 | A distinct action for an expired link request, handled by radio-hand, its replay encoding, and the firmware | merged (`59b71e0`) |
| N8 | 50 | A caller-supplied first-hop airtime allowance per interface, wired from each firmware's LoRa profile | merged (`81d029d`) |
| N9 | 54 | `open_link(now)` expires stale requests itself | merged (`246dfa8`) |
| N10 | 51 | `Endpoint` own-echo, including Channel: control first, fix if affected | merged (`3771f6a` controls, `9b63d99` fix) |
| N11 | 52 | Received duplicates in `Node` and `Endpoint`: control first, then match RNS | merged (`3771f6a`, `9b63d99`) |
| N13 | 65 | `reliable.rs` `on_identify` refuses to replace a set peer or adopt its own identity, with a control | merged (`de9e244` control, `30210a2` guard) |
| N14 | 66 | Own-echo and duplicate windows get dedicated per-profile capacity constants | merged (`788be35`) |
| N15 | 67 | Own-echo drops and duplicate drops are counted separately in `Node` and `Endpoint` | merged (`46922f8`) |
| N12 | 29, 41 | Publish 0.2.0 (Mark's separate call) and repin mere, coordinated with mere's active session | open |

**The gate**, run with `--locked`:
- `cargo test -p retinue -p retinue-sim -p radio-face -p radio-mirror --all-features --no-fail-fast`;
- `cargo test -p radio-hand --no-fail-fast` with default features;
- CI's `cargo clippy --all-targets --all-features -- -D warnings`;
- `cargo fmt --check`;
- T114 and V4 release builds.

radio-hand with `--all-features` fails to link on the host (defmt and embassy
symbols), on `main` too, so it is not part of the gate.

## Findings

**2026-10-01, from `retinue-sim` traces:**
- **The pending-link table could wedge.** At the T114 bound of 4, four lost
  requests filled it permanently.
- **A sender surfaced its own link data.** A relay's retransmission decrypted
  under the shared link key, and the sender delivered it as received data.
- **Echoes made nodes their own peers.** Every node recorded itself as a peer
  from its own announce echo.
- **Only transit nodes learned routes** (the gate on `node.rs` learning), so a
  leaf could not address a first relay.

**2026-10-02, V1 corroboration against RNS 1.5.4.** Receipts are in
[`testing/receipts/rns-1.5.4-link-echo-corroboration/`](../testing/receipts/rns-1.5.4-link-echo-corroboration/README.md).
The source review and black-box observation agree.
- **Link-request timeout.**
  - RNS waits a first-hop term plus 6 s x max(1, hops), sends the request
    once, and reports the reason TIMEOUT. That is the same reason it gives for
    an established link that is later lost.
  - Observed: 12.0 s at 0 relays, 18.0 s at 1, 24.0 s at 2, and 30.0 s at 3.
  - The first-hop extra tracks interface bitrate.
  - N3 matches, except for that airtime term, which N8 adds.
- **Own echo.**
  - RNS filters by the hash of each packet sent and drops received
    duplicates, so N4 matches.
  - RNS skips Channel in that filter. Observed: an echoed own Channel message
    was delivered as if it came from the far end, and the far end's message
    with the same sequence was lost. Retinue does not copy this (N10).
- **Own announce.**
  - N5 matches on no peer, no path, no handler event, and no re-relay.
  - RNS additionally writes its own identity into `known_destinations`;
    retinue writes nothing.
- **Side finding.** On TCP-type interfaces, a configured bitrate below 62,500
  makes RNS set `HW_MTU = None`, and every inbound frame raises.
- Per Mark (mer3ly Ruling 53), these RNS behaviours are held, not reported,
  and get re-checked when the oracle repins past 1.5.4.

**2026-10-02, integration defects.**
- The weave merge driver's merge of N5 (`835e679`) silently dropped the five
  `use` lines heading `node.rs`'s test module. They were restored in
  `d13e7a0`. A line-level check found every added line from the merged
  branches present.
- N4's test called the 0.1 `open_link` and was fixed in `00e4255`.

Before trusting a weave merge of `node.rs`, check that it compiles its tests.

## Source review and provenance

This batch is **source-informed work, not strict clean-room work**. It is
described here under the
[2026-09-26 source-review policy](2026-08-25_permissive_radio_protocol_compatibility_survey.md#source-review-policy-update-2026-09-26).
No RNS implementation code was copied or translated into retinue.

**Comparative reads (permitted review):**
- **Lane R1, 2026-10-01: RNS 1.5.5**, upstream master `e40191b`. These files
  were fetched by `curl` without a download approval, into a session
  scratchpad outside the repo, and deleted on 2026-10-01:
  - `Link.py`: lines 75, 204, 208, 270-290, 323, 715-745;
  - `Transport.py`: lines 118, 439, 492, 1780-1802, 2062, 3135-3147,
    3185-3208;
  - `Reticulum.py`: lines 142, 1810-1845.
- **Lane R2, 2026-10-01: RNS 1.5.4** from the oracle venv. `Link.receive`, plus
  `Transport.py` near 1591 (outbound hash storage) and 1624-1679 (the packet
  filter).
- **Lane V1, 2026-10-02: RNS 1.5.4** from the oracle venv:
  - `Link.py`: 75-118, 279-323, 662-705, 712-776, 929-960, 1147-1155;
  - `Reticulum.py`: 134, 142, 1717-1745;
  - `Transport.py`: 1443-1605, 1624-1679, 1960-1961, 2171-2213, 2483-2537,
    3135-3142, 3194-3197;
  - `Identity.py`: 101-113, 559-599;
  - `Packet.py`: 363-368;
  - `Channel.py`: 351-390;
  - `Interfaces/Interface.py`: 250-262.

**What was discarded as adaptation territory.** R1's and R2's designs, which
were taken from their reads, were discarded without being merged
(mer3ly Rulings 39 and 47). R1's uncommitted diff was restored away.

**How the landed code was derived:**
- **N3's formula** comes from the manual's `ESTABLISHMENT_TIMEOUT_PER_HOP = 6`
  ([wire reference §2.5](2026-07-13_rns_wire_format_reference.md)) plus a
  hop-scaling shape taken from Prns `prns-core/src/routing/timing.rs`.
  - Prns had been withdrawn on 2026-09-26 when that lane used it.
  - The lane's brief also pointed it at a record that described R1's
    discarded formula.
  - The formula therefore rests on V1's observation of RNS 1.5.4, which
    matches it at 0-3 relays.
- **N4** is built from this repo's wire reference §3.2.6 (the packet hash
  excludes hops, header type, and transport address), and V1's observation
  corroborates it.
- **N5** is a self-destination check written from the defect, and V1
  corroborates it.
- **N8's airtime term** is the relationship V1 *observed* (12.079 s at
  62,500 bps against 12.001 s unbounded), not a transcription.

## Progress

- 2026-10-01: S7 finds the defects. Lanes R1-R3 are opened; R1 and R2 read RNS
  source, and their designs are discarded. Clean lanes C1-C3 re-derive.
- 2026-10-02: V1 corroborates. Mark rules 43-61. Integration branch
  `site-canvas-integration` merges S7, R1's clean commits, C1-C4, R3, and V1:
  412 + 235 tests, 0 failed, twice; clippy and fmt clean; both firmwares
  build. Lanes L-A (N7-N9) and L-B (N10-N11) open.
- 2026-10-02: L-B merged (f62c50f). Its controls failed at `3771f6a`, which the
  coordinator re-ran:
  - `Endpoint` surfaced its own echoed link data and its own echoed Channel
    message. The Channel case is the defect V1 saw in RNS 1.5.4.
  - Both `Node` and `Endpoint` delivered a far-end duplicate twice.
  - Channel duplicates were already dropped by sequence, so that control
    passed and serves as a positive control.

  At `9b63d99` everything passes. `Endpoint` gained sent and received
  windows, `Node` a received window, and one shared rule decides which link
  contexts are covered. The gate on the integration branch is 418/0 for the
  host crates. Five forks go to Mark.
- 2026-10-02: L-A merged (0b5995a).
  - Rulings 46, 50, and 54 are N7-N9. Ruling 55 adds a `link_request_expired`
    trace event: the warm trace's three lost sends expire at 200,000,
    380,000, and 560,000 ms. Ruling 56 makes garage a leaf; both stories are
    unchanged, and only garage's own forwards drop.
  - `b79481e` gates `node_link_timeout` on `alloc`. C1 had introduced that
    `--no-default-features` failure. `request_string_map` fails the same
    way on `main` and is a separate, pre-existing issue.
  - Weave auto-resolved 5 entities. Every line L-A and L-B added is present;
    lines from earlier lanes that are missing are deliberate rewrites, absent
    from the later lane's own tip too.
  - The gate passed twice: host 425/0 and radio-hand 236/0, with clippy and
    fmt clean. T114, V4, and V4 `resident-protocols` all build.
  - L-A's seven forks go to Mark.
- 2026-10-02: L-C merged.
  - Its IDENTIFY control failed at `de9e244`, re-run by the coordinator: an
    echoed own IDENTIFY became the peer, `(true, false, false)`. It passes
    at `30210a2`.
  - Window constants: `OWN_ECHO_HASHES` and `DUPLICATE_HASHES` are 1024 on
    `desktop` and 16 on `small`.
  - Counters: `own_echo_dropped` and `duplicate_dropped` on `Node`'s
    `TransportCounters` and `Endpoint`'s `RoutingCounters`.
  - Weave auto-resolved 6 entities. Every L-B line absent after the merge is
    also absent at L-C's tip, as deliberate rewrites under Rulings 66-67, so
    nothing was lost.
  - Gate, run twice: host 429/0, radio-hand 236/0, and 264/0 with `replay` and
    `instances`. Clippy and fmt are clean; retinue checks with `alloc` only.
- 2026-10-02 (3b20342): T114's radio is configured from `board::DEFAULT_*`
  (Ruling 70). The values are unchanged and T114 builds clean.
