# Retinue firmware packages

This directory contains Linkboy package manifests, payloads, recovery instructions, and the
public package index. The index currently lists the two Retinue packages as `partial`. That
status is intentional: the packages have integrity metadata and recovery guidance, but the
public installer has not yet produced the required installer and recovery receipts.

## Before installing

Use Linkboy's package path so the manifest, payload hash, board facts, flash route, and recovery
instructions are checked together. The current development slice invokes the pinned helpers
from `PATH`:

- Heltec WiFi LoRa 32 V4: `espflash` 4.5.0 through the ESP32-S3 ROM loader.
- Heltec T114: `adafruit-nrfutil` 0.5.3.post16 through serial DFU.

Helper packaging, license review for `adafruit-nrfutil`, and Windows, macOS, and Linux physical
receipts remain open before this becomes a public cross-platform installer.

Inspect the catalog and a package before connecting a board:

```text
linkboy catalog firmware/packages/index.toml
linkboy inspect firmware/packages/heltec-v4-current.toml
linkboy inspect firmware/packages/t114-v47.toml
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
