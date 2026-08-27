# Retinue firmware packages

This directory contains Linkboy package manifests, one-file payloads or ordered sparse parts,
recovery instructions, and the public package index. Every part has its own digest; sparse ESP
parts also name their write offset. The index is the publishable installer evidence artifact:
`retinue.heltec-v4`, `retinue.t114`, and `prns.hopspot.heltec-v4` are
`proven-recipe` entries with installer, recovery, and host receipts. The retained Meshtastic
T114 package remains `partial` until its own interface check is recorded.

## Before installing

Use Linkboy's package path so the manifest, every artifact hash, board facts, flash route, and
recovery instructions are checked together. Installed builds resolve an admitted helper from their
own platform directory and keep that verified executable for the write:

- Heltec WiFi LoRa 32 V4: an official `espflash` 4.5.0 platform release through the ESP32-S3
  ROM loader. The manifest records both upstream archive and extracted executable SHA-256.
- Heltec T114: Linkboy's built-in application-only UF2 writer through the stock `HT-n5262`
  mass-storage bootloader. The retained serial-DFU package remains an expert recovery route,
  outside the public catalog.
- Meshtastic T114: Linkboy's built-in, explicit UF2-volume writer. The retained official
  release file is GPL-3.0 external firmware, not a Retinue capability claim. A UF2 volume can
  disappear while Linkboy flushes a complete file: that expected Windows device-removal
  acknowledgement is recorded, but the package still requires an upstream interface check.

The retained Prns Hopspot V4 package is an external firmware choice, not a Retinue capability
claim. Its signed channel descriptor and flash manifest are retained with its immutable sparse
parts. Linkboy verifies every part and preserves the HSPCFG1 provisioning slot, then requires the
owner to exercise Hopspot's own interface before calling the route proven.

The V4 routes have Windows, Intel-macOS, Apple-silicon-macOS, and Linux physical receipts.
The public T114 UF2 route has a Windows physical receipt. The index records those exact host
boundaries instead of extrapolating support from a helper's portability.

The Phase D package shape is `persistent_state.schema = 1` with the
`native_node_guard` and preserved `0xE8000..0xEC000` reservation. Linkboy refuses to flash a
known armed native-node device with a package that lacks that declaration. The retained v47 and
v51 binaries predate this guard, so they declare the preserved range in their manifests but do
not claim guard support. A rebuilt package must not be published with that claim until its
immutable firmware artifact emits and honors the guard. First-flash, unarmed, unknown, and
foreign running states remain eligible for an explicitly compatible package. Legacy and
external packages intentionally omit the declaration.

The guard is based on the running application's `state=node-timebase-v1` status token. A
bootloader-only observation cannot recover the prior application state; callers carrying that
fact across a loader transition must retain it in the serialized device observation. Linkboy
does not invent a hardware read for it.

Inspect the catalog and a package before connecting a board:

```text
linkboy catalog firmware/packages/index.toml
linkboy inspect firmware/packages/heltec-v4-current.toml
linkboy inspect firmware/packages/t114-v51.toml
linkboy inspect firmware/packages/hopspot-v4-0.3.4.toml
linkboy inspect firmware/packages/meshtastic-t114-2.7.26.54e0d8d.toml
```

The accepted plan is the only path that may write. Use the board-specific selection when a
loader can prove the processor but cannot name the carrier revision:

```text
linkboy plan PORT firmware/packages/heltec-v4-current.toml v4@REVISION
linkboy plan PORT firmware/packages/t114-v51.toml t114@REVISION
```

Do not identify a board from a COM number, USB identifier, or processor alone. Read the plan's
state-impact and recovery sections before accepting a write.

## Recovery

- [Heltec V4 recovery](heltec-v4-current-recovery.md)
- [T114 recovery](t114-v51-recovery.md)

The recovery page remains part of each package's public evidence. The index may use
`proven-recipe` only when installer and recovery receipts link to reproducible public runs and
name the host platforms that physically exercised the route.
