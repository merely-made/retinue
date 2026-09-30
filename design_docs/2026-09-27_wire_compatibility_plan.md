# Wire compatibility and ownership boundaries

**Status, 2026-09-30:** phases 1, 2 and 4 implemented and verified; phase 3 partially implemented;
phase 5 has bounded physical stock-peer receipts and restored original radios.
Sennet maximum-size stock acceptance remains unqualified after USB interruptions.
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
  **Implemented:** a shared radio-hand adapter, startup configuration in both
  shells, and a configured Node budget. Software and target results are recorded
  in the [firmware IFAC receipt](../testing/receipts/firmware-ifac/README.md).
  Provisioning and physical acceptance remain open.
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

### Embedded IFAC implementation and remaining acceptance

**Execution, September 27:** the owner approved the firmware IFAC software lane
after the topology/stream receipts. Three Sol agents cover Node packet budgets,
the two radio-hand carrier seams, and independent stock-RNS boundary captures.
The parent owns integration, target checks and receipts. The author's README at
`16c2784` is preserved.

The implementation contract is startup configuration: a caller supplies an
optional existing `Ifac` to a shared radio-hand carrier adapter, which reserves
its overhead from the physical frame limit. Existing constructors stay plain.
Node gains a validated logical MTU, with configuration changes refused while
sessions or carried links are active. Incoming protected frames authenticate
before logical decoding; final egress checks include relay-header growth.
Credentials, persistence and device provisioning stay with the caller.

Done for this software slice: exact boundary and refusal tests pass, both
firmware seams consume the adapter, Node link negotiation and local data honor
the configured budget, stock-RNS captures retain their real producer/version,
and target compilation is recorded. Physical acceptance and provisioned board
settings require their own identified-image receipt.

Both production seams now use `radio-hand/src/retinue_carrier.rs`:
`channel/node.rs` authenticates `Event::RadioFrame` and seals `transmit`, while
`instances.rs` does the same around retained Retinue receive/actions. The adapter
uses the existing IFAC codec; the logical replay diagnostic stays logical.
Protected construction rejects pending or active sessions even when the Node's
MTU already matches. Neither shell exposes a credential setter. A caller's
stricter Node budget is preserved.

Node defaults to 255 logical bytes, supports validated budgets down to the
167-byte plain announce minimum, and negotiates that budget in both link roles.
An eight-byte IFAC reserves eight physical bytes, leaving 247 logical bytes.
Direct sends account for CBC padding before allocation; fallible announces
include optional ratchet bytes. The unconstrained announce builder remains a
fixture tool. Relays check their final shape, including Type2's sixteen extra
address bytes, and refused link requests do not create phantom bridges.
ResourceSender retains ownership of its negotiated-MTU sizing.

The carrier independently checks final frames against Selvage's 255-byte limit.
Runtime records typed drops and rejection counts; NodeChannel uses its existing
undecoded/unsent counters. Host tests exercise the shared adapter and actual
Runtime. NodeChannel's hardware-facing integration is target-compiled; it has
not gained an emulated or physical driver receipt. Stock RNS 1.5.4 supplied
deterministic Type1 captures at the boundary; Type2 growth is tested locally.

Credential persistence, device provisioning and physical acceptance remain open.
They require identified firmware and matching peers plus negative cases.
Target compilation and local TCP capture do not close those gates.

### Credential provisioning preflight (September 27)

Two Sol audits traced the next lane through the actual board and host owners.
The opt-in carrier API is ready, but provisioning is a separate contract:

- `settings.rs` writes an identity-first 68-byte body. Both board stores read
  only the current body plus 32 bytes of extension space. Appending a 64-byte
  IFAC key exceeds older readers' capacity; after both A/B slots are rewritten,
  downgrade can reject both records and regenerate identity. Keep credential
  storage separate from this identity record. A persistent design also needs
  protected-mode downgrade behavior and an allocated, overlap-checked vault.
- `resident_wire.rs` version 1 is exactly 185 bytes. Its stream consumes those
  bytes before checking the version. Merely appending credentials to a version 2
  message lets older firmware interpret the suffix as ordinary commands. Reuse
  the existing bounded KISS demultiplexer for any extended exchange, with exact
  capability agreement before sending a secret. Do not arbitrarily narrow the
  owner's timing or lease ranges just to squeeze credentials into 185 bytes.
- The V4 resident setup is a local commissioning/probe path containing raw
  Sennet/Tucket material; it is not the authenticated management provisioner.
  The wall-node plan requires an encrypted carrier or a separately sealed
  payload bound to node, controller, transaction and operation. A signature
  alone does not satisfy that contract. `sealed_credentials` is opaque storage,
  not a sealing implementation; the current V4 applier refuses nonempty values.
  T114 credential custody remains host-owned in that plan.

