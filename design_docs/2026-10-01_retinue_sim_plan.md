# retinue-sim: the route-trace harness

**Status (2026-10-07):** landed. S7a and S7b merged to `main` in `fa4f925`
(2026-10-02), with the 0.2.0 link-lifecycle batch. mer3ly's site-canvas
Rulings 27-30 ruled forks 1-5 and 9, and Rulings 55, 56 and 76 changed the
trace and the lab after them. Forks 6-8 stand as built, with no ruling
recorded (see the 2026-10-07 disposition under Forks). S7c, the face track,
is built on branch `radio-consumers` and fast-forwarded to `main` on 2026-10-07.

What follows is the status as of 2026-10-01, kept as written.

**Status (2026-10-01):** in progress on a worktree lane; not merged. Provisional
choices await rulings (Forks, below).

## Purpose and authority

This is phase S7 of mer3ly's
`mer3ly/docs/2026-09-30_graphshell_site_canvas_plan.md`. That plan holds the
rulings; this doc cites them and does not restate them:

- Ruling 1: a stack capability any consumer uses unchanged. No `mer3ly` names in
  types, schema ids or APIs.
- Ruling 7: Retinue nodes run in process over a topology, a link is blocked, and
  the real route trace is emitted.
- Ruling 9: two scenarios, a cold cut and a warm cut that advances to the next
  announce.
- Ruling 17: a new crate holds the shared-radio medium, the topology, link cuts,
  a simulated clock and the trace schema. `Node` gains first-relay addressing
  and a read-only next-hop accessor.
- Ruling 24: the crate is `retinue-sim`, unpublished, at `crates/retinue-sim`.
- Ruling 8 (context): each node's state in the trace must fill radio-face's
  TRAFFIC page and ticker.

## Phases

**S7a: `Node` changes (published crate, MSRV 1.88).**
- `open_link` addresses its request to the first relay of a learned route
  (header type 2 naming the relay), using the same helper `Endpoint`'s
  `address_for` uses (`Packet::address_via`).
- `Node::next_hop(destination, now) -> Option<NextHop>`: read-only, excludes
  routes past their TTL without evicting them.

Done when `cargo test -p retinue` and clippy pass, existing tests unchanged, and
a new test shows a transit node's request leaving as type 2 through its relay.

**S7b: the `retinue-sim` crate.** Topology (names and edges as data), a
shared-radio medium (a frame is heard by every uncut neighbour after a fixed
delay), cuts, a simulated millisecond clock with deterministic event order, and
a versioned JSON trace, `retinue-sim.route-trace/v1`.

Done when, over the lab topology held in an example:
1. both traces serialize byte-identically across two runs;
2. the cold trace delivers fire → church → water → garage;
3. the warm trace delivers fire → water → garage, then shows undelivered sends
   after the cut, then delivers through church after the next announce;
4. `cargo test` and `cargo clippy` are clean on both crates;
5. the schema id is neutral and documented in the crate rustdoc.

**S7c: the face track (2026-10-07; site Rulings 1 and 127).** The mapping from a
trace's `NodeState` to radio-face's `LocalStatus` and `HostSnapshot` existed only in a
test, so a consumer drawing a node's face would have re-derived it, which Ruling 1
rules out. Ruling 127 has the site commit traces generated outside its build, at a
pinned retinue revision.
- `retinue_sim::face::face`, behind a `face` feature that enables an optional
  radio-face dependency, ships the mapping, and the test uses it.
- `face::FaceTrack` derives a separate file, `retinue-sim.face-track/v1`: for each
  event carrying a node state, that node's face as radio-mirror's local and host JSON
  documents, plus the SHA-256 of the route trace it came from.
- The lab example prints it with `--faces`.

Done when:
1. `retinue-sim.route-trace/v1` output is byte-identical to before;
2. face tracks are byte-identical across runs and round-trip;
3. every entry's documents are read by radio-mirror's own JSON readers back to the
   mapped face;
