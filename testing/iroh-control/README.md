# Iroh wall-node control host probe

This standalone executable composes iroh 1.3.0, Postilion's `ControlClient` and
`ControlExchange`, the existing Retinue signed envelope, Seneschal's durable
codec, and radio-hand's real `ControlRuntime`. Its separate workspace and lock
leave the production dependency graph unchanged. The owning direction and
remaining device work are in the
[wall-node plan](../../design_docs/archive_docs/2026-11-02/2026-08-30_wall_node_management_plan.md#proposed-iroh-management-carrier-2026-10-10).

Run from the repository root. Create the ignored output directory first and
choose a new filename; the probe refuses to overwrite an existing file.

```sh
cargo run --manifest-path testing/iroh-control/Cargo.toml --locked --offline -- --output testing/receipts/iroh-control/new-run.json
cargo test --manifest-path testing/iroh-control/Cargo.toml --locked --offline
cargo clippy --manifest-path testing/iroh-control/Cargo.toml --locked --offline --all-targets -- -D warnings
cargo fmt --manifest-path testing/iroh-control/Cargo.toml --check
```

For the first build, omit `--offline` if the locked dependencies are absent.
The controller launches fresh server processes in a temporary directory and
removes the fixture on exit. Each exchange uses a new real QUIC connection and
one bidirectional stream; FIN delimits the existing signed-command bytes and
WN0 response. Both endpoints use IPv4 loopback, the Minimal preset, disabled
relays and no address lookup. The experiment's ALPN is
`retinue-probe/control/1`; application reset codes distinguish rejection,
oversized input and deliberate response loss. This is a probe-local mapping.

Four concurrent network sessions and a four-entry board queue are permitted.
Input is bounded to the existing 365-byte command maximum and responses to the
existing 256-byte maximum, with a three-second exchange/read timeout. These
are application limits, not a measured total iroh memory bound. A separate
board actor completes admitted commands and runs expiry while connections
disconnect or wait for FIN. No network future owns the control runtime.

The board uses the existing A/B record selection and durable state codec over
two 4096-byte files with fsync, including readback. The quiet guard and physical
configuration applier are simulations. Known fixture signing, transport and
semantic-tag keys are test data; commissioning and production custody are not
implemented. Only public identity and non-secret observations enter the
receipt; the temporary journal and commit tokens are not retained.

The [recorded run](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/iroh-control/results/host-20261011.json) passes 16 scenarios: signed status;
wrong command target; bad signature; transport identity versus command grant;
outer replay; oversized stream input; verified malformed inner payload;
apply/commit; explicit revert; conflicting armed transaction reuse; lost
response followed by a fresh-counter semantic retry; old mutation after cache
replacement; committed reopen and replay rejection in a fresh process;
autonomous expiry during an unfinished stream; reboot rollback; and wrong
iroh endpoint key. The receipt identifies source and lock hashes and the
[dependency graph](https://github.com/merely-made/retinue/blob/b0e31de8b581940dcd347968585c401e3ad786ca/testing/iroh-control/results/dependencies-20261011.txt).

The useful client finding is that the high-level mutation methods mint a new
transaction each time. The lost-response probe retains a `Request` and uses
the existing `sign_request` helper to repeat it with a new outer counter. A
prepared-request/retry API is a future Postilion integration choice. The
existing durable engine already supplies duplicate behavior: conflicting
armed reuse is `TransactionConflict`; an old mutation after its cached result
has been replaced is `TransactionExpired`.

This evidence is host composition on macOS arm64. Abrupt fresh-process restart
does not model a mid-write power cut. MCU execution, bare noq interoperability,
NAT/relay/discovery, physical quiet windows, radio coexistence, power and
unattended operation remain unmeasured. No WN, HW or Signalman gate closes.
