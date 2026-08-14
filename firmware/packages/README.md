# Retinue firmware packages

This directory contains Linkboy package manifests, one-file payloads or ordered sparse parts,
recovery instructions, and the public package index. Every part has its own digest; sparse ESP
parts also name their write offset. The index currently lists the two Retinue packages as
`partial`. That status is intentional: the packages have integrity metadata and recovery
guidance, but the public installer has not yet produced the required installer and recovery
receipts.

## Before installing

Use Linkboy's package path so the manifest, every artifact hash, board facts, flash route, and
recovery instructions are checked together. The current development slice resolves the pinned helpers
from `PATH` once per install, then keeps that resolved executable for the write:

- Heltec WiFi LoRa 32 V4: `espflash` 4.5.0 through the ESP32-S3 ROM loader. The current Windows
  development package also requires its recorded executable SHA-256.
- Heltec T114: `adafruit-nrfutil` 0.5.3.post16 through serial DFU.
- Meshtastic T114: Linkboy's built-in, explicit UF2-volume writer. The retained official
  release file is GPL-3.0 external firmware, not a Retinue capability claim. A UF2 volume can
  disappear while Linkboy flushes a complete file: that expected Windows device-removal
  acknowledgement is recorded, but the package still requires an upstream interface check.

The retained Prns Hopspot V4 package is an external firmware choice, not a Retinue capability
claim. Its signed channel descriptor and flash manifest are retained with its immutable sparse
parts. Linkboy verifies every part and preserves the HSPCFG1 provisioning slot, then requires the
owner to exercise Hopspot's own interface before calling the route proven.

Helper packaging, license review for `adafruit-nrfutil`, and Windows, macOS, and Linux physical
receipts remain open before this becomes a public cross-platform installer.

Inspect the catalog and a package before connecting a board:

```text
linkboy catalog firmware/packages/index.toml
linkboy inspect firmware/packages/heltec-v4-current.toml
linkboy inspect firmware/packages/t114-v47.toml
linkboy inspect firmware/packages/hopspot-v4-0.3.4.toml
linkboy inspect firmware/packages/meshtastic-t114-2.7.26.54e0d8d.toml
```

The accepted plan is the only path that may write. Use the board-specific selection when a
loader can prove the processor but cannot name the carrier revision:

```text
linkboy plan PORT firmware/packages/heltec-v4-current.toml v4@REVISION
linkboy plan PORT firmware/packages/t114-v47.toml t114@REVISION
```

Do not identify a board from a COM number, USB identifier, or processor alone. Read the plan's
state-impact and recovery sections before accepting a write.

## Recovery

- [Heltec V4 recovery](heltec-v4-current-recovery.md)
- [T114 recovery](t114-v47-recovery.md)

The recovery page remains part of each package's public evidence. The index must not be changed
to `proven-recipe` until both installer and recovery receipts link to reproducible public runs.
