# AIR2 announce-ingress software receipt

Date: 2026-08-12  
Lane: Air  
Scope: AIR2 only

> **Follow-up, 2026-08-30:** this remains the historical pre-freshness receipt. Phase C
> later made its rapid-repeat fixture stale because each packet was minted by a new sender
> with a reset whole-second timebase. The current fixture retains one sender so the second
> announce is genuinely newer. The diagnosis and current validation are recorded in the
> [announce timebase plan](2026-08-25_announce_timebase_plan.md); the original run below is
> unchanged.

## Claim

Retinue now admits inbound announces through two bounded, independently
attributed state machines. A noisy interface holds verified unknown-route
announces and releases them one at a time after the burst subsides. A noisy
destination remains locally learnable but is not re-relayed while its
destination rate state is blocked.

This is a host software receipt. It is not an on-air measurement, a firmware
high-water measurement, route-expiry work, or a closure of FT1, FT2, FS6, or
LE3.

## Implemented seam

- [`announce_admission.rs`](../crates/retinue/src/announce_admission.rs)
  owns the policy, state rows, verdicts, capacity eviction, and per-interface
  counters. Its normal policy follows the H1 donor facts: fresh interfaces are
  judged at 3 Hz, established interfaces at 10 Hz after two hours, with a
  15-second burst latch and penalty and five-second release spacing. Interface,
  destination, and held-work capacities are bounded and configurable.
- [`endpoint.rs`](../crates/retinue/src/endpoint.rs) verifies an announce before
  applying ingress pressure. It retains a held packet with its original
  `InterfaceId`, runs one deferred-release task per interface, and exposes
  `announce_ingress_counters`. Detaching an interface drops its retained work
  and its admission row and wakes its release task.
- Destination rate state replaces the old timestamp-only announce budget. Its
  `rate_violations` and `blocked_until` state govern relaying only: a valid
  announce still reaches the address book, path table, and local announcement
  stream before an outbound relay is suppressed.
- `Endpoint::set_announce_ingress_policy` controls the operating values. The
  default destination policy preserves the previous one-second minimum relay
  interval; callers can choose a grace count and penalty.

## Host flood receipt

`crates/retinue/tests/endpoint_ingress.rs` sends ten distinct, signed announces
through one raw interface under an accelerated test policy with a four-entry
hold queue. It proves that the endpoint:

1. attributes held and dropped work to the noisy interface;
2. bounds that queue and drops excess verified work;
3. releases retained work later instead of silently discarding all of it;
4. accepts a quiet announce on a different interface while the first remains
   burst-latched; and
5. learns a rapid valid re-announce locally while recording a destination
   relay-rate block.

The accelerated policy exists only to keep the receipt fast. The normal
production defaults are asserted separately in the deterministic admission
tests.

## Verification

```
$env:CARGO_TARGET_DIR='C:\t\retinue-air-20260812'
cargo test -p retinue
```

Passed on 2026-08-12, including the deterministic admission-state tests and
the raw-interface ingress flood receipt. `rustfmt --edition 2024` on the four
changed Rust files and `git diff --check` also passed.

## Still open

- AIR3 route expiry, firmware-bounded state, and the sustained T114 memory and
  transport-relay receipt.
- FT1 modeled-versus-measured airtime and the enforced two-interface on-air
  announce cap.
- LE3 capture-dwell measurements and all other physical-radio claims.
