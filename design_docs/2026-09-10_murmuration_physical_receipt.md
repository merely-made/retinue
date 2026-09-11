# Murmuration host packet-adapter physical receipt

**Status, 2026-09-10 local / 2026-09-11 UTC:** MC3a passed. This is host-driven
Retinue/Sennet packet-adapter switching through the board radio owner, not full
protocol-session suspension or autonomous firmware scheduling.

[Implementation plan](2026-09-10_murmuration_controller_plan.md) ·
[Complete experiment data](2026-09-10_murmuration_physical_receipt.json)

## Qualified result

Runs 10 and 11 passed on the same continuously booted pair:

| Property | Evidence |
| --- | --- |
| DUT | Heltec V4 COM7, USB 303A:1001, `44:1B:F6:6A:FA:64` |
| Peer | T114 COM10, USB 1915:521F, `TULLE-T114-01` |
| PHY | 906.875 MHz, BW250, SF11, CR5, preamble16, 7 dBm |
| Home / excursion | Retinue plain packets, sync 0x12 / Sennet encrypted texts, sync 0x2b |
| Successful transfers | 32 byte-exact, decoded receives: 24 Retinue and eight Sennet |
| Normal operation | Two complete excursions per board per run, retaining packet/channel state |
| Return acknowledgements | Ten, 0.883–7.362 ms; separate packets prove reception after return |
| Deadline | One-second V4 excursion expires; return and subsequent home receive pass |
| Pin | Excursion refused after reconstructing immutable pin policy over the same live serial link; home receive still passes |
| Uncovered home traffic | Eight peer TX acknowledgements; zero corresponding captures at the away V4 |
| USB boundary regression | Exact 64- and 128-byte RX envelopes pass in both RF directions |
| Boot continuity | V4 `93234391158868337`; T114 `9426683542982997560`, unchanged before/after both runs |
| Restoration | Captured original profile restored with ACK0; status and sync match before/after each run |

The home-gap count is a deliberately missed receive opportunity, not proof of
eight independently witnessed on-air losses. Config ACK0 reports profile
application, not a dedicated RX-ready signal. Return timings exclude subsequent
packet airtime. Sennet IDs are reserved and flushed to per-run local state before
TX; identity keys from the attached boards are not used by the test adapters.
The third V4 on COM6 was left unused.

## USB finding and guarded firmware work

Earlier runs completed normal switching but lost a 57-byte radio packet whose
seven-byte USB envelope made an exactly 64-byte event. Three runs reproduced
this, including a 100 ms settling experiment. Shorter packets passed the same
deadline-return scenario. The local esp-hal 1.1.1 writer sends 64-byte chunks;
its flush waits for endpoint availability without adding a short terminator.

[`SplitHost`](../firmware/heltec-v4-phy/src/host.rs) now splits the last byte of
nonempty exact USB packet multiples into a separate write. This preserves the
stream and provides a final short packet. Diagnostic writes retain the existing
250 ms deadline and error latch. UART behavior is unchanged.

The V4 firmware was built from working-tree base `13f46b6` using:

```text
cargo +esp build -p tulle-heltec-v4-phy --release --target xtensa-esp32s3-none-elf -Zbuild-std=core --locked --offline --target-dir C:/t/murmuration-v4-target
```

The installed ELF SHA-256 is
`8308553feffc0b3e216208a927da908b2cf4f7d578bfb7ce47e25b39c17497a9`.
The application is 397,696 bytes. A complete 16 MiB backup preceded flashing;
the merged image ends below the settings region at 0x3F0000. The 32 KiB settings,
reservation, control and commissioning region read back byte-for-byte unchanged
during the flash experiment. Backup bytes remain private under
`C:/t/murmuration-mc3`; the JSON records hashes rather than identity material.

Initial post-flash serial probes failed. Recovery restored the original flash
prefix and verified the original status/identity, then reinstalled the fixed
image. Explicit DTR attachment and status-read retries established the new
runtime before the qualified tests. These installation resets are separate from
the uninterrupted boot-nonce intervals of runs 10 and 11. The precise cold-attach
timing is not characterized. The T114 was not flashed; its generic banner is not
an exact firmware binary receipt.

## Reproduction and scope

```text
cargo build -p retinue --example murmuration_probe --features tulle-radio --locked --offline --target-dir C:/t/murmuration-target
python testing/mc3_personality_bench.py --exe C:/t/murmuration-target/debug/examples/murmuration_probe.exe --output C:/t/murmuration-mc3/repeat.json --baseline-receipt C:/t/murmuration-mc3/receipt-11.json
```

The runner requires the named stable USB identities and a compatible baseline.
A first run captures a sole profile descriptor; subsequent runs reuse a prior
verified restoration only with the same boot/status/profile. An explicitly
reset or reflashed DUT uses `--fresh-dut-baseline`. It refuses ambiguous baselines,
records partial failures, and restores captured profiles in `finally`.
Do not use the example directly to bypass those preconditions.

The raw record retains failed preflight and physical experiments. Run 5 is
excluded from exact-build evidence because compilation overlapped its preflight;
run 6 is the stable-build short-payload control. Runs 10/11 pin the host executable
and source hashes, fixed V4 image, physical results and restoration.

Software validation: Tulle all-feature tests passed 59 unit, 21 independent
controller acceptance and five RNode capture tests. Strict Tulle/example Clippy,
strict Tulle Rustdoc, scoped formatting and registry verification passed. The
V4 release build had only two existing sleep-proof dead-code warnings.
Four inline V4 host-writer tests also passed through a temporary desktop harness
that includes the actual firmware file. It uses Embassy's `std` driver and
`generic-queue-8`, plus `critical-section/std`; the harness manifest and entry
point are retained in the JSON. These tests check byte preservation, terminal
short writes and diagnostic write/flush fault latching. They do not simulate USB.

Remaining gates: actual remote-session preservation or explicit termination,
busy RX/TX transitions, physical cancellation and failed restoration, autonomous
board operation, keeper coverage and full three-stack compatibility. MC3a does
not close these LE/CM gates.

## Onboarding observations

MeshChat X was optional. The local application backend started, but its HTTPS
UI at 127.0.0.1:9337 was blocked by certificate authority validation. No node list
was inspected; no message was sent, no configuration was changed, and the
processes started for this check were closed. ThinkPad inspection was not needed
for the radio proof.

Concrete follow-on improvements: identify boards by USB serial rather than COM
number; distinguish bootloader, USB-attached and firmware-responsive states;
report port ownership and profile compatibility; show configured home, current
personality, acknowledged return and counted absence separately. Cold-attach
recovery should be an explicit diagnostic action, not a hidden reset during a
claimed uninterrupted session. These are findings, not implemented UI changes.
