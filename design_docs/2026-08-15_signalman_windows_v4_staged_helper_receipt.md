# Signalman Windows V4 staged-helper receipt

**Date:** 2026-08-15  
**Host:** Windows development machine  
**Board:** Meshnology N39 V4.2 on `COM6`  
**Status:** physical Windows V4 staging evidence. F5 remains open.

## Staged artifact

The reproducible staging script assembled
`C:\t\retinue-signalman-v4-stage-20260815` from the locked desktop binary,
the V4-only package index, firmware parts, notices, and a copied `espflash`.
Its `stage.json` records:

| Field | Value |
| --- | --- |
| helper | `espflash 4.5.0` |
| helper SHA-256 | `768f0adfc71629a1e2e690923dd63d267cbfcd2828c26ac1315f664bca1dffc7` |
| installed helper path | `helpers/windows-x86_64/espflash.exe` |
| package identifiers | `retinue.heltec-v4`, `prns.hopspot.heltec-v4` |
| deliberately excluded | `retinue.t114`, `meshtastic.heltec-mesh-node-t114` |

The staged helper's independently computed SHA-256 matched the recorded digest.
The stage was launched with `SIGNALMAN_SERIAL_PORTS=COM6` only. The process had
no `LINKBOY_HELPER_DIR`, `LINKBOY_ALLOW_PATH_HELPERS`, or
`SIGNALMAN_CATALOG_PATH` override. Signalman's installed-catalog lookup showed
only the two V4 packages, and Linkboy resolved the adjacent helper rather than
an ambient `PATH` program.

## Physical loop

1. Signalman selected the silent `COM6`, declared the N39 V4.2 profile, and
   completed the non-writing ESP ROM probe.
2. The stage reviewed and installed `prns.hopspot.heltec-v4` version `0.3.4`.
   Its plan wrote the sparse firmware parts and preserved `0xd000..0xe000`.
3. The required external 115200-baud serial check reported
   `HOPSPOT_HELTECV4 boot version=0.3.4`, OLED initialization, and the fixed
   channel-6 ESPNOW interface. It reported `network_stack=false`, as expected;
   no SoftAP behavior is claimed.
4. The same stage then reviewed and restored `retinue.heltec-v4`
   `0.0.1-current`, writing `0x000000..0x3f0000` and preserving
   `0x3f0000..0x400000`. Its terminal result was `Verified`, `Complete`, and
   `Heltec V4 0.0.1`.

The board is therefore back on Retinue.

## Accessibility witness

Windows Narrator traversed the actual staged build by keyboard through the
detected COM6 row, Board revision edit, Rescan, Use this device, V4 choice,
catalog items, Review this firmware, and the non-writing review boundary's
Approve these changes control. The final control was focused but not activated.
This proves a headed Narrator traversal exists; only a listener can judge the
spoken wording and order. On 2026-08-19 the owner confirmed that the screen
reader worked in that pass. That supplies the manual quality judgement for this
Windows flow; it does not substitute for accessibility checks on another host.

## Boundary

This is not a public installer receipt. It does not prove macOS or Linux, does
not distribute the T114 DFU helper, and does not promote either package into a
public catalog. F5 closes only after those remaining helper-policy and
real-device receipts exist.
