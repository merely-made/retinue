# IFAC provisioning preflight

September 27, 2026. Baseline `6c69592`, clean at start. Two Sol agents audited
the board and host owners; one implemented the resident Debug redaction fix.
The parent reviewed, formatted and verified the patch. Root README is unchanged.

The [wire plan](../../../design_docs/2026-09-27_wire_compatibility_plan.md#credential-provisioning-preflight-september-27)
records the findings and remaining design choice. This receipt does not claim
credential provisioning, a vault, encrypted USB, or physical acceptance.

## Verified source boundaries

- `radio-hand/settings.rs`: 68-byte body, identity first, appended preferences.
  Both board `store.rs` implementations read the current body plus 32 bytes.
  A full 64-byte IFAC key appended to this record would exceed older readers'
  capacity. Storage migration cannot assume old firmware preserves identity.
- `radio-hand/resident_wire.rs`: version 1, fixed 185-byte setup. Decode checks
  version after collecting that fixed size. An extended message is not safely
  rejected as a unit by an older firmware. V4 `channels::ControlFrameStream`
  already supplies a bounded KISS boundary for extended exchanges.
- T114 main constructs `NodeChannel::new`; V4 resident construction uses
  `Runtime::new`. The protected constructors from the prior commit remain opt-in.
- `radio-hand/control` owns authenticated management and durable transaction
  semantics. V4 `radio_owner` refuses nonempty sealed credentials. The wall-node
  plan requires encryption or a separately sealed, context-bound payload and
  keeps T114 credentials host-owned. The existing resident probe path cannot
  be represented as a completed authenticated management provisioner.

## Patch and checks

`SennetKey` and `ResidentSetup` now implement Debug with redacted key/seed
fields. Nested `ResidentSetupByte::Complete` inherits that protection. Public
metadata, profiles and budgets remain inspectable; encoded bytes are unchanged.
The regression uses distinct byte sentinels for both AES key forms and the
Tucket seed, asserts absence from direct and nested Debug output, and checks
that useful metadata is retained.

`cargo test -p radio-hand --lib resident_wire --locked --offline -j 2`:
**4 passed**, including unchanged round-trip, fragmentation and refusal tests.
See `resident-wire.log`. `cargo fmt -p radio-hand` formatted the patch.

The existing target `C:\t\cargo-targets\retinue` and ordinary Cargo cache were
reused. No hardware, real credentials, settings records, isolated build directory,
Cargo home or worktree was changed or created. Retained directories from the
preceding receipt were not retried for deletion.
