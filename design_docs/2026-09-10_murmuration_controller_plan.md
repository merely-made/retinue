# Murmuration controller implementation plan

**Status, 2026-09-11: MC0–MC2, MC3a packet adapters, MC3b host-retained
sessions and MC3c resource drain complete; full MC3 remains open.** The user authorized
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