The concrete prerequisite repaired here is accidental disclosure through Debug:
`SennetKey`, `ResidentSetup` and nested setup events now redact keys and identity
seeds while retaining diagnostic metadata. Wire bytes, setup admission and
identity storage are unchanged. The [preflight receipt](../testing/receipts/ifac-provisioning-preflight/README.md)
records the focused regression and audited owner boundaries.

The implementation choice remains explicit: volatile session provisioning first,
or a durable vault plus recovery. Volatile provisioning avoids a flash migration
but still requires the confidential, versioned exchange above. Neither choice
authorizes plaintext management secrets or live credential replacement. Software
acceptance must cover unsupported peers, every fragmentation boundary, wrong
recipient/context, tampering, redacted diagnostics and protected startup before
sessions. Durable acceptance additionally needs torn-write, rollback, downgrade
and identity-preservation receipts. No new credential command or board write is
claimed by this preflight.

### Existing owner seams

| Boundary | Current ownership and finding |
| --- | --- |
| Shared wire and transfer logic | `packet`, `link`, `channel`, `reliable`, `resource`, and `resource_transfer` are shared by the runtimes. Node and Endpoint both use ResourceSender/ResourceReceiver. Preserve this seam. |
| Compression feature | The optional bz2 I/O adapter needs `std` independently of Tokio. A compression-only library check exposed a missing import; its feature guard now matches that ownership. Allocation-only and core-only consumers remain distinct. |
| Host versus firmware runtime | `node.rs` explicitly owns caller-driven bounded firmware state; `endpoint.rs` owns asynchronous host tasks/interfaces. Different lifecycle and allocation requirements justify separate runtimes. |
| Routing policy | Route learning, announce relay and link bridges are implemented in both runtimes. Node rejects ingress outside a bridge pair; Endpoint initially treated every non-`from` interface as the reverse direction. This is a concrete parity defect. |
| Announce egress | Node's `relay_announce` emits on its ingress radio; Endpoint fans out to other permitted interfaces. The difference needs a topology-specific test, not an unsupported assertion that one is universally wrong. |
| IFAC | Host Endpoint and `iface/tulle.rs` retain their carrier envelope. Both radio-hand firmware shells now share an adapter around the same codec with Node MTU reservation. Board provisioning and physical acceptance remain separate. |
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
- September 27: three Sol lanes implemented the shared firmware IFAC adapter,
  startup configuration in both shells and Node logical budgets. Review added
  active-session startup refusal and prevented phantom bridges on oversized
  transit. Passed 354 default Retinue tests, 259 radio-hand tests, 255
  allocation-only Retinue tests, replay-only fixture tests, core-only check,
  strict Clippy and both T114/V4 resident target checks. The allocation-only
  integration run exposed an older optional-compression assertion, now gated;
  its failed attempt is retained. Actual RNS 1.5.4 Type1 captures reproduce
  byte-identically; local Type2 growth is separately labelled. See the
  [firmware receipt](../testing/receipts/firmware-ifac/README.md). Board provisioning,
  physical IFAC and identified-image acceptance remain open.
- September 27: provisioning preflight audited board settings, resident framing
  and signed management custody. Found identity-loss risk from oversized settings
  extensions and command-suffix risk from extending the fixed resident message.
  Repaired secret-bearing resident Debug output and retained the provisioning
  lifetime/confidentiality decision explicitly above.

## Sennet and Tucket current-reference work (2026-09-29)

**Status:** software slice complete; stock-radio qualification open. The owner requested current Sennet and Tucket
compatibility while awaiting the next published RNS package. RNS remains 1.5.4.
This extends the existing compatibility plan; it does not promote either sibling
to full reference parity.

**Software results:** 149 Sennet/Tucket tests, hardware-feature checks, strict
Clippy and the installed `thumbv7em-none-eabihf` library target pass. All 235
downstream radio-hand tests and five simulated capture CLI cases pass.
See the [current-reference receipt](../testing/receipts/sennet-tucket-current/README.md).

### Phase 4: protocol-core repairs and current reference pins

Tucket targets official MeshCore companion/repeater 1.17.1, revision
`d929643`. Sennet targets published Meshtastic 2.7.26.54e0d8d beta first;
2.8.0.47db0e3 alpha is a separate comparison. Preserve Sennet's documented
independent-source boundary: public prose and black-box captures, excluding
firmware implementation and schemas. MeshCore's MIT source remains an approved
comparison input with attribution.

Done when multi-byte MeshCore path hashes are forwarded and matched in their
entirety, malformed/unsupported frames cannot corrupt state or poison a later
valid packet, and Sennet broadcast-relay decisions do not masquerade as directed
routing. Exact envelope budgets must apply before outbound allocation. Run
focused host tests, strict Clippy, hardware-example compilation, and the installed
embedded target checks using `C:/t/cargo-targets/retinue`. Record reference pins,
source-derived versus observed inputs, commands, results and remaining gates.

