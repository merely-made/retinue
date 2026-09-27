# Wire compatibility and ownership boundaries

**Status, 2026-09-27:** phases 1 and 2 implemented and verified; phase 3 partially implemented.
RNS 1.5.4 is the live oracle within the [recorded scope](../testing/receipts/rns-1.5.4-repin/README.md).
Versioned receipts are preserved, with concrete host/core boundary defects repaired.
The earlier 1.5.2 and signed-artifact-only 1.5.4 receipts remain historical facts.

## Scope and evidence

This work targets the public-domain protocol through project-owned code and
observed reference behavior. Comparative source review remains permitted under
the recorded policy, but this qualification uses published packages, public APIs,
CLI output and packet observations. It does not copy or translate upstream code.
Prns is withdrawn as a trusted independent reference. Its old matrix results,
attribution and uncertainty remain recorded; they are not prerequisites to rerun
or current endorsements. Passing protocol tests cannot establish source authorship.

RNS 1.5.4 has replaced the live 1.5.2 pin; LXMF stays 1.1.1.
Physical firmware and public-network claims require their own receipts. Local TCP
success does not requalify radio scheduling, on-air relay, power, or board IFAC.

## Phase 1: current reference qualification

Run the twelve stock-RNS local gates, seven Outrider gates, interleaved Resource
checks, persistent announce-timebase probe and full route-freshness / same-blob
diagnostic. Retain per-gate output and failed attempts. Build the current examples
before timing network gates, using the existing approved Retinue Cargo target.

Done: exact package acquisition and environment are recorded; each measured gate
has a result; failures are understood or explicitly limit the promoted claim;
current pin/status references agree; historical fixtures keep their actual producer
versions. Replay both 1.5.2 and 1.5.4 signed-artifact fixtures. Do not relabel older
byte captures or treat a single passing run as a bound on the flake rate.

## Phase 2: bounded implementation repairs

1. Host link bridges accept transit only from one of the bridge's recorded pair
   of interfaces. A packet from a third interface must neither forward nor renew
   bridge lifetime. Done: deterministic tests exercise both permitted directions
   and the third-interface refusal, matching the embedded Node invariant.
2. Channel read buffering respects `READ_BYTES` when a decoded frame is larger
   than remaining capacity. Retain already-proven bytes until the caller drains
   them, preserve ordering and EOF, and stop draining further Channel messages
   while a frame remainder is pending. Done: small-capacity tests cover a large
   compressed frame followed by ordinary data and EOF, without truncation.

These are distinct ownership repairs, not a Node/Endpoint merger or a new codec.
The buffer repair does not by itself cap the allocation required to decompress
one frame. A separate decoded-frame budget needs an explicit compatibility and
failure contract before it can be claimed as a total memory limit.

## Phase 3: next compatibility gates

- Specify single-radio rebroadcast versus multi-interface relay. Feed equivalent
  valid announces into Node and Endpoint virtual interfaces and compare recipient
  sets, route changes and suppression decisions against the intended topology.
  Preserve deliberate topology differences instead of hiding them in a shared
  helper that assumes one host model. **Software gate passed:**
  `tests/announce_topology.rs` checks route learning, recipient sets, transport
  stamps, egress policy and duplicate suppression with a processed-input barrier.
- Specify embedded IFAC admission and egress at the carrier boundary before
  `Packet::decode`. Done requires positive/negative authenticated-frame tests and
  a separate physical receipt; host TCP/Tulle IFAC results do not close it.
- Define configurable decoded-frame limits and error propagation for compressed
  streams. Measure peak allocation and prove that over-limit input cannot silently
  acknowledge and discard bytes while presenting a healthy stream to its caller.
  **Implemented with software tests:** the default ceiling is 65,536 bytes and
  can be set before stream creation. Owned decoded output reserves the ceiling
  plus one sentinel byte without vector growth. A 100,000-byte expansion is
  rejected at a 32-byte limit; an exact fit reports vector capacity 33. Invalid,
  unsupported or oversized compressed input is terminal, preserves the delivered
  prefix and reaches `LinkStream` as `InvalidData`. The driver sends link close.
  Known failures withhold proofs; already-admitted queued packets can have been
  proved before deferred decoding fails, covered by a separate model test.
  **Still open:** total peak allocation including decoder workspace and allocator
  overhead. The output-vector test does not close that broader measurement.
- Add a stock-RNS reliable Channel/Buffer live gate, including compressed data
  carrying EOF. The existing Endpoint stream oracle exchanges raw link packets;
  reliable host integration and model tests cannot substitute for that peer gate.
  **Stock-RNS 1.5.4 gate passed:** the new two-link driver verifies ordinary
  Buffer traffic and a public Channel message carrying compressed data plus EOF,
  with exact bytes and EOF in both directions. It is now in the live runner.
- Add queue-pressure and route-expiry qualification separately. This repin does
  not establish I2P keepalive behavior, discovery metadata compatibility, natural
  elapsed expiry, public-network behavior, or arbitrary-load scheduling parity.

## Findings (2026-09-27)

### Phase 3 orchestration

Three non-Astra lanes own disjoint test/implementation surfaces:

| Lane | Owner surface | Done-condition |
| --- | --- | --- |
| Announce topology | Paired Node/Endpoint integration tests | Equivalent signed input establishes route learning, topology-specific recipient sets, duplicate suppression and Endpoint egress policy. |
| Stream limits | Channel, ReliableChannel and host LinkStream | Caller-configurable decoded-frame budget; bounded output allocation; terminal errors reach an awaiting reader; delivered prefixes remain readable and failure cannot appear as healthy EOF. |
| Stock reliable peer | New Rust example and public RNS API driver | Exact payload and EOF in both directions over a real link, with observed compressed data and EOF on the same Channel message. |

