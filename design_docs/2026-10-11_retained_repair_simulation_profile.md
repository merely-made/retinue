# Retained repair simulation prototype

**Status, updated 2026-10-12:** the historical finite host model is implemented;
an incremental host core and persistent peer provide lane C. Mere's admitted
exact-inventory/QUIC composition is published in `26eb8038`, with final
acceptance `abdf6d51`.
This document records their boundary and the historical simulation choices so
they can change with the prototype. It is not an RF wire protocol or a qualification gate. The owning sequence and later
admission/device gates are in the
[transport conformance plan](2026-11-02_transport_conformance_plan.md).

## Standards and evidence boundary

The model uses the pinned `raptorq 2.0.1` implementation for actual RFC 6330
encoding and reconstruction. It uses the existing bounded `Al=4, N=1` profile,
exact twelve-octet OTI and four-octet payload IDs. The separately qualified
OpenRQ `Al=1` corpus remains separate. Selected interoperability evidence does
not establish complete RFC 6330 conformance or reconstruction from every set
of K symbols.

Manifest and symbol authentication use Ed25519 signatures over exact authored
CBOR array encodings. The narrow encoder follows RFC 8949 core deterministic
rules: definite lengths and shortest integer/length representations; this
schema has no maps, floats or tags. The envelope, trust root and schedule are
project choices. They do not establish COSE, TLS, FLUTE, NORM or Reticulum wire
compatibility. Namespace and generation identity bind coding metadata and
payload IDs to their retained object.

The [CDDL field schema](../testing/transport-conformance/scenarios/retained-repair-v1.cddl)
declares two bodies. A manifest is `[1, 0, namespace, generation, publish,
expiry, OTI, object_digest, channels, manifest_interval, repair_floor]`; a
symbol is `[1, 1, namespace, generation, BLAKE3(manifest_body), packet]`.
The signed envelope is `[body_bytes, signature]`, with the signature over the
exact encoded body bytes. A fixed published test key is the simulation
authority; it is not a user credential. Structural validation also rejects
unknown fields/types, overlong encodings, wrong lengths and trailing bytes.
CDDL is a readable field contract here; independent general-purpose CBOR/CDDL
validation remains a separate qualification slice.

All successful reconstruction metrics mean decoded original bytes whose hash
matches the manifest. They do not mean domain authorization or store admission.
The existing operation processor remains the admission owner; the host example
below now exercises that boundary.

## Opaque original-byte handoff

The later [`retained-repair` host library](../crates/retained-repair/README.md)
accepts publication after startup and retains at most four exact opaque
originals of 1–16,384 bytes, with a 64 KiB original-payload ceiling. It splits
frame preparation from sink acceptance, preserves deferred bytes/ESI/token,
and reports expiry invalidation. Its dynamic collector learns from signed
manifests, authenticates before decoder construction, and returns actual
length/digest-checked decoder bytes. It uses explicit caller authority and
namespace, and the authored T=128/Z=1/N=1/Al=4 profile. Codec workspaces and
returned bytes need additional heap; the payload cap is not a heap measurement.

