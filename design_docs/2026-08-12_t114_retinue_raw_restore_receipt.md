# T114 Retinue raw recovery receipt

**Date:** 2026-08-12

**Purpose:** restore the T114 after the admitted official Meshtastic T114 package run.

## Pre-write package check

`linkboy inspect firmware/packages/t114-v47.toml` accepted the Retinue T114 package and its
single DFU ZIP payload:

- package: `retinue.t114` `0.0.1-v47`
- payload: `firmware/t114-phy/tulle-t114-phy-v47.zip`
- SHA-256: `e730dcdbfd8c5139dd8754faa2f1b0fb3a34dfbad9fc356fe4cd81e2bc80f268`
- declared application range: `0x26000..0x681e2`
- declared preserved ranges: `0xea000..0xec000`, `0xec000..0x100000`

## Transfer

The ordinary package command deliberately refused the silent Meshtastic application before
opening the port because its serial DFU helper cannot acquire processor, flash-size, and
SoftDevice facts. It made no write.

The operator then used Linkboy's explicit expert route with the inspected exact ZIP:

```text
linkboy flash-raw COM3 firmware/t114-phy/tulle-t114-phy-v47.zip t114
```

Linkboy sent the named application port to its bootloader and discovered the newly enumerated
`COM4` DFU port. `adafruit-nrfutil` reported `Device programmed.` after activation.

## Application verification

The restored board re-enumerated on `COM10`. Its own 115200-baud replies were:

```text
tulle/t114 phy online; version=0.0.1; ... region=US915 ...
identity=loaded slot=A seq=82
region=US915
channel=modem
```

This proves the exact inspected ZIP reached a newly discovered T114 DFU port and that the
returned application identified as Retinue T114 `0.0.1` with `US915` and `modem`.

It is not an immutable-package execution receipt because `flash-raw` is the expert recovery
route, and it is not a Signalman graphical receipt. The missing product seam is a factual
handoff from a newly observed T114 bootloader into the package planner, without inventing
loader facts for a silent foreign application.