### Phase 5: stock-radio qualification

Identify each physical peer and its installed firmware before changing settings.
Preserve settings and identities. For Sennet, record an exact stock release,
bidirectional encrypted broadcast text, maximum-length/refusal boundaries,
node-info association, duplicate behavior and restart packet-ID continuity.
For Tucket, run adverts, flood, learned routes, encrypted text, ACK, retry,
fallback and multi-repeater/multi-byte routing against identified 1.17.1 peers.
Scope policy and unsupported roles require their own cases. Store failed attempts
alongside successes. Historical captures retain their original unknown or older
pins. Software/self-peer runs do not close these physical gates.

### Findings, September 29

- `tucket::packet` accepts one-, two- and three-byte path hashes, while
  `mesh::route_recv` appends only one byte and `Node::on_frame` matches only
  the first byte. The accepted wider-path format therefore exceeds routing
  support.
- `Packet::packet_hash` hashes a second zero byte after TRACE's path-length
  metadata. The pinned MeshCore `Packet.cpp` hashes only its one-byte
  `path_len` field.
- Sennet's `ManagedFlood` currently forwards directed packets without applying
  the published distinction between broadcast flooding and directed routing.
  The retained text leaf calls that relay engine and discards generated output.
- A generic Sennet application port can require more bytes than the text-port
  envelope budget. Check the actual encoded size before allocating.

### Progress, September 29

Repository initially clean at `10a1711`; connected ports are COM6, COM7 and
COM10, with no protocol/firmware identity inferred from their port numbers.
Physical testing is pending bench availability. No firmware changed.

Implemented complete MeshCore path-prefix append/match/consume and configurable
outbound flood width, corrected TRACE hashing, and kept future versions and
transport scopes out of the unscoped Node before state mutation. Sennet now
separates broadcast relay from admitted leaf observations, checks the full
destination ID, and applies the actual port-varint envelope budget. The companion
harness can identify a peer without radio configuration and refuses an unexpected
firmware version before changing settings. Sennet captures can retain separate
producer-identification evidence; unknown producers remain unknown.

The temporary gate home remains at `C:/t/cargo-homes/retinue-wire-compat`, owned
by this Sennet/Tucket software qualification. Automatic approval review blocked
the checked cleanup command with reason "blocked by policy"; the subsequent
live-owner recheck found no matching gate process. The ordinary reusable build
target is `C:/t/cargo-targets/retinue`. No isolated target or worktree was created.

### Physical preparation, September 30

Read-only status queries identified COM6 and COM7 as running Heltec V4 Retinue
direct-PHY firmware and COM10 as running T114 Retinue direct-PHY firmware. All
three report US915 and modem personality. No current stock peer was connected.
The owner authorized use of the connected radios. COM7 is the temporary stock
oracle candidate; a complete private 16 MiB flash backup and a verified restore
route are prerequisites to any foreign firmware write. Failed/interrupted
backup attempts remain under ignored validation results.

The official MeshCore 1.17.1 V4 USB merged binary and Meshtastic 2.7.26 V4
factory binary were acquired from their pinned GitHub release assets. Only the
Meshtastic firmware binary was extracted; no schemas or implementation source
were consulted. Its untouched CLI is a process-boundary oracle.

The Sennet transmit receipt now requires the exact source, packet ID and text,
and ignores unrelated or malformed traffic. Its receive example accepts an
optional expected source/text pair and exposes received frame bytes for replay.

The August 20 physical transaction already selected COM7's carrier as V4 4.2;
its current MAC and loaded slot/sequence match that same carrier. COM6's July
28 UI receipt names V4 4.2 and its matching current MAC. Those existing board
selections were reused for the temporary packages. MeshCore's firmware model
string is `Heltec V4.3 OLED`; that build label does not replace the recorded
carrier selections. Official espflash 4.5.0 executable/archive hashes were
checked before Linkboy wrote either stock package.

Tucket passed endpoint adverts, bidirectional private text/ACKs, reciprocal
route learning and three failed direct attempts followed by fourth-attempt
flood recovery against stock `v1.17.1-d929643`, API 13, at all three flood widths.
The official repeater also passed forced one-hop source routes with complete
one-, two- and three-byte prefixes in both directions, using T114 as the
independent endpoint. The forced first hop cannot be consumed by either endpoint.
These are one-repeater results, not a multiple-repeater topology receipt.

The exact stock version exposed a harness defect: `matches_release` originally
refused its official hexadecimal build suffix. The repair accepts that suffix
for a base release, preserves exact full-build pins, and refuses malformed or
other-release strings. A final-build endpoint run and regression cover the
subsequent tighter empty/full-pin guards. Earlier repeater runs retain the
executed example's separate binary hash.

