# T114 installation and isolated resident cancellation

Firmware bytes remain pinned to clean source
`711816a8124bd58cd8cc314ec4de5009be7df166`. The initial parent receipt
is immutable and describes its earlier stopping point. This follow-up adds
fresh Windows hardware evidence for T114 and a host-only cancellation bench
at `1525278`.

## T114 write and preservation boundary

The owner double-tapped reset into the stock `HT-n5262` volume. Linkboy
captured bootloader 0.9.0/S140 6.1.1 facts, accepted the application-only UF2
plan and wrote all 623104 verified file bytes. The payload SHA-256 is
`27424c8e634c10fe84a85823990d63ea8c11de23cd008e3ae877896984e6cfc9`.
Its 311552 padded application bytes occupy `0x26000..0x72100`.
The `manual-check-required` transaction result is completed by independent
live source/image status in `t114-installed.json` and `t114-first-identity.log`.
The first return retained original slot A / sequence 84 and US915/modem.
Later deliberate native/modem selections advanced the persisted sequence;
the final return is modem, slot B / sequence 93, same node `599997c8`.

Before writing, the stock CURRENT.UF2 was retained privately. It covers only
`0x1000..0xEA000`, excluding the settings pair `0xEA000..0xEC000` and
parts of the bootloader. `t114-original-state.json` records that limited
coverage. It is not a full-device backup. Original live settings sequence
and identity survived, but byte-identical settings preservation was not
measured. The manifest excludes control, announce, settings and bootloader
ranges from the write; physical power-cut/rekey acceptance remains open.

The new artifact emits `state=node-timebase-v1` while native mode is armed.
`t114-guarded-plan.log` records admission of the matching guard-aware package;
`t114-armed-v51-refusal.log` records refusal of the old unguarded v51 package
on the same armed device. Legacy artifacts remain unchanged. Catalog release
52 offers the current bytes as `partial`: no same-image failed-application
UF2 recovery retry has been exercised. Historical v51 recovery is separate.

Reproduce the package route with the current recovery instructions and:

```text
linkboy flash-volume VOLUME firmware/packages/t114-current.toml t114@2.x --receipt NEW.json
linkboy ask APP_PORT status channel heap
```

The initial serial-only loader plan lacked the required loader facts and
was refused. The accepted volume route supplied those facts. UF2 ejecting
after the complete verified write was the loader transfer acknowledgement.

## Native RF scope

`t114-native-bigoffer.log` records refusal of a 20 KiB offer and a subsequent
1024-byte resource echoed byte-exact in both directions through the ordinary
V4 modem. Board diagnostics independently show refused offers and selected
heap peak 6604 bytes in a 49152-byte heap. These figures do not establish
maximum decoder/stack high-water or unattended memory acceptance.

The two raw announce captures and `t114-native-announce-verification.json`
show one unchanged public identity, independently verified Ed25519 signatures,
and strictly advancing native ordinals 131073 then 262145 across controlled
soft resets. A deliberately modified signed message was refused. There were
several resets between captures; they are not consecutive-announce or physical
power-cut evidence. Copy `verify-native-announces.txt` to a `.py` file beside
the captures and run it with Python/cryptography to recompute verification.
The native announce interval is ten minutes. Receivers were armed before
controlled restarts to capture the first announce.

Failed attempts are retained: the fuzz leg never discovered an announce,
so its transmitted malformed frames do not establish flood survival. An
empty second capture missed the restart window; the corrected capture armed
the receiver and restarted the board in one process. An initial unquoted
multiword CLI selection did not change native mode; the corrected selection
is separate. An immediate final probe failed during USB re-enumeration;
the later successful final status is retained independently.

## Resident cancellation boundary

The full resident run with refreshed T114 as peer passed three Sennet visits,
three signed home proofs and a Tucket text/ACK visit, then timed out during
the next Tucket visit. The complete suite did not pass. Earlier COM7 write
timeouts and an announce timeout prevented several focused runs from reaching
the cancellation case. Those attempts remain separate from the final run.

The host-only `--mode cancel` runner exercises three independent cycles:
signed home proof/close before departure, a missed fresh request while away,
Cancel, Home status, then a new signed home proof and encrypted close. Run:

```text
python -B testing/mc4_resident_bench.py --mode cancel --exe RESIDENT_PROBE --dut COM7 --peer COM10 --output NEW.json
```

The final `resident-cancellation-reconnected.json` verifies concrete USB serial
identities and restores the captured peer descriptor in `finally`. The DUT
accepts one-shot private commissioning material and requires an external reset
before another setup. A reused peer boot with multiple configurations requires
its prior baseline receipt; a fresh peer boot supplies its own baseline.

Cycle zero passed including same-identity signed proof and close. Cycle one
returned Home/generation 5 and reported LinkUp, but its expected proof timed
out at the peer. Independently calculated request link id
`c027c1c5331c95d82e84c76bf4b6e580` matches the DUT LinkUp. Thus request RX
and link acceptance occurred; firmware TX, RF loss and peer RX are not yet
distinguished. The three-cycle gate and complete resident acceptance remain
open. No firmware change was inferred or made from that timeout.

The later ASCII resident-status probe was invalid in the binary resident
command stream and produced refusals; it is not acceptance evidence. The
resident V4 was explicitly ROM-reset afterward, and live final probes identify
both V4 images and T114 at the exact firmware pin, all in modem mode. T114
peer restoration matched its original descriptor. The resident package remains
partial. IFAC provisioning remains a separate open gate.

## Evidence and retained tools

`checks.json` records package, staged payload, formatting, registry and Linkboy
verification. `manifest.json` hashes selected public evidence. Private UF2/raw
flash backups, packet-state files and the 185-byte setup/secret seeds are
excluded. `seal-follow-up.txt` retains the selection and checks for exclusion.

The normal target remains `C:/t/cargo-targets/retinue`; no new target, Cargo
home or worktree was created. Private evidence remains under
`validation/results/firmware-refresh`. The reused Cargo home
`C:/t/cargo-homes/retinue-wire-compat` and old stock CLI environment remain
because earlier automatic approval review rejected cleanup as "blocked by
policy". They are retained build/oracle caches, not active radio owners.
