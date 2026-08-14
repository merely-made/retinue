# Hopspot V4 COM7 demo receipt

**Date:** 2026-08-11

**Board:** Heltec V4 revision 4.2 on COM7

**Package:** `prns.hopspot.heltec-v4` 0.3.4 (same verified parts and espflash
4.5.0 helper digest as the 2026-08-10 install; Linkboy transfer receipt:
[JSON](2026-08-11_hopspot_v4_com7_demo_receipt.json))

**Purpose:** phone-app BLE demo (Retichat). Not a Retinue capability claim;
Retinue firmware carries no BLE transport as of this date.

Manual check at 115200 baud after the flash, all observed:

- `HOPSPOT_HELTECV4 boot version=0.3.4 commit=fba40b292422`
- `OLED initialized`
- `RNS_ESPNOW interface up, policy Fixed(Channel(6))`
- `boot-stage phase=22 stage=bluetooth.begin` then `phase=23
  stage=bluetooth.ready`; partition table carries `ble_id` at `0xC000`
- `wifi-config ... station=false`, `network_stack=false` (expected without
  provisioning; no SoftAP claim, matching the 2026-08-10 finding)
- `state restored` with zero seeded route records

Not observed in the boot console: any LoRa interface line. Do not claim
on-air LoRa from this receipt; the demo claim is BLE only, proven when a
phone actually pairs.

COM6 remains on `retinue.heltec-v4` 0.0.1 (US915, rnode). Restore path for
COM7 is the proven 2026-08-10 route: `linkboy flash COM7
firmware/packages/heltec-v4-current.toml v4@4.2`.