Both V4 radios received complete private 16 MiB backups before foreign writes.
COM6's full-image compressed restore timed out in FlashDeflData. Its original
image was recovered through an uncompressed esptool 5.3.1 write, with full-data
verification followed by exact independent readback of `0x3f0000..0x400000`.
The application again reports loaded slot B / sequence 9. Failure attempts
remain evidence; that manual recovery is separate from Linkboy package success.
T114 was never reflashed and again reports US915/modem, sync 2b, 906.875 MHz,
and its original slot A / sequence 84 in the retained attach transcript.

Meshtastic's untouched CLI independently reports installed firmware
`2.7.26.54e0d8d`, CLIENT role and HELTEC_V4. The temporary stock peer was set to
US/LongFast and named `Sennet Current Stock` / `SC26` for the node-association
comparison. Alpha 2.8.0 remains outside this qualification.

Sennet's first two stock attempts received no matching rebroadcast while the
stock region remained UNSET. A separate setter with five seconds of connection
settling persisted US; its readback precedes the successful encrypted broadcast,
matching RF rebroadcast and exactly one stock-client delivery. Two repetitions
of the same packet produced no duplicate client delivery in the bounded window.
The 233-byte text refusal occurs before opening the radio. Independent V4/T114
exchanges also retained consecutive packet IDs across host-session restarts.

COM7 disappeared from the USB inventory after the successful stock duplicate
test. The subsequent maximum-size attempt failed before opening that absent
port. Reverse/current maximum-size checks and restoration initially awaited
physical reconnection; the exact original full-flash backup remained intact.
COM6 restoration and T114's unchanged image are verified separately. The public
receipt preserves successes, failed attempts and this interruption without
claiming full beta, alpha or upstream-role parity.

The owner reconnected COM7 on September 30. Fresh stock metadata still identifies
`2.7.26.54e0d8d`, and US/LongFast persisted. Stock-to-Sennet encrypted text passed
with exact source/text matching, and its captured node-info resolves the sender
to `Sennet Current Stock` / `SC26`. Two 232-byte attempts encountered USB capture
interruptions; the second retains the exact reserved ID and 2115.584 ms transmit
output. Neither establishes stock acceptance. The bench now retains child output
on serial failure, with a simulated interruption regression.

COM7's exact original 16 MiB image was written uncompressed and fully verified.
Independent readback of the entire private settings tail matches the backup.
The readback tool left application diagnostics silent; an explicit watchdog
reset returned original firmware 0.0.1, slot B / sequence 5, US915/modem and
906.875 MHz. Failed diagnostics remain separate from the final passing probe.
Both V4 originals are restored, T114 was never reflashed, and 152 protocol tests
plus strict Clippy, formatting, registry and bench controls pass. The earlier
embedded checks remain valid because this follow-up changes capture tooling and
replay tests, not the protocol libraries.

### Firmware refresh, September 30

The owner authorized new firmware builds, state-preserving packages and physical
qualification. Build the ordinary V4 modem image, the optional V4 resident image,
and T114's native Retinue image from a clean source pin. Status now identifies the
build revision supplied through `RETINUE_FIRMWARE_REVISION` and the image type;
omitting that build input reports `unidentified`, never a guessed revision.
The first locked/offline builds exposed missing cache dependencies; acquired
builds linked both board families. These initial untagged builds are preparation,
not upgraded-device receipts. Preserve existing private backup/state evidence,
use accepted Linkboy plans and require returned build identity plus RF operation
before promoting updated packages. IFAC provisioning remains a separate gate.

The [firmware refresh receipt](../testing/receipts/firmware-refresh/README.md)
records clean-pin builds at `711816a`, installed ordinary/resident V4 images and
returned build identities. Both original settings pairs survived byte-identically;
the resident run changed only announce/packet reservations. Bidirectional Sennet
text passed between the upgraded V4s. The resident repeat passed three Sennet
visits, three Tucket visits including text/ACKs, and six signed home link proofs,
but its final proof after cancellation timed out. Keep this separate image partial.
The ordinary image also passed a Windows ROM-loader reinstall and returned the
same build identity. Other hosts' older receipts do not qualify these new bytes.
T114's new native image is built but unpublished: its serial loader has appeared,
and physical double-tap into the stock UF2 volume remains required before its
state-preserving installation and native-node RF qualification. Historical v51
packages and receipts remain unchanged.

The T114 UF2 drive appeared after owner reset. The verified application-only write
returned source `711816a`, original slot A / sequence 84, and US915/modem.
Native-node RF then refused oversized resource offers and echoed 1024 bytes in
both directions. Two signature-verified native announces retained one identity
and advanced across controlled soft resets. The full resident run with T114 as
peer timed out during a Tucket visit, so the host bench now exposes an explicitly
scoped three-cancellation mode. It reuses the existing cancellation checks and
peer restoration, without claiming the complete visit suite from that narrow run.
