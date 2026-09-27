# Firmware IFAC software and target receipt

**Date:** September 27, 2026. **Baseline:** `16c278461afe6873ed7f9fde2d43cea5fff2293e`.
**Scope:** Node packet budgets and both radio-hand carrier integrations.
The owner's root README is unchanged. Three Sol lanes implemented core budgets,
carrier integration and stock captures; the parent reviewed and qualified them
under the [wire plan](../../../design_docs/2026-09-27_wire_compatibility_plan.md).

## Contract and evidence

The shared carrier wraps the existing project IFAC codec. Protected ingress
authenticates before Packet decoding, and final egress reserves access-code
bytes within Selvage's 255-byte physical frame cap. Plain constructors remain
plain. A configured eight-byte access code leaves 247 logical bytes. Both
constructors preserve a caller's narrower Node budget and reject protected
startup with existing pending links, sessions, transfers or transit bridges.

Node negotiates its configured budget in both link roles. Direct sends account
for CBC padding before allocation. Fallible announcements include ratchet size;
the unconstrained builder remains available for fixtures. Relay checks include
Type2 header growth, and a refused link request creates no phantom bridge.
ResourceSender continues to own its negotiated-MTU sizing. NodeChannel exposes
refusals through existing counters; Runtime adds typed drops and RX/TX counts.

Host tests exercise actual Runtime authentication, absence of peer learning on
bad frames, matching-key peer learning, queued sealed egress and completion.
They also exercise the shared adapter used by NodeChannel. NodeChannel's radio
event integration is target-checked, not executed with a simulated PHY or a
physical board in this receipt.

The fixture producer is actual stock **RNS 1.5.4**, installed in the existing
oracle environment. LXMF remains 1.1.1. The capture uses public RNS APIs and
observed TCP/HDLC frames; no reference source was copied or translated.
The existing provenance and withdrawn-Prns ledger remain unchanged.

| Observed Type1 logical bytes | Stock IFAC wire bytes | Radio adapter result |
| --- | --- | --- |
| 231 | 239 | accepted |
| 232 | 240 | accepted |
| 247 | 255 | accepted |
| 248 | 256 | refused |

The committed fixture and independent recaptures have identical SHA-256:
`07bf7fc151bc401e2131e7cec5efc733563057f6ccd0a284ccc924520c991108`.
Rust replay verifies matching encode/decode bytes, wrong credentials, protected
plain-frame rejection, and independent one-bit mutations at every byte position.
Type2's sixteen-byte growth is tested as a **local transformation** of stock
Type1 input, not represented as a stock Type2 capture. TCP can carry 256 bytes;
the physical refusal is a Rust software result, not an on-air observation.

## Attempts and qualification

`core-initial.log` retains a constructor typo exposed by the first compilation.
`core-repaired.log` records 41 focused passing tests before the final bridge
lifecycle regression. The complete final suite includes that regression.

The first capture invocation failed because its log destination did not exist.
No output file was created. `capture-attempt2.log` records a frame-selection
assertion failure. `capture-attempt3.log` records the stock public Type2 PLAIN
constructor failure. Captures 4 through 6 succeeded after narrowing to Type1;
their logs and independent capture remain. The revised supervising runner's
attempt 7 reproduced the bytes and confirmed `TEMP_CONFIG_REMOVED=True`.
The deliberate overwrite refusal preserves the original output and is recorded
in `capture-overwrite-refusal.log`. A PowerShell quoting error before Cargo ran
is recorded in `v4-invocation-failure.log`; the corrected flag is quoted below.

Final commands and outcomes are recorded in `checks.json`, with tool versions
and source hashes in `environment.json`. Logs are retained alongside them.
These are working-tree software checks against the recorded source hashes,
not claims that each command ran from a clean committed checkout.

Passed: 354 default Retinue tests, 259 radio-hand tests with instances/replay,
255 allocation-only Retinue tests, four replay-only IFAC fixture tests, and 20
final default-feature Node pause tests. Core-only check, strict Clippy for both
changed crates, formatting and registry validation passed. T114 and V4 resident
target checks passed. V4 reports existing unused power/wake helper warnings;
allocation-only tests report an existing unused compression-test helper.

The first allocation-only integration attempt exposed an unconditional optional
compression assertion in `node_pause`; `retinue-alloc.log` retains the failure.
Gating only that assertion preserves the resource-limit test in both feature
profiles. `retinue-alloc-repaired.log` and `node-pause-final.log` verify both.

## Reproduction and open gates

Reuse `C:\t\cargo-targets\retinue` and the ordinary Cargo cache. For a fresh
stock capture, use the existing oracle `.venv/Scripts/python.exe` to run
`capture_firmware_ifac.py --output <new-file.json>`; the script refuses existing
paths. Replay through `cargo test -p radio-hand --features instances,replay`.
The exact locked/offline commands are in `checks.json`.

No device was flashed or opened. Firmware entry points still use plain defaults;
this slice supplies opt-in construction APIs, not a provisioning interface.
Credential persistence, matching provisioned peers, negative physical cases,
identified-image acceptance and on-air relay remain open. Firmware target checks
do not establish linked image size, memory headroom or power behavior.

No new isolated target, Cargo home or worktree was created. The reusable target
remains. The prior `C:\t\cargo-homes\retinue-wire-compat` remains from the earlier
lane because its cleanup was rejected. Three pre-supervisor generated RNS temp
config directories also remain: `cleanup.log` records exact paths and ownership.
Automatic approval review rejected their verified PowerShell removal with
"blocked by policy". No alternate deletion mechanism was attempted.
