# T114 Retinue Linkboy package restore receipt

**Date:** 2026-08-12  
**Scope:** physical T114 package restore through Linkboy. This is not a Signalman graphical or accessibility receipt.

## Package and loader facts

- package: `retinue.t114` `0.0.1-v47`
- route: `adafruit-dfu` using `adafruit-nrfutil 0.5.3.post16`
- application payload: `firmware/t114-phy/tulle-t114-phy-v47.zip`
- payload SHA-256: `e730dcdbfd8c5139dd8754faa2f1b0fb3a34dfbad9fc356fe4cd81e2bc80f268`
- captured mounted-loader record: `HT-n5262`, nRF52840, 1 MiB flash, UF2 bootloader `0.9.0`, SoftDevice `S140 6.1.1`
- preserved package ranges: `0xea000..0xec000` and `0xec000..0x100000`

The loader record is captured in
`design_docs/2026-08-12_t114_loader_snapshot.json`. It supplies hardware facts from the
actual mounted T114 bootloader rather than inferring them from the foreign Meshtastic application.

## Restore and recovery verification

The packaged restore reached application verification but initially selected an unrelated V4
on `COM6`; Linkboy emitted `recovery-required` instead of a false success. The retained
post-write receipt is
`design_docs/2026-08-12_t114_retinue_packaged_restore_replay_receipt.json`.

Linkboy now filters returned applications by the package's expected board family. Its
non-writing recovery verifier checked that exact recovery receipt, package, loader record,
and the returned T114 on `COM10`. The terminal receipt is
`design_docs/2026-08-12_t114_retinue_packaged_restore_completed_receipt.json`:

- result: `complete`
- returned board and version: T114 `0.0.1`
- radio: SX1262 online
- region: `US915`
- channel: `modem`

## Evidence boundary

This is a completed immutable-package restore and returned-application interface receipt.
It does not prove the Signalman desktop flow or its accessibility surface. That run remains
blocked by unrelated incomplete `retinue::endpoint` changes in the shared worktree.
