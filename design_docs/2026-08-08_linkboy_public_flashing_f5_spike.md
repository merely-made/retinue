# Linkboy F5 helper packaging spike

**Date:** 2026-08-08  
**Status:** Windows V4 staging decision landed; public helper policy remains open

## Measured inputs

The current Windows checkout reports:

- `espflash 4.5.0`
- `adafruit-nrfutil version 0.5.3.post16`

The installed `espflash` package declares `MIT OR Apache-2.0` and points to
`https://github.com/esp-rs/espflash`. The installed `adafruit-nrfutil` distribution reports
`Nordic Semiconductor proprietary license` and points to
`https://github.com/adafruit/Adafruit_nRF52_nrfutil`.

## Decision for this slice

Keep the T114 helper external while its redistribution policy is unresolved. The
Windows V4 staging build bundles the pinned `espflash` executable below the
desktop executable at `helpers/windows-x86_64`. Package manifests record the
helper program, exact version, license, source, and notice; Linkboy resolves
that installed helper before it probes or writes. Ambient `PATH` lookup is
available only through the explicit development setting
`LINKBOY_ALLOW_PATH_HELPERS=1`.

The T114 helper is not a bundling candidate until redistribution permission is resolved. The
ESP helper has a reproducible Windows V4 stage and a physical flash-and-recovery
receipt in `2026-08-15_signalman_windows_v4_staged_helper_receipt.md`.
macOS and Linux receipts are still required. A native ESP ROM adapter and the
T114 UF2 route remain measured alternatives, not assumptions.

## Stop condition

F5 is not promoted by this spike. The V4 Windows stage is neither a signed
public installer nor a T114 distribution path, and cross-platform status
remains unclaimed until each named desktop system has a real flash and recovery
receipt.
