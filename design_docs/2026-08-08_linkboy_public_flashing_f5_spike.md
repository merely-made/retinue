# Linkboy F5 helper packaging spike

**Date:** 2026-08-08  
**Status:** measured Windows slice, packaging decision open

## Measured inputs

The current Windows checkout reports:

- `espflash 4.5.0`
- `adafruit-nrfutil version 0.5.3.post16`

The installed `espflash` package declares `MIT OR Apache-2.0` and points to
`https://github.com/esp-rs/espflash`. The installed `adafruit-nrfutil` distribution reports
`Nordic Semiconductor proprietary license` and points to
`https://github.com/adafruit/Adafruit_nRF52_nrfutil`.

## Decision for this slice

Keep both helpers as explicit external dependencies while Linkboy's package and execution
semantics settle. Package manifests now record the helper program, exact version, license, source,
and notice. The public package command verifies that the installed helper matches those facts
before it begins a transfer.

The T114 helper is not a bundling candidate until redistribution permission is resolved. The
ESP helper can be reconsidered for bundling, but that needs a reproducible artifact build and
real-device receipts on Windows, macOS, and Linux. A native ESP ROM adapter and the T114 UF2 route
remain measured alternatives, not assumptions.

## Stop condition

F5 is not promoted by this spike. The public build still depends on `PATH`, and cross-platform
status remains unclaimed until each named desktop system has a real flash and recovery receipt.
