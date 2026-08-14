# Meshtastic T114 UF2 demo receipt

**Date:** 2026-08-11

**Board:** Heltec Mesh Node T114 (HT-n5262), stock Adafruit UF2 bootloader
0.9.0, SoftDevice S140 6.1.1. Found in boot mode on the demo bench (owner
action); previously enumerated app was TinyUSB PID 8071.

**Status:** historical pre-admission interface demo. The manual transfer below is not a
Linkboy receipt. The exact same official asset is now retained as
[`meshtastic.heltec-mesh-node-t114`](../firmware/packages/meshtastic-t114-2.7.26.54e0d8d.toml).
The subsequent admitted Linkboy install and raw Retinue recovery are recorded separately in
the [2026-08-12 transfer receipt](2026-08-12_meshtastic_t114_linkboy_transfer_receipt.json)
and [raw restore receipt](2026-08-12_t114_retinue_raw_restore_receipt.md). Neither is a
packaged or graphical restore receipt.

**Firmware:** official Meshtastic `v2.7.26.54e0d8d`, asset
`firmware-heltec-mesh-node-t114-2.7.26.54e0d8d.uf2` (1,467,392 bytes) from
`github.com/meshtastic/firmware` releases, flashed by UF2 drag-and-drop to
the `HT-n5262` mass-storage drive. At the time, this was a manual route,
deliberately outside Linkboy, and this receipt remains demo evidence only.

Post-flash observation: board re-enumerated as PID 4405 on COM3; console at
115200 shows Meshtastic running, node `!f5d9eabd`, owner "Meshtastic eabd",
LoRa TX active, BLE pairing via OLED PIN.

**Purpose:** phone-app BLE demo (Meshtastic app), alongside the
[Hopspot V4 COM7 demo receipt](2026-08-11_hopspot_v4_com7_demo_receipt.md).
Not a Retinue capability claim; Retinue firmware carries no BLE transport as
of this date.

**Restore:** the UF2 route left the bootloader untouched. The 2026-08-12 recovery used the
hash-checked Retinue package payload through Linkboy's explicit raw T114 route after the
ordinary package planner correctly refused a silent foreign application without fresh
serial-DFU loader facts. The public package-to-serial-DFU recovery handoff remains open.