The parent integrates the live runner, reviews boundaries and records results.
Software gates precede any physical claim. Queue pressure and elapsed route expiry
remain separate lanes.

### Embedded IFAC prerequisites

Firmware has two production carrier seams: `radio-hand/src/channel/node.rs`
decodes directly in `Event::RadioFrame` and encodes in `transmit`, while
`radio-hand/src/instances.rs` does the same in retained Retinue receive/actions.
Both must apply the existing `retinue::ifac::Ifac` envelope outside logical
`Packet` decoding. The replay diagnostic consumes logical packets and must not
silently acquire a physical-carrier envelope.

`node.rs` currently uses a fixed `LINK_MTU = 255` for both link roles, matching
Selvage's physical frame limit before IFAC. An eight-byte access code would make
a full logical packet too large. A complete lane therefore needs a configured
logical MTU that reserves the access-code bytes before link negotiation and
packet formation, then propagation through both firmware shells. Credential
configuration and durability remain caller-owned; do not add a second IFAC codec
or silently enable credentials.

Link MTU alone is insufficient: `Node::send` checks the configured payload cap,
but `Link::data_packet` and `Packet::encode` do not enforce the negotiated size.
Announce app-data limits also need to account for the carrier budget. A relayed
Type1 announce becomes Type2 and gains sixteen address bytes, so valid ingress
does not guarantee valid egress. Preserve an observable final frame-size refusal
in both shells. ResourceSender already sizes its parts and advertisement from
the negotiated link MTU and should keep owning that calculation.

Software acceptance must exercise both shells, exact physical frame limits,
matching credentials, wrong credentials, tampering and plain-frame refusal on a
protected carrier. Physical acceptance requires identified firmware and peers
with matching settings plus negative cases. Neither is closed by the host IFAC
receipt or by this phase's software-only orchestration.

### Existing owner seams

| Boundary | Current ownership and finding |
| --- | --- |
| Shared wire and transfer logic | `packet`, `link`, `channel`, `reliable`, `resource`, and `resource_transfer` are shared by the runtimes. Node and Endpoint both use ResourceSender/ResourceReceiver. Preserve this seam. |
| Compression feature | The optional bz2 I/O adapter needs `std` independently of Tokio. A compression-only library check exposed a missing import; its feature guard now matches that ownership. Allocation-only and core-only consumers remain distinct. |
| Host versus firmware runtime | `node.rs` explicitly owns caller-driven bounded firmware state; `endpoint.rs` owns asynchronous host tasks/interfaces. Different lifecycle and allocation requirements justify separate runtimes. |
| Routing policy | Route learning, announce relay and link bridges are implemented in both runtimes. Node rejects ingress outside a bridge pair; Endpoint initially treated every non-`from` interface as the reverse direction. This is a concrete parity defect. |
| Announce egress | Node's `relay_announce` emits on its ingress radio; Endpoint fans out to other permitted interfaces. The difference needs a topology-specific test, not an unsupported assertion that one is universally wrong. |
| IFAC | Host Endpoint and `iface/tulle.rs` apply the carrier envelope. Both `radio-hand/src/channel/node.rs` and `radio-hand/src/instances.rs` decode received RF directly as a Packet; firmware IFAC and its MTU reservation remain outside the measured host claim. |
| Read buffering | `channel::Buffer::fill` initially appended whole decoded frames after only testing whether the read buffer was below its limit. A large frame can cross that limit. |
| Radio execution | Selvage owns portable PHY/wire/controller facts; Tulle re-exports them and supplies host async radio interfaces; radio-hand owns physical execution. Sibling protocols retain their own protocol state. |

Manifest inspection confirms Retinue's optional `tulle-radio` host adapter and
radio-hand's `default-features = false` Retinue dependency preserve the runtime
split. No benchmark in this audit establishes duplicated routing decisions as a
performance bottleneck. The issue established here is divergent behavior and
maintenance risk, not measured inefficiency. Avoid a broad abstraction rewrite
until the paired behavioral tests identify a useful common policy seam.

## Progress

- September 27: read-only code/manifest audit completed with a non-Astra specialist.
  Identified two bounded repairs and explicit firmware/relay follow-ups.
- September 27: 12 stock-RNS gates, 7 Outrider gates and 12 repeated Resource
  executions passed at 1.5.4. Timebase decisions, all 72 route decisions and six
  same-blob decisions agree with retained 1.5.2 evidence. The environment and
  requirements now stay at 1.5.4; LXMF remains 1.1.1.
- September 27: two non-Astra implementation lanes repaired bridge admission and
  stream buffering. Review additionally caught Node refreshing/deduplicating foreign
  bridge input, and the host needing to drain frame remainders before EOF. Final
  qualification passed 397 default tests, 178 allocation-only tests, the core-only
  check, strict Clippy, and a fresh 12/12 stock-RNS run after repairs. Formatting
  and registry checks passed after formatting tests and registering the new runner.
  See the receipt for logs, hashes, the initial registry failure, and scope limits.
- September 27: three Sol lanes added paired announce-topology tests, bounded
  compressed-frame decoding with host errors, and a stock-RNS reliable peer gate.
  The combined run passed 403 tests; final library verification after the
  compression-only feature repair passed 266, allocation-only passed 178, and
  core-only/compression-only checks plus strict Clippy passed. The expanded live
  runner passed 13/13. Failed compilation, stalled builds and harness teardown
  attempts remain in the [lane receipt](../testing/receipts/rns-1.5.4-lanes/README.md).
  The firmware audit identified both carrier shells, fixed Node MTU and relay
  header growth as IFAC prerequisites. Physical gates and total decoder peak
  allocation remain open.