4. radio-mirror's dependency set is unchanged (it links nothing of retinue);
5. fmt, clippy (CI's flags) and the workspace tests are clean.

## Findings

All 2026-10-01, from the lab traces (`crates/retinue-sim/examples/lab/`), five transit
nodes at the T114 table bounds (32 peers, 8 actions, 4 links, 16 routes), a 100 ms hop
delay and a 5 s poll.

**Cold.** Fire's request is addressed to church; its route has hop count 2. The data
frame goes fire → church → water → garage: three transmissions, two relays. Retinue's
route hop count is the number of relays.

**Warm.** The first send crosses the shortcut (fire → water → garage, delivered at
10.6 s). After the cut at 60 s, the sends at 180, 360 and 540 s are lost: each request
names water as its first relay, water cannot hear it, and church hears it and drops it
because it is not addressed to church. Fire's route moves to church at 600.3 s, when
garage's 600 s announce arrives through water and church. A newer timebase replaces a
route whatever its hop count (`node.rs` `learn_route`; `announce_freshness.rs`
`evaluate`). So the next announce ends the outage, not the route TTL; the old route
would have expired at 1,800.2 s. The 720 s send arrives through church.

**A sender receives its own link data.** On a shared radio the first relay's
retransmission is heard by the hop before it. `Link::receive` decrypts anything
addressed to the link id with the link's one key, so fire's `Node` returns
`Action::Data` for its own payload (10.8 s cold, 10.6 s warm). The trace records it as
a `data` effect at fire; only the destination counts as a delivery. Not fixed here.

**Each node learns itself.** A relay's re-broadcast of a node's own announce is admitted
to that node's address book and returned as `Action::Learned`, so `peers` counts the
node itself (5 in a 5-node mesh). `learn_route` skips self (`node.rs`, the
`destination == self.destination()` guard); `AddressBook::ingest` does not.

**The pending-link table can wedge.** `Node` has no timeout for a link request nobody
answers (`pending` is cleared only by a proof or `force_interrupt`). At the T114 bound
of four, four lost requests fill it, and `open_link` refuses every later send. Verified
by adding a fourth lost send at 590 s: the 720 s send, after the reroute, is refused as
`pending_full`. The lab's warm scenario loses three.

**Only transit nodes can address a first relay.** `Node` learns routes only when its
transport policy relays (`if relay_announces || relay_packets` before `learn_route`).
A non-transit node has no route, so `open_link` sends header type 1, and relays carry
only type-2 requests that name them. `Endpoint` learns paths whatever its policy. The
existing `transport_relays_announce_request_and_proof` test uses a non-transit source,
so its hand-set transport field is still needed and was left alone.

**The firmware face notes little.** The Retinue channel node
(`radio-hand/src/channel/node.rs`, `perform`) writes the event line only for link up,
link down, a resource echo and timebase exhaustion. Forwards, announces and link data
change counters only. Drawn faithfully, a relay's ticker stays on its last event while
its TX/RX counts climb.

**Sizes.** Cold: 95 events, 82,060 bytes. Warm: 204 events, 183,105 bytes (pretty JSON).

**Compact sizes and face tracks (2026-10-07, branch `radio-consumers` on `3dd84b1`).**
The lab example prints each file with a trailing newline. Sizes are of those files;
gzip is `gzip -9`.

| File | Events or entries | Raw bytes | gzip |
| --- | --- | --- | --- |
| cold route trace | 79 events | 33,724 | 2,639 |
| cold face track | 76 entries | 27,015 | 1,133 |
| warm route trace | 171 events | 74,641 | 4,801 |
| warm face track | 163 entries | 59,312 | 1,995 |

Both route traces are byte-identical to `3dd84b1`'s lab output. A face track's
`trace_sha256` is the digest of its route trace's canonical JSON, without the trailing
newline.

**What the face carries (2026-10-07).** The trace has no board, firmware, power,
profile or uptime, no RSSI or SNR, and no peer names or ages. So the face fills
counters, last TX and RX lengths, link counts, the Retinue personality and the face
line, and leaves the rest at radio-face's defaults. The Retinue channel node also
publishes a named `NodeSummary`, up to three peers and `IfacState::Off`
(`radio-hand/src/channel/face.rs`); the face track does not. The face line is still
the harness's own copy of the channel node's notes (fork 7).

## Forks

Provisional choices, built so the lane could verify end to end. Each is open.

1. **`next_hop`'s signature** (published crate). Built: `next_hop(&self, destination,
   now) -> Option<NextHop { interface, via, hops }>`, which hides a route past its TTL
   without evicting it. Alternatives: no `now`, reporting a route until it is evicted;
   widen `route_to`'s return (a breaking change); add the learned time.
2. **`open_link` has no clock.** It addresses by a route that may be past its TTL but not
   yet evicted (every `ingest` and `poll` evicts). Alternatives: an `open_link_at` taking
   `now`; a `now` parameter (breaking).
3. **Route learning for non-transit nodes** (finding above). Keep as is, or learn routes
   on every node and relay only under transit, a behaviour change in the published
   crate.
4. **The trace's format.** Built: pretty JSON. Alternatives: compact JSON, about half
   the size; a binary codec.
5. **The trace's shape.** Built: each `transmit` and `receive` carries the acting node's
   state. Alternatives: every node's state at every step; deltas only.
6. **What a send is.** Built: one link per send, with the payload sent once the link is
   up. With one long-lived link instead, data follows the relays' link bridges rather
   than routes, so the next announce does not repair it: fire's link stays up until 15
   minutes of silence (`LINK_IDLE_TIMEOUT`) and nothing reopens it. Ruling 9's story
   holds for the per-send model.
7. **Where the face mapping lives.** Built: the harness repeats the channel node's
   "link up" and "link down" notes, citing it. Alternative: a shared function from
   `Action` to face event that both use.
8. **The node profile.** Built: `run` uses the T114 bounds and `run_with` takes any.
   With the T114 bounds the pending wedge is reachable.
9. **The echo and self-learning findings** are protocol behaviour outside S7, for
   retinue's owner to rule.

**Disposition (2026-10-07).** The list above is kept as written. Rulings are mer3ly's
site-canvas plan's.
1. Ruling 29: `next_hop` kept as built.
2. Ruling 29: `open_link` takes `now` and checks the route's TTL, a breaking change
   that moved retinue to 0.2.0 (`457911b`).
3. Ruling 27: every node learns routes, and only transit nodes forward.
4. Ruling 30: compact JSON (`e01196b`).
5. Ruling 30: the acting node's state. Ruling 55 added `link_request_expired` to v1
   (`4dbd553`), and Ruling 76 records the state at expiry (`83bda43`).
6. No ruling recorded. One link per send stands as built.
7. No ruling recorded on a shared `Action`-to-face-event function; the harness still
   repeats the channel node's notes. Separately, the `NodeState`-to-face mapping now
   ships in the crate (S7c), so a consumer does not repeat that.
8. No ruling recorded. `run` (T114 bounds) and `run_with` stand as built.
9. Ruling 28: all three fixes (link-request expiry, own-echo filtering, no
   self-learning), landed in the 0.2.0 batch; see the
   [link lifecycle plan](2026-10-02_node_link_lifecycle_plan.md).

Ruling 56 also made fire and garage leaves in the lab topology (`58a126e`).

## Progress

- 2026-10-01: plan written; S7a and S7b opened on the lane branch.
- 2026-10-01: S7a and S7b built. Lab traces meet done-conditions 1-3 and 5; tests and
  clippy are clean on both crates. Forks above are open.
- 2026-10-02: merged to `main` in `fa4f925` with the link-lifecycle batch, carrying
  Rulings 27-30, 55, 56 and 76.
- 2026-10-07: S7c built on branch `radio-consumers` (`688744b`, `96712e8`): the `face`
  feature, `face::face`, `FaceTrack`, and the lab example's `--faces`. retinue-sim
  passes 7/0 by default and 11/0 with `face`. The new tests cover face-track
  determinism and round-trip, acceptance by radio-mirror's readers, and the TRAFFIC
  page drawn through the mirror. CI gains a `cargo test -p retinue-sim --features
  face` step. Done-conditions 1-5 met.
