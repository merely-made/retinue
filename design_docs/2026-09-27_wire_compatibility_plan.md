# Wire compatibility and ownership boundaries

**Status, 2026-09-27:** phases 1 and 2 implemented and verified; phase 3 open.
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

RNS 1.5.4 is the proposed replacement for the live 1.5.2 pin; LXMF stays 1.1.1.
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
  helper that assumes one host model.
- Specify embedded IFAC admission and egress at the carrier boundary before
  `Packet::decode`. Done requires positive/negative authenticated-frame tests and
  a separate physical receipt; host TCP/Tulle IFAC results do not close it.
- Define configurable decoded-frame limits and error propagation for compressed
  streams. Measure peak allocation and prove that over-limit input cannot silently
  acknowledge and discard bytes while presenting a healthy stream to its caller.
- Add a stock-RNS reliable Channel/Buffer live gate, including compressed data
  carrying EOF. The existing Endpoint stream oracle exchanges raw link packets;
  reliable host integration and model tests cannot substitute for that peer gate.
- Add queue-pressure and route-expiry qualification separately. This repin does
  not establish I2P keepalive behavior, discovery metadata compatibility, natural
  elapsed expiry, public-network behavior, or arbitrary-load scheduling parity.

## Findings (2026-09-27)

| Boundary | Current ownership and finding |
| --- | --- |
| Shared wire and transfer logic | `packet`, `link`, `channel`, `reliable`, `resource`, and `resource_transfer` are shared by the runtimes. Node and Endpoint both use ResourceSender/ResourceReceiver. Preserve this seam. |
| Host versus firmware runtime | `node.rs` explicitly owns caller-driven bounded firmware state; `endpoint.rs` owns asynchronous host tasks/interfaces. Different lifecycle and allocation requirements justify separate runtimes. |
| Routing policy | Route learning, announce relay and link bridges are implemented in both runtimes. Node rejects ingress outside a bridge pair; Endpoint initially treated every non-`from` interface as the reverse direction. This is a concrete parity defect. |
| Announce egress | Node's `relay_announce` emits on its ingress radio; Endpoint fans out to other permitted interfaces. The difference needs a topology-specific test, not an unsupported assertion that one is universally wrong. |
| IFAC | Host Endpoint and `iface/tulle.rs` apply the carrier envelope. `radio-hand/src/channel/node.rs` decodes received RF directly as a Packet; firmware IFAC remains outside the measured host claim. |
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
