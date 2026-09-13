# Murmuration controller implementation plan

**Status, 2026-09-11: MC0–MC2, MC3a packet adapters, MC3b host-retained
sessions, MC3c resource drain, MC3d explicit loss and MC3e board PHY deadlines
complete at their stated scope; full MC3 remains open.** The user authorized
implementation planning, Luna/Terra work, then real-radio integration and guarded
flashing. Software and [physical receipts](2026-09-10_murmuration_physical_receipt.md)
cover the controller and host-driven adapters. Product authority is the
[current murmuration direction](2026-08-09_channel_murmuration.md#current-direction-coordinated-runtime-personalities).

## Ownership and scope

Tulle owns the personality coordination contract. Protocol implementations own
local session state and describe whether interruption is resumable, busy or
requires explicitly permitted session loss. The embedded radio owner remains in
radio-hand. This controller makes decisions and consumes explicit completion
reports; it never touches hardware or receives arbitrary protocol packets.

Keep the controller dependency-free within `crates/tulle`: caller-supplied
monotonic milliseconds, bounded values, no async runtime or new registry/service.
Use a small installed-personality set and explicit home/pin configuration.
Retinue is the preferred home when supported, but construction validates the
user's chosen home. Changing durable configuration, serial command formats,
real adapters, observation wire formats and UI is outside this slice.

Coverage admission is supplied by the caller under an explicit policy: allow a
known gap or require coverage through the return budget. The existing
radio-hand scheduler owns keeper maps and liveness. Do not copy that scheduler
or pull embedded dependencies into Tulle. A later bridge must produce coverage
inputs from authenticated, locally admitted evidence and preserve immediate
expiry. A coverage input is not an RF receipt or authentication proof.

## Phases and done conditions

### MC0: Controller contract and implementation

Terra owns `crates/tulle/src/personality.rs` and the minimal module export in
`crates/tulle/src/lib.rs`. Use plain API names; no new crate is required.

Model explicit excursion, completion/cancellation and return, with stable home,
optional pin, configured finite duration/return budget, and pause outcomes.
A busy adapter may defer only within a bound. Session-discarding interruption
requires explicit permission. Preserve caller-owned fake-adapter state across a
resumable pause. No success report may precede acknowledgement from the radio
owner, and stale or wrong transition acknowledgements must not complete a new
transition. Time regression and overflow are visible errors with safe state.

**Done:** the model refuses unsupported/pinned/busy/unauthorized work; caps
excursions including pending transitions; requires the declared coverage bound;
requests return on cancellation, expiry or lost required coverage; reports
uncertain/overdue restoration as recovery required; and never changes the saved
home implicitly. Its public docs explain trusted caller inputs and absence of
hardware guarantees. There is no packet-to-switch input.

### MC1: Independent behavioral acceptance

Luna first inspects the existing scheduler and board interruption contract,
then reviews the proposed API. After the API is available, Luna owns
`crates/tulle/tests/personality.rs` and uses two stateful fake adapters.

**Done:** externally observable scenarios cover normal excursion/return,
persistent local state, pin/unsupported refusal, bounded busy deferral,
explicitly permitted session loss, cancellation before and after activation,
finite deadline including stalled activation, optional gaps versus required
coverage and its expiry, incorrect/stale acknowledgements, clock regression,
overflow, and restoration failure. Tests assert independent expected behavior,
not duplicate the controller algorithm. Fake tests make no remote-session or
physical compatibility claims.

### MC2: Integration review and receipts

The parent agent owns plans/index/crate README/validation integration and reviews the public
API and test outcomes. Run focused `tulle` tests, all-feature library checking,
strict Clippy for touched targets, scoped rustfmt and rustdoc as appropriate.
Register new validation assets if required by the repository registry. Preserve
the dirty Signalman lane and the prior four doc edits. Work on existing main;
no branch or firmware flashing is part of this pass.

**Done:** tests and static checks pass, findings and results are recorded here,
and the architecture docs distinguish the implemented controller from missing
real-adapter/board integration. Any review finding is fixed and rechecked before
calling the slice complete.

### MC3: Real-adapter and board integration (follow-on)

Adapt two admitted implementations to a shared pause/resume contract and connect
one board loop through its radio owner. Reuse the existing interruption lifecycle
and shared observation types. Decide actual adapter availability and firmware
memory cost from builds, not the installed host crates.

**Done:** exact-build physical receipts demonstrate normal switches without
reboot, busy RX/TX handling, preserved or explicitly ended state, bounded return,
missed home traffic, cancellation and failed restoration. Two-listener coverage
is a separate measured gate. MC0–MC2 do not close LE/CM or compatibility gates.

## Findings

- 2026-09-10: `tulle/src/lib.rs` is the shared interface beneath Retinue, Sennet
  and Tucket; it has no personality controller at the start of this pass.
- 2026-09-10: `radio-hand/src/scheduler.rs` already owns static keeper admission,
  lease deadlines and acknowledged listening restoration. It has no firmware
  consumer. Reusing its caller boundary avoids duplicating peer policy here.
- 2026-09-10: `radio-hand/src/channel.rs` constructs boot-selected Modem, Node or
  Rnode; host-session `stop` and parser `at_boundary` do not establish safe
  protocol suspension. `V4RadioOwner` has completed-event and restore/reset
  machinery, which must remain the embedded hardware authority.

## Progress

- 2026-09-10: Plan established before implementation. Terra implementation and
  Luna independent acceptance/review lanes assigned; parent retains integration.
- 2026-09-10: Baseline `cargo test -p tulle --locked --offline` passed 41 unit
  and 5 integration tests. `python validation/run.py verify` passed: 20 Cargo
  manifests, 81 assets, 14 suites at base `13f46b6`. Cargo integration tests are
  covered by the existing host-workspace suite; no registry expansion is needed.
- 2026-09-10: First parent review found extendable busy deferrals, missing
  coverage refresh after deferral, uncapped return acknowledgements and gaps in
  cancellation/overdue reporting. Terra is correcting these; Luna owns regression
  scenarios. No controller acceptance is claimed from the initial draft.
- 2026-09-10: Both implementation and independent tests are present. Review
  added successful-completion return, shortened-coverage admission during
  activation, and preservation of latched return obligations if coverage recovers.
  The constructor assumes hardware-confirmed home; configuration is immutable
  and a pin equals the selected home. These are explicit limits of this API,
  not live settings or firmware integration.

### MC0–MC2 result, 2026-09-10

Terra implemented [`tulle::personality`](../crates/tulle/src/personality.rs);
Luna authored [21 independent acceptance scenarios](../crates/tulle/tests/personality.rs).
Parent review corrected the deadline and return-obligation findings above and
consolidated the package documentation. The model preserves caller-owned state
across fake-adapter pause/resume; it does not establish remote-session survival.
Acknowledgements correlate a controller-local transition ID; the trusted caller
must verify the actual commanded target and hardware outcome. Initialization
assumes confirmed home, and recovery requires external owner intervention.

Validation from the shared working tree based on `13f46b6` (isolated build output
`C:/t/murmuration-target`):

- `cargo test -p tulle --all-features --locked --offline`: **55 unit + 21
  personality acceptance + 5 RNode capture tests passed**, zero failures.
- `cargo clippy -p tulle --all-targets --all-features --locked --offline -- -D warnings`:
  passed, including final source and acceptance tests.
- `cargo doc -p tulle --all-features --no-deps --locked --offline` with
  `RUSTDOCFLAGS=-D warnings`: passed.
- Scoped Rust 2024 rustfmt, `git diff --check`, local Markdown link-target checks
  and `python validation/run.py verify`: passed.

These are working-tree software results, not clean-revision release evidence.
The unrelated Signalman changes were preserved. No firmware was built or flashed,
and no LE/CM, hardware or protocol-compatibility gate was closed.

MC3 remains the next engineering proof: choose two actually available adapters,
implement their pause/resume and interruption reporting, connect one radio owner,
and obtain the bounded-switch and missed-traffic receipts described above.
The controller intentionally defines neither a keeper-policy bridge nor a
serialized management command; those integrations must preserve the proven
admission and acknowledgement distinctions.

### MC3a authorized physical slice, 2026-09-10

**Status: passed at the bounded scope below.** The user authorized real-radio integration, necessary
builds/guarded flashing, and bounded physical acceptance. Live inventory found
COM7 V4 `44:1B:F6:6A:FA:64` and COM10 `TULLE-T114-01`, both answering direct-PHY
status, sync and observation queries. The other V4 `44:1B:F6:6A:FB:28` initially
was absent, later enumerated as COM6, and was left unused. No competing active
serial/firmware process was found. Two available
boards are sufficient for this slice; existing firmware already supports runtime
configuration. A discovered USB packet-boundary defect subsequently required a
narrow V4 host-writer change and a guarded firmware installation.

Terra delivered the persistent-link controller bridge and V4 USB fix. Luna
reviewed hardware seams and the USB failure; parent completed and reviewed the
Retinue/Sennet example, guarded restoration runner and all physical access.
The bench uses the established 906.875 MHz, BW250, SF11,
CR5, preamble16 profiles at 7 dBm; sync 0x12 home and 0x2b excursion. Test actual
Retinue plain packet and Sennet encrypted text codecs, explicit excursions,
acknowledged retune/return, counted home-traffic misses, preserved caller packet
state, deadline return and pin refusal where feasible.

MC3a is host-driven packet-adapter integration through existing firmware radio
ownership. It does not close the broader MC3 requirement for full protocol-session
pause/resume or autonomous on-board switching. Peer custody/coverage guarantees
and arbitrary third-party firmware remain separate. Record boot nonces around
the run and restore captured runtime profiles in finally; do not treat generic
firmware version banners as read-back binary hashes. MeshChat X inspection is
optional and must not compete for these serial ports.

### MC3a result

Two qualified runs on COM7 V4 and COM10 T114 passed 32 exact RF receives:
24 Retinue plain data and eight encrypted Sennet texts. Each run performed two
normal excursion/return cycles on each board, a one-second deadline excursion
on the V4, and pin refusal over the same serial session. Ten return configuration
acknowledgements took 0.883–7.362 ms. Those timings measure command completion;
subsequent decoded packets separately prove usable reception. Eight deliberately
injected, TX-acknowledged home packets were not observed at the away V4. There
was no independent on-air witness for these missed receive opportunities.

The V4's exact 64-byte USB receive envelope initially stalled. Adding 100 ms of
settling did not help; shortening the payload did. The USB writer now ends exact
64-byte multiples with a short packet while preserving the byte stream. The
qualified runs cover 64- and 128-byte receive envelopes in both RF directions.
The V4 build/flash artifacts and failed experiments are retained in the receipt.
Both qualified runs share unchanged per-board boot nonces. Original runtime
profiles and status were restored after each run; the flash experiment also
verified the 32 KiB settings/control region byte-for-byte against a full backup.

Final Tulle tests passed **59 unit + 21 independent acceptance + 5 capture**
tests. Strict all-feature Tulle Clippy, example Clippy, strict Rustdoc and the
validation registry passed. The V4 release build passed with two existing
sleep-proof dead-code warnings. Further checks and exact commands are recorded
in the physical receipt.

Still open: full stateful protocol adapters, autonomous board scheduling,
physical busy/cancel/failure recovery, authenticated keeper coverage and exact
T114 firmware provenance. This slice closes none of those larger LE/CM gates.

### MC3b: retained Retinue sessions, authorized 2026-09-11

**Status: passed at host-retained scope.** The qualified V4/T114 run retained one
real Retinue link through normal excursions, cancellation, deadline return and
pin refusal, with ten encrypted messages and RF-observed Sennet duplicate
recognition. Both boards retained boot IDs and restored profiles. The pure Node
assessment and nine independent tests passed, as did the full Retinue test suite,
strict Clippy and Rustdoc. See the [MC3b receipt](2026-09-11_murmuration_session_receipt.md)
for source hashes, preliminary versus qualified runs and remaining limits.

The next slice keeps real `retinue::node::Node` instances alive across host-driven
excursions. Add a side-effect-free protocol-owned pause assessment: pending
handshakes, active resources and transit obligations must be visible; an idle
link is locally retainable only within its ordinary expiry. The host also owns
its outstanding action queue and radio-operation boundary. Do not freeze clocks
or promise that an uncooperative remote peer retains its session.

Terra owns the Node assessment; Luna independently tests real session state and
busy/expiry behavior. Parent owns physical harness integration, review and
receipts. Done: establish a link through real RF, exchange encrypted data before
and after an acknowledged excursion without a new handshake, retain Sennet
channel/packet and duplicate-detection state, refuse a busy local operation,
and measure cancellation return. Preserve MC3a artifacts and board profiles.
Autonomous firmware, full resource interruption and keeper coverage remain
separate gates. A core-only controller extraction must have a real firmware
consumer before it becomes an implementation lane.

### MC3c: drain active resources before departure, 2026-09-11

**Status: passed.** MC0–MC3b committed as `95393fb`. Exercise an active
Retinue resource on the existing physical pair: departure must remain refused
while either endpoint owes transfer work, including a sender waiting for proof
after receiver delivery. Drain the actual RF action queue, verify exact resource
delivery and cleared obligations, then admit an excursion and retain the link.
Terra owns the session harness method; Luna owns independent completion and
lost-proof regressions. Parent owns integration, physical access and receipts.

Done: software regressions and a guarded physical run prove refusal, completion,
subsequent admitted departure and encrypted same-link return. Reuse the session
receipt for this extension. No forced session discard or arbitrary retry time
is introduced. Autonomous scheduling, active-resource interruption with explicit
loss and physical failed-restoration recovery remain separate follow-ons.

Review also restored the `alloc` requirement on the new `node_pause` test target.
The allocation-free library/test configuration passes 23 tests. Resource part
size already follows negotiated link MTU; this slice needs no wire-format change.

Deferred finding from `Node::poll`: idle-link expiry removes the link but does
not itself purge associated resource entries. Keep pause refusal conservative;
do not advertise timeout-based resource cleanup or invent a retry deadline.
Explicit interruption/cleanup must define caller-visible loss and queued-action
ownership before changing that behavior.

Qualified physical run `mc3c-1` passed: a 350-byte resource in two parts, three
busy-stage refusals, proof-dependent completion, subsequent admitted excursions
and ten encrypted messages on the retained link. Both boot IDs and original
profiles were preserved. Eleven pause tests, strict example/test Clippy, scoped
formatting and registry verification passed. Exact evidence is appended to the
[session receipt](2026-09-11_murmuration_session_receipt.md#mc3c-extension-resource-drain-before-departure).

### MC3d / MC3e: explicit interruption and board deadlines, 2026-09-11

**Status: passed at the scope below, user authorized both lanes.** MC3d adds explicit permission
for Node-wide session loss, a complete bounded report of discarded links,
handshakes, resources and transit bridges, and best-effort encrypted close packets.
The caller owns cancellation of previously issued actions and the physical I/O
boundary. No close packet or local state change proves remote acknowledgement.
Identity, freshness history and IV sequence must survive interruption.

MC3e integrates a configured one-shot PHY excursion into the V4 owner loop with
an on-board return deadline, without waiting for a subsequent host command.
The image is currently allocation-free direct PHY, not a Retinue/Sennet protocol
host. Keep that distinction in every receipt. Reuse the controller through a
core-only shared home if needed; retain one radio owner, completed-frame
handling, pin/admission checks and explicit recovery on uncertain restoration.

Parent owns Node implementation, integration, docs and hardware. Terra owns
the board lane after its seam review; Luna owns independent interruption tests.
Done: focused software tests and exact-build V4/T114 physical evidence for
explicit session loss, fresh re-establishment, and board-timed return while
host scheduling is absent. Preserve profile/identity settings and existing WIP.

MC3d software now reports every affected ID independently of action-queue
capacity and never mutates on denied permission. A real transit test proves
old bridged traffic stops forwarding after loss. Sixteen pause tests pass.
Physical run `mc3d-1` cancels one held resource request, reports local loss,
delivers an encrypted close over RF, observes peer LinkDown/resource cleanup,
and completes a fresh handshake on the same Nodes. It records 41 exact RF
receives and twelve encrypted application messages across old/new sessions.

MC3e moved the unchanged controller implementation to no_std `selvage`, with
Tulle re-exporting it. V4 USB accepts a volatile 24-byte `CMD_EXCURSION` (0x06),
captures current home, and returns using its own clock. The controller permits
explicit gaps for this stateless PHY consumer; there are no resident protocol
sessions in this image. Duration is caller-selected up to 60 seconds. Return
budget is two seconds, individual transition bound 1.5 seconds. Provisional
configuration, non-quiet entry, invalid duration/profile and unsupported host
builds refuse admission. Active work fences competing USB commands and records
RF locally without waiting for host output. A completed level-high radio IRQ
is collected ahead of a ready timer. Uncertain transition or overdue recovery
resets and is a failed excursion, not a successful reboot-free return.

Physical `mc3e-1` accepted a fragmented two-second request, then returned 20 ms
after its board deadline during 3.5 seconds without host writes or reads. Two
subsequent RF packets prove restored reception in both directions. The guarded
V4 installation preserved the 32 KiB durable region byte-for-byte. The same V4
boot ID spans MC3e and MC3d. Release USB build and UART check passed; 29 Selvage
unit, seven wire, and 85 Tulle tests passed, plus strict Clippy/Rustdoc.

Still open: resident firmware Retinue/Sennet adapters, recurring schedule/UI
settings, firmware pin controls, authenticated coverage, power-cut/driver-fault
injection and adversarial RX/host stress. The ordinary successful physical
returns do not qualify those fault paths. See the extended
[session receipt](2026-09-11_murmuration_session_receipt.md).

### MC4: resident protocol runtime, design continuation 2026-09-11

**Status, 2026-09-13: MC4a embedded cores, explicit capacity controls and
build/host memory evidence implemented. Target runtime memory qualification and
later steps remain open.**
MC3e proves an autonomous PHY timer. MC4 must put actual protocol state and
packet handling on the board. Its target includes Retinue, Sennet and Tucket;
availability is declared per firmware build and protocol capability.

#### Ownership

| Component | Responsibility |
| --- | --- |
| Retinue | Reticulum identity, links, resources, routing, protocol timers, pause assessment and explicit interruption reports. |
| Sennet | Channel keys, packet identity allocation, packet validation, duplicate history and supported messaging/forwarding behavior. Compose its currently separate components into retained runtime state. |
| Tucket | Identity, contacts, routes, duplicate history, pending texts, ACK matching and retry policy. Its caller-owned pending sends must participate in suspension assessment. |
| Tulle | Host radio access and the public coordination surface used by host applications. It does not own sibling protocol state. |
| Selvage | Shared allocation-free PHY/wire/observation types and the single controller implementation, re-exported by Tulle. Keep protocol implementations and board drivers out. |
| radio-hand and board owner | Embedded runtime integration, pending physical actions, exclusive radio access, airtime enforcement, completed RX/TX handling and confirmed restoration. |

An installed personality is a configured protocol instance: implementation,
identity/channel configuration, PHY profile and supported capabilities. A PHY
profile alone does not identify one. Multiple instances of the same protocol
must remain representable, with separate state where required. Retinue is a
configurable home choice, not the supervisor of its siblings.

#### Runtime contract

Keep protocol objects alive while another instance uses the radio. Introduce a
small common lifecycle vocabulary at the shared core boundary, using the existing
controller outcomes. Each implementation supplies its own assessment and loss
details; do not flatten every protocol into Retinue links. The embedded shell
owns composition and execution, and supplies time, entropy, persistence and TX
results. It must include caller-held pending work when assessing departure.

Switching proceeds through explicit steps: assess protocol and queued work;
drain or explicitly account for interruption; collect completed physical events;
suspend the outgoing instance; apply the target profile and arm RX through the
owner; then activate the target instance. Acknowledgement identifies the actual
instance and completed transition. Frames and queued actions carry instance and
activation identifiers so an old action cannot transmit under a new profile.
Returning follows the same physical ownership boundary.

Elapsed time continues during absence. Expire protocol obligations at their
normal deadlines, report missed work, and revalidate retained state before
resuming transmission. Local state retention does not guarantee remote retention.
An away operation must fit the remaining dwell and restoration budget or be
deferred/refused. Forced return cannot silently discard its pending work.

#### Implementation order and done conditions

1. **Embedded cores and capacity.** Make Sennet and Tucket protocol code usable
   with `no_std + alloc`, retaining optional host I/O. Audit dependency features,
   collection growth, packet/resource sizes and failure behavior. Retinue already
   has an alloc core; the present V4 image has no allocator. Select explicit
   board capacities and allocator/storage arrangements from target builds and
   memory measurements. Done: all three cores compile for the target, existing
   wire fixtures pass, and bounded resident state plus peak working memory has
   a documented budget. Flash size alone is insufficient evidence.
2. **Actual retained instances.** Compose Sennet channel, packet IDs and duplicate
   state; include Tucket pending texts and retry deadlines alongside its Node;
   reuse Retinue assessment/interruption. Keep identities and counters across
   excursions. Persist or reserve Sennet IDs before use so reset cannot reuse a
   key/source/packet-ID combination; retain Retinue's durable announce reservation.
   Done: tests exercise each real implementation across absence, expiry, busy
   deferral and permitted loss, including external action queues.
3. **One embedded runtime.** Integrate these instances through radio-hand and
   the V4 owner, replacing exclusive boot-channel loop ownership where needed.
   Advertise installed capabilities; expose home, pin and explicit bounded
   excursion settings. Reuse the existing controller, with board-owned return
   deadlines. Done: target builds and runtime tests prove dispatch isolation,
   stale-action refusal, confirmed activation/return and explicit recovery.
4. **Physical protocol proof.** First qualify Retinue home with a Sennet visit,
   then add Tucket through the same lifecycle. Exercise actual board-resident
   packet handling while host scheduling is absent. Done: exact-build RF evidence
   shows retained or explicitly ended protocol state, valid traffic before/after,
   busy departure, cancellation, deadline return and failed-restoration behavior.
   Record memory use, boot continuity and missed home receive opportunities.
5. **Cooperative schedules.** Once local switching works, add configured recurring
   schedules and optional authenticated peer coverage/rendezvous using the existing
   keeper/observation machinery. Done: multi-board measurements distinguish promised
   coverage from actual captures and misses. This is separate from a single-board
   successful excursion.

These steps do not require equal protocol feature breadth. Initial Sennet
capabilities must name the supported messaging behavior, rather than imply a
complete mesh participant. Preserve its current provenance boundary throughout.
No automatic packet-triggered switching or implicit cross-mesh forwarding is
introduced. The existing raw-PHY excursion remains a separately named capability.

#### MC4a execution, 2026-09-12

The user authorized orchestration of the embedded-core/capacity slice. Separate
Terra lanes own Sennet and Tucket portability, configurable retained-state
limits, input bounds and regression tests. Parent owns integration, Retinue
capacity review, target compilation and memory evidence. Existing host tools
must remain usable. Use the existing main checkout and preserve unrelated work.
This slice does not install firmware or claim resident protocol switching.

#### MC4a result, 2026-09-13

Both Terra lanes delivered `no_std + alloc` protocol cores and capacity controls.
Sennet now bounds directory fields/count, stream buffering with explicit retry
on output backpressure, and packet/application encoding. Tucket bounds contacts,
duplicate history and caller-held pending texts, and provides borrowed packet
validation and checked text/path/advert entry points. Independent review caught
allocation before size checks and public retry-policy values that could exceed
the fixed ACK array; both were corrected. Sennet errors implement `core::error::Error`
so existing host callers retain ordinary `?` error conversion.

Parent added optional Retinue Node payload limits, preserving existing defaults:
ingress size, local announce data, link payloads, outbound resource bytes and
inbound part count. Refused inputs leave protocol state intact. Initial resource
hashmaps cannot exceed their advertised count, and later HMUs cannot grow past
that count. Raw input must still be bounded before Packet decoding. Compression
is disabled for this target graph; it needs its own expansion limit before use.

The [capacity fixture](../testing/protocol-capacity/README.md) compiles all three
cores together for Xtensa and uses concrete, selectable capacities. It exercises
real protocol objects, full directories, duplicate churn, pending Tucket texts,
a Retinue link and a 1 KiB resource transfer. Exact source/artifact hashes and
checks are in its [receipt](../testing/protocol-capacity/receipt.json).

| Measurement | Result | Scope |
| --- | --- | --- |
| Combined resident inline layout | 3,128 bytes | Xtensa compiler layout; heap excluded |
| Retained allocations | 11,183 bytes | 64-bit host requested bytes |
| Peak allocations | 18,380 bytes | One host workload, including synthetic peer objects |
| Allocations remaining after drop | 0 bytes | Same host workload |
| Candidate fixed heap | 65,536 bytes | Linked V4 example reservation |
| Radio/runtime plus queue/state reserve | 65,536 + 16,384 bytes | Linked BSS placeholders |
| Capacity-image data + BSS | 151,912 bytes | Includes heap, placeholders and actual inline objects |
| Linker stack region remaining | 184,132 bytes | Available region, not measured stack usage |

The standalone V4 capacity image links the workload and fixed LLFF heap; it was
not executed or installed. It does not run the radio loop. Its reservations
show space for a candidate integration budget, not sufficient memory under all
traffic or target allocator fragmentation. The ordinary V4 USB firmware also
builds and remains a separate allocation-free modem image.

Validation passed: Sennet 57 tests, Tucket 61 tests, Retinue 208 default-feature
library tests (171 in alloc-only configuration) and 20 pause/capacity tests;
strict scoped Clippy, all-feature Rustdoc, host application/example checks,
combined Xtensa library compilation, both V4 release links, formatting and the
registry (21 manifests, 83 assets, 15 suites). An extra alloc-only strict Retinue
Rustdoc run exposed existing links to disabled Endpoint/TCP/connect items; the
normal all-feature documentation check passes. The ordinary V4 image retains
two pre-existing dead-code warnings. Concurrent `endpoint.rs` edits are outside
this slice and preserved.

Next: compose these real components into retained instances with protocol-owned
pause/expiry/loss behavior and bounded caller action queues. Then integrate the
board runtime and measure target heap/stack high-water, fragmentation and stress.
The candidate memory budget above does not close those runtime/physical gates.
