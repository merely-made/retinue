# Murmuration retained-session physical receipt

**Status: MC3b–MC3e bounded slices passed, 2026-09-11.** Two host-owned Retinue `Node` instances
retain one link while their V4/T114 radios change personalities. This establishes
local session retention through real RF. The extensions below prove explicit
session loss and board-timed PHY return; resident protocol adapters and arbitrary
remote-peer survival remain unproved.

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

## MC3c extension: resource drain before departure

Run `mc3c-1` passed in 41.854 seconds, based on commit `95393fb` plus the
source-hashed harness extension. A 350-byte random resource crossed from V4 to
T114 in two MTU-constrained parts. Five exact RF packet deliveries carried its
advertisement, request, two parts and final proof. The host refused departure
at three stages: outbound publication before advertisement, active inbound
reception, and sender awaiting proof after exact receiver delivery. Both radio
controllers remained home throughout those refusals.

After transmitting the queued proof, both resource states cleared and both
Nodes admitted a 36-second pause bound. The existing sequence then passed:
normal excursion, cancellation, deadline return, pin refusal and ten encrypted
messages on one retained link. This run logged 35 exact receives in total,
including Sennet replay recognition. Five return configuration acknowledgements
took 1.561–7.486 ms, with subsequent RF traffic separately proving reception.
Four TX-acknowledged home packets were not captured at the away DUT, under the
same observation limits as MC3b.

Original profiles, status and sync were restored, with the same boot IDs as MC3b.
No firmware was flashed. Executable SHA256:
`c893618b47e06e2d3317e5d746c44caa2c7ed035c0332ebef93886276cb8a06e`.
The raw JSON's `extensions` entry preserves this run separately from MC3b.

Validation passed eleven pause tests, including a 3,000-byte multipart exchange
and a dropped-final-proof case that keeps the sender blocked while polling emits
a retry. The pump checks action overflow, interface, recipient and complete queue
drain. The physical pump bounds queue length and total deliveries and verifies
actual received bytes before Node ingestion. Strict Clippy for the example and
pause tests, scoped formatting and the validation registry passed. Adding the
test target's missing `alloc` requirement also restored the allocation-free test
configuration, which passed 23 tests.

This proves waiting for completion before departure. Forced interruption and
loss reporting remain open. Review found that `Node::poll` does not purge resource
entries when it expires their link; the plan records this cleanup boundary.
Lost-proof tests establish conservative refusal and retry, not exactly-once
application delivery or timeout-based cleanup.

## MC3d: explicitly reported session loss

`Node::force_interrupt` requires `AllowSessionLoss`; `PreserveSessions` refuses
before mutation or entropy consumption. A bounded report lists every lost link,
pending handshake, resource and transit bridge independently of the ordinary
action queue capacity. Encrypted close packets are best effort and make no
remote-acknowledgement promise. The owner must settle in-flight hardware work,
cancel or account for old queued actions, and stop ingestion during the switch.
Identity, learned peers, freshness history and the resource IV sequence survive.

Qualified run `mc3d-1`, 48.868 seconds, first completed the earlier resource-drain
sequence. It then held a new 350-byte resource after its offer crossed RF,
cancelled one unsent peer request, and verified denied permission preserved state.
Allowed interruption reported one local link and one outbound resource lost.
The encrypted close crossed RF and caused exactly one peer LinkDown and receiver
cleanup. A fresh request and proof established a different link ID on the same
Node instances; encrypted data and the normal excursion sequence then passed.
This run logged 41 exact receives and twelve decrypted application messages
across the old and new sessions. Return configuration ACKs took 1.682–7.475 ms.

Sixteen independent pause tests passed, including pending proof rejection,
inbound/outbound resource loss, retained caller-owned actions and complete reports
with `ACTIONS=1` and two links. A Node unit regression separately verifies a
reported transit bridge is removed and late link traffic is not forwarded.
The full Retinue library/integration suite passed before that additional focused
regression, which also passed. Strict library/example/test Clippy and Rustdoc passed.

## MC3e: V4 board-timed PHY return

