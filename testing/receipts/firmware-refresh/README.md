# September 30 firmware refresh

Firmware source is the clean commit
`711816a8124bd58cd8cc314ec4de5009be7df166`. These are new immutable
artifacts, rather than qualification inherited from older packages.

| Device | Image | Returned state |
| --- | --- | --- |
| V4 4.2, MAC `44:1b:f6:6a:fb:28`, COM6 | `v4-modem-usb` | Exact source revision, US915/modem, loaded slot B / sequence 9 |
| V4 4.2, MAC `44:1b:f6:6a:fa:64`, COM7 | `v4-resident-usb` | Exact source revision, US915/modem after reset, loaded slot B / sequence 5 |
| T114 2.x, previously COM10 | `t114-native` built only | Original application remains intact; serial bootloader COM4, awaiting physical double-tap into the stock UF2 volume |

The V4 carrier selections reuse the owner's earlier 4.2 selections for these
same MAC addresses. T114's recorded carrier selection is 2.x. Port numbers do
not establish board identity. The T114's observed loader transition is retained
in `t114-enter-loader.log`; no UF2 volume was present during this run.

## Builds

Set `CARGO_HOME=C:/t/cargo-homes/retinue-wire-compat`,
`CARGO_TARGET_DIR=C:/t/cargo-targets/retinue` and
`RETINUE_FIRMWARE_REVISION=711816a8124bd58cd8cc314ec4de5009be7df166`.
From that clean source revision:

```text
cargo +esp build -p tulle-heltec-v4-phy --release --target xtensa-esp32s3-none-elf -Zbuild-std=core --locked --offline -j2
cargo +esp build -p tulle-heltec-v4-phy --release --target xtensa-esp32s3-none-elf --features resident-protocols -Zbuild-std=core,alloc --locked --offline -j2
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf --locked --offline -j2
```

Copy each V4 ELF before building the other feature selection. The pinned logs
and `build-artifacts.json` retain artifact lengths and hashes. Official
espflash 4.5.0 `save-image --merge --skip-padding --flash-size 16mb` produced
the declared write extents. T114's raw application is 311394 bytes; Linkboy's
UF2 encoder pads it to 311552 payload bytes at `0x26000`, ending at `0x72100`.
Its ELF has 310982 text, 384 data and 92732 BSS bytes, including its 49152-byte
heap reservation. These are link figures, not measured T114 runtime peaks.
Existing V4 dead-code warnings remain in the build logs.

## Installation and recovery

Both V4 packages passed accepted Linkboy plans and digest-verified writes using
the admitted official Windows espflash 4.5.0 helper. Its executable and archive
digests are recorded in both manifests and transaction receipts. Set
`LINKBOY_HELPER_DIR` to the directory containing that admitted executable:

```text
linkboy plan PORT firmware/packages/heltec-v4-current.toml v4@4.2
linkboy flash PORT firmware/packages/heltec-v4-current.toml v4@4.2 --receipt RECEIPT.json
linkboy plan PORT firmware/packages/heltec-v4-resident.toml v4@4.2
linkboy flash PORT firmware/packages/heltec-v4-resident.toml v4@4.2 --receipt RECEIPT.json
```

The installation receipts end at `manual-check-required`, intentionally:
`v4-after-read-final.json` supplies the exact live build/image check and original
settings sequence. The ordinary modem recovery run explicitly entered the ROM
loader using `espflash board-info --before usb-reset --after no-reset`, confirmed
the same MAC and 16 MiB flash, and reinstalled the same verified public package.
`v4-modem-recovery.json` and `v4-recovery-returned.json` retain that write and
returned identity. This proves the software-entered loader retry on Windows;
physical button entry and other host platforms were not requalified.

Independent readback of the full `0x3F0000..0x400000` tail matched each original
private backup immediately after installation. After RF use and the modem
recovery, COM6 still matches its entire original tail. COM7's original settings
pair remains byte-identical; its 24 changed bytes lie only in the announce and
Sennet packet-ID reservation pairs. `v4-preservation.json` and
`v4-final-preservation.json` retain the comparisons. Backups, raw flash state and
resident setup material remain private in the ignored bench directory.

## RF scope and failures

`v4-sennet-pair.log` records encrypted public LongFast text in both directions
between the freshly installed V4s, with independent durable host packet counters.
The two-board resident adapter explicitly selects COM6's V4 MAC as peer;
`resident-peer-selection.txt` preserves that small adapter's exact source.
Reproduce it with Python and the existing `testing/mc4_resident_bench.py` runner,
passing `--dut COM7 --peer COM6 --exe resident_probe.exe --output NEW.json`.
It uses a fresh one-shot setup and restores the peer's captured PHY descriptor.

The repeated resident run records a signature-verified spontaneous home
announce, three bidirectional Sennet visits with duplicate retention, three
stable Tucket advert visits, private text/ACKs, intentional ACK withholding,
and six signed home link proofs followed by encrypted closes. A home request
was missed while away. The final fresh home link proof after cancellation timed
out, although status returned Home. Thus the complete runner reports failure:
these selected successes do not close the cancellation or resident acceptance
gate. The first RF run timed out after its second Sennet visit. Separate initial
preflight failures caught boot diagnostics before status or timed out on USB
write; all attempts remain evidence. Returned modem status after explicit resets
is retained separately, rather than inferred from reset commands.

Selected resident diagnostics show allocator peak 1444 bytes, requested peak
1439 bytes and zero allocation failures in a 65536-byte heap. The 62624-byte
stack figure samples the current stack pointer; it is not a stack high-water
measurement. These figures do not establish total decoder peak memory or
unattended-operation acceptance. No current-stock comparison was repeated here.
IFAC provisioning and its physical acceptance remain open.

The ordinary V4 package has fresh Windows installation and ROM-retry evidence.
The separate resident package remains partial. T114's new image is unpublished
pending UF2 installation, preserved-state comparison and native-node RF checks;
the retained v51 artifact and its historical receipts are unchanged.

The normal target remains `C:/t/cargo-targets/retinue`. No worktree or isolated
target was created. Private evidence is retained under
`validation/results/firmware-refresh` and the previous radio bench directory.
The reusable Cargo home and earlier stock CLI environment remain because the
earlier cleanup attempts were rejected by automatic approval review as
"blocked by policy". They are not active radio owners.

`manifest.json` hashes the selected public evidence; private binary state and
ephemeral channel material are excluded. `checks.json` records the final
package/catalog, Linkboy, formatting and registry checks.