The [persistent `--retained-peer` API](../testing/transport-conformance/README.md#incremental-retained-peer)
exposes that core for the Mere owner's exact-inventory/ingress composition.
Its NDJSON commands are separately bounded by line size, command count,
lifetime bytes and wall-clock deadlines. Logical clock advancement/expiry is
caller-owned and ends at slot 8192. A prepared-frame token acknowledges local
sink custody. The bounded `recover` helper accepts/drop source frames and
delivers repair frames through the real collector, with prior decoder state
reported; it does not substitute producer originals for decoder output.
The selected process experiment lives once at
[`retained-live-20261012/exploration.json`](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/transport-conformance/results/retained-live-20261012/exploration.json).
These volatile host components add no radio, persistence or application
admission owner. The following `generation.original_hex` and checkpoint API
continue to describe the historical scenario mode.

The later A/C/D composition and its evidence live once in the
[Mere-owned ingress guide](https://github.com/merely-made/mere/blob/abdf6d51556dd175f57440199a8335e37bb19f1a/crates/mesh/mesh/examples/inventory_ingress/README.md).
An admitted exact snapshot selects missing retained generations; actual host
RaptorQ output then crosses real QUIC into fresh signed decoding and the existing
mesh owner. The resident mounts on its existing endpoint/store, with inventory
and submission sharing two active calls/four pending reservations. The receipt
includes unchanged signed bytes and separate-process durable reopen. Coded
frames remain host loopback before QUIC; MCU/RF/worker/native-UI qualification
stays separate. The receipt preserves the intermittent full-transport gossip
failure rather than claiming consistently green whole-transport tests.

An optional `generation.original_hex` lets the owning application supply actual
original bytes instead of the deterministic synthetic fixture. The decoded hex
must match `object_octets` and remains bounded to 64 KiB including its wrapper.
After actual codec reconstruction, length and signed-manifest digest checks,
the receiver report exposes those exact bytes as `recovered_object_hex`.
Both fields are omitted for the existing synthetic experiments. Potential
returned external bytes across all receivers are capped at 256 KiB raw;
the JSON process request/response still has its 4 MiB ceiling. Peer responses
also identify their compiled implementation and dependency/source fingerprints.

Retinue leaves these bytes opaque. The Mere-owned `retained_receive` example
uses an existing CBOR-encoded Stickleback `DropRecord::Operation`, carrying
the original signed p2panda header and inline body. Mere freshly decodes the
recovered record/header and sends the operation to
`MeshStore::accept_outcome(expected_mesh, operation)`, which invokes the current
mesh policy and Stickleback operation processor before storage mutation.
The receiver supplies its expected mesh independently of the carried object.
It does not use the operation's own addressed mesh as an admission decision.

The p2panda operation ID hashes signed header bytes. The retained-object digest
hashes the whole encoded record; manifest hashes and generation IDs identify
other parts of delivery. Preserve each identity for its own purpose. Compare
exact record, header and body bytes when observing preservation, because typed
operation equality compares only operation IDs. A valid outer envelope can
carry an invalid inner operation, so header authentication, payload commitments
and domain policy remain checks performed at their existing owners.

The selected Mere experiment records a 1,015-byte V2 mesh job operation record
with eight source symbols. Four modeled radios recover it at slot 2; one radio
joining at slot 6 recovers it at slot 16. The two arrivals preserve exact record,
header and body bytes and the operation ID. The existing owner inserts one
operation/job and reports the later arrival as a duplicate. Three separately
delivered records with a wrong mesh, changed body or changed signature are
refused without changing that durable effect. Closing and reopening the redb
store in process preserves the bytes and posted job. The source-bound report
and reproduction guide live once at
`mere/crates/mesh/mesh/examples/retained_receive/receive-exploration-20261011.json`
and `mere/crates/mesh/mesh/examples/retained_receive/README.md`. This does not
transfer job blobs or execute a worker, and it uses modeled lanes rather than
a live carrier.

The subsequent real-carrier experiment adds the example's `--carrier quic`
mode. Exact recovered bytes traverse production p2panda-net/iroh streams with
loopback routes before the same receiver-owned decode/admission callback.
Stickleback's generic one-object stream requires a bounded length, complete
payload and clean FIN within one deadline. Protocol and authenticated carrier
peer checks precede reading; the expected mesh remains a receiver choice and
the original author remains the signed header's author. A bounded result
acknowledgement follows the existing owner's decision. The report distinguishes
that decision from sender acknowledgement receipt.

The finite example caps 16 KiB objects, 2 KiB acknowledgements, 12 attempted
sessions, 64 KiB request frames and 128 KiB reserved request/reply frames, with
one in-flight object and explicit I/O/exchange/campaign deadlines. Oversized,
truncated and stalled stream attempts consume the budget and stop before the
owner; valid subsequent streams still reach admission. Source-bound observations
live once in
`mere/crates/mesh/mesh/examples/retained_receive/quic-handoff-exploration-20261011.json`.
Actual local QUIC does not turn modeled radio slots into physical airtime or
goodput. The production transport's global accepted-session queue is a
separate host service admission concern.
Actual-path observation is limited to two seconds within the exchange/campaign
deadlines; empty actor snapshots may be retried, while an observed active route
outside loopback fails immediately. The Mere guide owns the observation details
and the intermediate undiagnostic failure's unresolved classification.

## Time, capacity and retention

Time is an integer slot index. The modeled transmitter has parallel capacity
to offer one bounded frame on every channel in a slot. The four-channel
experiments therefore assume four simultaneous transmitting lanes as well as
the compared receiver radios. Every manifest repeat, source frame, repair frame
and authentication byte contributes to the modeled transmission accounting.
The slot octet budget is a declared model input. No slot-to-seconds, modulation,
range, interference, duty-cycle or energy conversion is assumed.
Channels are ideal independent lanes. Multiple modeled radios can collect
simultaneous lanes without adjacent-channel leakage or shared-antenna costs.
Those assumptions must be revisited for an actual radio implementation.

Each generation declares a namespace-local ID, publication slot, expiry slot,
object length and coding metadata. Retained originals regenerate fresh repair
symbols while a generation remains live. A repeated symbol ID adds no new
information. Fresh source work has priority only after the declared repair
floor is reserved. Repair work rotates among live generations, and spare
fresh capacity may carry repairs. Floor zero is permitted as an explicit
counterfactual to test starvation; the proposed retained-repair schedule uses
a positive floor. Anchor manifests consume channel zero first; the available
data lanes then receive up to the requested repair floor. A one-channel
manifest slot has no remaining data lane. Expiry terminates
transmission/reception eligibility.

Repeated anchor-channel manifests let receivers learn a generation without
receiving its first announcement. Receivers already know the channel plan,
manifest cadence and common slot clock; acquisition of an unknown beacon or
clock is future prototype work. Full-radio receivers remain assigned to all
channels. Partial receivers hop a contiguous channel window and prefer channel
zero on predicted manifest slots. Join time, phase, dwell, sleep, drift and
nominal dwell guards reduce collection opportunities. The anchor override does
not model a synthesizer's actual settle transition. Radio capture is assessed
at transmission; a bounded reorder queue and duplication then affect delivery
without inventing extra radio capture or transmitter airtime. Seeded erasure
and burst erasure are repeatable impairment inputs, not RF measurements.

Catch-up claims name the finite workload, horizon and retention lifetime.
Reports retain cumulative produced and completed object bytes, including
unmet demand from expired work. An incomplete expired generation remains
unmet; historical successful completions survive expiry. A source rate
exceeding a receiver's one-symbol-per-opportunity
upper bound cannot be made recoverable by retaining repairs. A bounded live
queue alone is not evidence of stable demand or eventual recovery.

## Resource and trust bounds

Scenario validation runs before codec construction. The model caps slots,
channels, receivers, generations, live generations, retained originals,
frame size, received packets, decode work, reorder delay and peer/checkpoint
bytes. Exact limits are serialized with the campaign's selected profile.
Authentication checks precede decoder allocation and decode work. Duplicate,
unknown-generation, expired, malformed, unauthenticated and over-budget inputs
have distinct observations. These host bounds do not qualify an MCU heap,
interrupt budget or radio coexistence profile.

## Checkpoint and process contract

A checkpoint retains the next slot, transmitter cursors and originals,
round-robin position, pending deliveries, receiver manifests and collected
signed packets, counters and trace state. Codec structures are reconstructed
from validated bounded state. Replay work is reported separately from the
original run's decode work. Resume begins at the saved next slot.

The JSON process interface is a bounded test harness, not a radio transport.
It supports run, checkpoint and resume operations. The retained finite-burst
experiment serializes and restores a real partial checkpoint in process, then
compares terminal and trace hashes with the uninterrupted run. A separate small
manual smoke used three fresh peer processes, restored seven pending deliveries
at next slot six, and matched the normalized report and hashes; that observation
was stdout-only and has no retained receipt. The frontier report accurately
records its own process-resume scope as unexercised. Broad restart qualification
is deferred with the other stable-contract evaluation.

## Experiments and publication

Human-authored TOML scenarios declare experiment inputs. A machine-written
JSON report retains their assumptions, source identity, trace digests,
reconstruction observations and capacity bounds. Compare single-/multi-radio
collection, positive-/zero-floor scheduling and finite bursts/continued
overload. A partial-state resume can expose retention/state design mistakes.
Incomplete and expired delivery remain visible. These comparisons guide the
next implementation choice; they do not freeze the interim design into a
conformance or regression contract. Keep smoke checks small and postpone broad
qualification until the stack's shape is stable.

The selected [exploration report](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/transport-conformance/results/retained-repair-exploration-20261011.json)
records four experiments on macOS aarch64. These observations describe the
checked-in inputs, not an invariant every future scheduler must preserve:

| Design question | Current observation |
|---|---|
| Finite production: can one radio catch up? | Four radios reconstruct the 4,097- and 1,281-byte objects at slots 9 and 33; one radio at 64 and 53. Both finish with zero unmet bytes within the 160-slot lifetime. |
| Does fresh priority starve retained work? | A receiver joining at slot 8 gets zero symbols of the older object with floor zero. Floor one supplies 11 unique symbols and reconstructs it at slot 43. |
| What does the floor cost? | The paired 96-slot experiments transmit the same 364 data frames and 103,305 wire bytes. Fresh frames change from 345 to 257; repairs from 19 to 107. |
| Can repairs overcome continued overload? | 1,024 offered source symbols exceed a one-radio ideal upper bound of 256 opportunities. All 131,072 produced object bytes remain unmet for that receiver, including 106,496 bytes expired incomplete. Four radios complete eight objects (32,768 bytes), leaving 98,304 unmet bytes. |
| Does saved partial state resume? | The finite checkpoint after slot 25 restores matching terminal and trace hashes; rebuilding codec state requires 57 separately counted replay calls. |

The early shape is a shared retained-object sender with priority and repair
policy, explicit receiver opportunities, and authenticated generation envelopes.
The historical owner-local receive example demonstrates one recovered signed
object crossing bounded real local QUIC into the existing operation processor.
The later published A/C/D slice mounts bounded admitted ingress in the owning
resident and selects missing retained originals through exact inventory.
Other carriers, physical scheduling and wider qualification remain separate.

Run and record the campaign locally. GitHub Actions is disabled in Mere and
Retinue by owner request; this work uses direct commits and pushes to `main`,
without workflow additions or PRs.

Primary specifications: [RFC 6330](https://www.rfc-editor.org/rfc/rfc6330),
[verified erratum 5548](https://www.rfc-editor.org/errata/eid5548),
[RFC 8032](https://www.rfc-editor.org/rfc/rfc8032), and
[RFC 8949 core deterministic encoding](https://www.rfc-editor.org/rfc/rfc8949#section-4.2.1).
