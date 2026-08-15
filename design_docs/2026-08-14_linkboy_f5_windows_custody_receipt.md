# Linkboy F5 Windows helper-custody receipt

**Date:** 2026-08-14  
**Host:** Windows development checkout  
**Status:** local custody evidence only; F5 remains open.

## Observed helpers

| Route | Program | Version | SHA-256 | Current source path |
| --- | --- | --- | --- | --- |
| ESP ROM | `espflash` | `4.5.0` | `768f0adfc71629a1e2e690923dd63d267cbfcd2828c26ac1315f664bca1dffc7` | `C:\Users\mark_\.cargo\bin\espflash.exe` |
| serial DFU | `adafruit-nrfutil` | `0.5.3.post16` | `458cd93f99c0ac4aa85c5e9a8a5cd9ffde4144c211d499a4ab5cf2cc9704fd9d` | `C:\Users\mark_\AppData\Local\hermes\hermes-agent\venv\Scripts\adafruit-nrfutil.exe` |

The observed `espflash` version and digest exactly match the V4 package
manifests used by the physical receipts. `adafruit-nrfutil` is presently
provided by another application's virtual environment, so it is expressly not
public helper custody.

## Boundary

No helper binary was bundled or promoted. The public installer still requires a
per-platform policy, redistribution and notice review, and real-device
flash-and-recovery receipts on Windows, macOS, and Linux. This Windows snapshot
does not satisfy any of those cross-platform conditions.
