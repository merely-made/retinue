# Linkboy F5 helper packaging spike

**Date:** 2026-08-08  
**Status:** complete 2026-08-19; V4 physical receipts cover Windows, macOS, and
Linux, and the public T114 UF2 route has its Windows real-device receipt

## Measured inputs

The current Windows checkout reports:

- `espflash 4.5.0`
- `adafruit-nrfutil version 0.5.3.post16`

The installed `espflash` package declares `MIT OR Apache-2.0` and points to
`https://github.com/esp-rs/espflash`. The installed `adafruit-nrfutil` distribution reports
`Nordic Semiconductor proprietary license` and points to
`https://github.com/adafruit/Adafruit_nRF52_nrfutil`.

## Decision for this slice

The V4 route bundles an official `espflash 4.5.0` release executable below the
desktop executable at `helpers/<platform>`. Each package records the exact
executable digest, retained release-archive digest and URL, version, license,
source, and notice for Windows x86-64, macOS Arm and x86-64, and Linux Arm and
x86-64. Linkboy selects the current platform record and verifies the executable
before it probes or writes. Ambient `PATH` lookup remains available only through
the explicit development setting `LINKBOY_ALLOW_PATH_HELPERS=1`.

The public T114 route no longer redistributes `adafruit-nrfutil`. Linkboy creates
and writes a deterministic application-only UF2 through the stock HT-n5262
mass-storage bootloader. Package admission verifies the UF2 block sequence,
address map, write range, and nRF52840 family ID before a plan exists. The
application must return as the expected Retinue board and version before the
receipt becomes `Complete`. Serial DFU remains an expert recovery route, outside
the public catalog.

The official Windows V4 executable and the generated T114 UF2 are assembled in
the four-package Windows stage recorded by
`2026-08-19_signalman_public_f5_windows_receipt.md`.
`2026-08-19_linkboy_f5_macos_linux_v4_receipt.md` records V4 physical
flash-and-recovery on Intel macOS, Apple-silicon macOS, and Linux. The full
Windows stage receipt records the matching V4 loop on O-PC. A native ESP ROM
adapter remains a possible later simplification, not an F5 dependency.

## Stop condition

F5 is complete. The cross-platform V4 evidence and the public T114 real-device
receipt are retained in the linked receipt documents. Signing and installer
format remain later distribution work.
