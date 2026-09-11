# Murmuration retained-session physical receipt

**Status: MC3b passed, 2026-09-11.** Two host-owned Retinue `Node` instances
retain one link while their V4/T114 radios change personalities. This establishes
local session retention through real RF; autonomous firmware scheduling and
arbitrary remote-peer survival remain unproved.

[Plan](2026-09-10_murmuration_controller_plan.md) ·
[Raw experiments and source hashes](2026-09-11_murmuration_session_receipt.json) ·
[Prior firmware installation and MC3a evidence](2026-09-10_murmuration_physical_receipt.md)

## Qualified run

Run `mc3b-2` passed in 34.501 seconds on COM7 Heltec V4
`44:1B:F6:6A:FA:64` and COM10 T114 `TULLE-T114-01`. The PHY remained
906.875 MHz, BW250, SF11, CR5, preamble16, 7 dBm, with home sync 0x12 and
excursion sync 0x2b. No firmware was flashed in this slice.

| Check | Observed result |
| --- | --- |
| Real link establishment | Announce, request and proof traversed RF; both Nodes retained one matching link |
| Encrypted session traffic | Ten exact decrypted messages, both directions before departure and after normal return, cancellation, deadline return and pin refusal; no new handshake |
| Sennet retained state | Replayed ciphertext received over RF was recognized by the retained `ManagedFlood` as `Ignore(Duplicate)` |
| Busy refusal | Pending real handshake and queued action prevented departure before a radio retune |
| Exact receives | 30 total: 13 session packets and 17 packet-adapter transfers, including 64/128-byte USB receive envelopes |
| Returns | Five configuration acknowledgements, 1.596–7.094 ms; cancellation return 1.782 ms |
| Home absence | Four TX-acknowledged home packets, zero captures at the away DUT; no independent on-air witness |
| Continuity | V4 boot ID 93234391158868337; T114 9426683542982997560, unchanged |
| Restoration | Both original profiles restored with ACK0, unchanged status and sync |

Return timings cover configuration completion, not packet airtime or a dedicated
RX-ready signal. Successful subsequent encrypted exchanges separately prove
usable reception. Sennet duplicate recognition is a relay decision, not a
session protocol or an application-delivery guarantee.

## Protocol boundary and review

`Node::pause_assessment` is a pure query. Pending handshakes, active resources
and transit bridges block a retained pause. Idle links must outlive the requested
return bound under ordinary expiry rules; clock regression and expiry overflow
fail closed. The host must drain outstanding actions and own the radio boundary.
Protocol time continues to advance, and both Nodes are polled after returning.

The bench derives admission from both real Node assessments. Its conservative
duration includes the requested excursion plus transition, return and guard
budgets (36 seconds for a 25-second excursion). Return admission reserves six
seconds. Durations are translated into the Node clock domain rather than mixing
the independent runtime epochs.

Run `mc3b-1` also completed, but is retained as preliminary evidence only:
review subsequently extended the pause horizon to cover all transition budgets
and made Sennet duplicate assessment consume the actual received bytes.
Run `mc3b-2` includes those fixes and per-message link-count checks.

## Validation and limits

Terra implemented the protocol assessment; Luna supplied nine independent tests.
Parent integrated and reviewed the harness and performed the guarded physical run.
Validation passed:

- `cargo test -p retinue --lib --tests --locked --offline --target-dir C:/t/murmuration-target`
  (202 unit tests plus all integration suites, including nine new pause tests).
- `cargo build -p retinue --example murmuration_probe --features tulle-radio --locked --offline --target-dir C:/t/murmuration-target`.
- Strict Clippy for Retinue library, tests and the example with `tulle-radio`.
- Strict Retinue Rustdoc with `tulle-radio`; validation registry verification.

The qualified executable SHA256 is
`0df8ff519da555f11b1b65388d4d26c9f4323feb4356208d6a271675d267c2ff`.
Source hashes and full restoration transcripts accompany the raw receipt.
These are dirty-working-tree results based on
`13f46b6b3837efa6abfddece8cee25f0d275eff0`, not release evidence. Existing
Signalman work was preserved. The previous V4 installation receipt supplies
firmware provenance; exact T114 binary provenance remains open.

Still open: autonomous board scheduling, active-resource interruption and
physical fault recovery, authenticated peer coverage and broader LE/CM gates.