The existing controller now lives once in allocation-free `selvage`, re-exported
by Tulle. The V4 USB modem is the embedded consumer. A volatile request is
`0x06` plus the remaining fifteen bytes of a normal PHY configuration and an
eight-byte little-endian duration. `0x87` reports status, original deadline and
board time in eighteen bytes. The fixed-size demultiplexer handles fragmented
and coalesced input and preserves literal markers inside other command bodies.
The accepted profile and duration come from the caller; home is captured from
the current radio profile. Duration is limited to 60 seconds in this first board
integration. It neither installs firmware protocol adapters nor changes flash.

The board refuses invalid duration/profile, non-quiet entry and pending provisional
configuration. This operation is supported only on the USB modem build; other
builds refuse it. During an excursion, host commands are fenced and completed RF
is recorded locally without blocking on USB output. A level-high completed frame
is collected before an already-ready timer. Profile transitions remain in the
radio owner's quiet guard. Uncertain operations or overdue restoration reset as
fault recovery; ordinary success never means such a reset occurred.

Run `mc3e-1` refused 60,001 ms, accepted a fragmented 2,000 ms request, and then
received no host reads or writes for 3,500 ms. Its accepted deadline was board
time 49,791 ms. It reported home RX armed at 49,811 ms, **20 ms after deadline**.
Two subsequent byte-exact RF transfers, one each direction, proved usable home
reception. These are board-reported timing and separate RF receipts, not an
external timing instrument measurement or adversarial RX stress result.

The same boot interval spans MC3e and MC3d: V4 `5068585929170320857`, T114
`9426683542982997560`. Both runs restored original profiles with ACK0 and matching
status/sync. The T114 was not flashed. Installation resets precede this interval.

### Exact build and guarded installation

V4 USB release build:

```text
cargo +esp build -p tulle-heltec-v4-phy --release --locked --offline --target xtensa-esp32s3-none-elf -Zbuild-std=core --target-dir C:/t/murmuration-v4-target
```

The application is 402,272 bytes. The unpadded merged image is 467,808 bytes,
below the backed-up 512 KiB application/boot region and well below durable state
at 0x3F0000. Installed ELF SHA256:
`e9f15ffaf3d4de2e78de7a24a464b00a12d9b0e63f87eaa7904afc1b91449eb5`.
Merged image SHA256:
`43adbe756cd77b79d61ffc346c293bde13de42786aaafdb913941523ac2f7eff`.
The flash tool verified writes. Final readback of the 32 KiB settings/control
region matched its pre-installation copy byte-for-byte, SHA256
`34add75ebb5560978f9eb5801f71ef62a60942584acbbe53ae05f1ed933041a0`.

Two attempts at a new full 16 MiB read stopped mid-transfer and yielded no usable
full backup. The earlier full backup remains available privately, together with
a successful fresh 512 KiB boot/application read and the 32 KiB durable read.
The fresh prefix hash is
`4f59092ec2c48b3a66267571328e13eb1f3f39431a57caabfc49ebb55e7bf653`.
Backup contents are not published. A USB build was installed, then refreshed
after a UART-only declaration fix; final qualification uses the latter artifact.
Cold USB attachment required a status retry, as in MC3a.

Final software checks passed: 29 Selvage unit tests, seven wire tests, 85 Tulle
tests, strict shared-crate Clippy/Rustdoc, USB release build and UART release
check. The USB build has the same two sleep-proof dead-code warnings as MC3a;
UART reports five feature-specific dead-code warnings. Source hashes, raw runs
and artifact metadata are in the JSON extensions. These are working-tree
receipts based on `37a5226`, with unrelated Signalman work preserved.
Final formatting reordered only Selvage module declarations after the physical
runs; the JSON preserves the exercised source text and original byte hashes.

Remaining: resident firmware Retinue/Sennet adapters, recurring schedules and
firmware pin settings, authenticated coverage, power-cut/driver-fault injection
and adversarial host/RX stress. Explicit loss is locally complete; remote closure
requires actual delivery, as demonstrated only for this paired bench.
