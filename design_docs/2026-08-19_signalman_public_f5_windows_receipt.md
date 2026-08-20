# Signalman public F5 Windows receipt

**Date:** 2026-08-19

**Host:** Windows 11 Home Insider Preview 10.0.26220

**Source base:** `864645e96f2782675acb1d3d8b827b4f0381bb68`

**Status:** software, headed flow, and physical V4 and T114 loops complete

## Reproducible source boundary

An isolated detached worktree at the source base received only the Linkboy F5
files, package manifests, staging scripts, and generated T114 UF2. This kept an
unfinished concurrent Signalman voice lane out of the receipt. Git's Windows
line-ending conversion changed two retained Prns signed JSON files in that
temporary checkout; replacing them byte-for-byte from the verified primary
checkout restored their original digests and Minisign verification.

The following commands passed with the isolated target directory and no network
resolution:

```text
cargo test --manifest-path apps/linkboy/Cargo.toml --locked --offline
cargo test --manifest-path apps/signalman-desktop/Cargo.toml --locked --offline
```

Linkboy passed 68 unit tests, the retained Prns signature test, and the official
T114 UF2 release test. Signalman desktop passed 10 unit tests, 5 accessibility
tests, 6 management-shell tests, 4 network-face tests, and 13 owner-flow tests.

## Staged artifact

`assemble-public-stage.ps1` created
`C:\t\retinue-signalman-public-stage-20260819-1` for `windows-x86_64`.
`stage.json` names all four public packages, the built-in UF2 route, and the
official `espflash 4.5.0` Windows release custody.

| Artifact | SHA-256 |
| --- | --- |
| `signalman-desktop.exe` | `231290d16000ee202ee6ef1a0589160b24be6db799c9f5a842a9aca645349a0e` |
| `linkboy.exe` | `fdc7ebc04de0bc87218e6bf9cbb1f021b95f538afc141f7d430d2a2065f2d38c` |
| `helpers/windows-x86_64/espflash.exe` | `0cc03364c70a86325236f18ad1aaed17eedf267d89312c0cdabe4964f5cb758e` |
| `firmware/t114-phy/tulle-t114-phy-v51.uf2` | `3b802471f5402f38cf4ab30c39d9acb9a9e893aaf2a588455536b57706452f1b` |

The staged CLI loaded the four-package catalog, inspected both public Retinue
packages, reported the T114 write map as `0x26000..0x69400`, and reported the
bundled helper as `espflash 4.5.0`. The T114 package requires no external helper.

## Headed preflight

The staged `signalman-desktop.exe` launched as an actual Windows window and
discovered the connected silent T114 on `COM10`. Windows UI Automation exposed
named controls for Devices, Network, Messages, Map, Browse, the COM10 candidate,
Board revision, Rescan, Use this device, explicit V4 and T114 declarations, the
mounted UF2 volume, the optional loader-record path, and both T114 routes.

Keyboard input entered revision `2.x`, and the visible T114 declaration exposed
the stock-bootloader UF2 controls. This is a headed packaged-executable receipt,
not a source projection or coordinate-only screenshot.

## Physical Windows V4 leg

O-PC is the Windows host named above. A Heltec V4 revision 4.2 appeared on
`COM7` and first answered with its running Retinue identity. Standalone Linkboy
from this exact stage then planned and installed `prns.hopspot.heltec-v4` 0.3.4
with the adjacent, digest-checked `espflash 4.5.0`. Its
[transfer receipt](2026-08-19_linkboy_f5_windows_v4_hopspot_receipt.json)
records the three package parts, publisher signature, ESP32-S3 and 16 MiB loader
facts, and preservation of `0xd000..0xe000`.

An independent 115200-baud capture after a normal DTR reset reported:

```text
HOPSPOT_HELTECV4 boot version=0.3.4
OLED initialized
RNS_ESPNOW interface up, policy Fixed(Channel(6))
```

It also reported `network_stack=false`, as expected without Wi-Fi provisioning;
this receipt makes no SoftAP claim. The same staged Linkboy then restored
`retinue.heltec-v4`. The
[restore receipt](2026-08-19_linkboy_f5_windows_v4_retinue_receipt.json)
reached `complete` and `application-verified` with Heltec V4 0.0.1, region
`US915`, and channel `rnode`. The board is back on Retinue.

## Physical T114 UF2 leg

The owner double-tapped reset on the attached T114. Its stock bootloader
mounted as the FAT volume `E:\` with label `HT-n5262`; the bootloader serial
face appeared on `COM4`. Signalman retained the explicit loader record at
`C:\t\lr` with SHA-256
`1150e54d4f9832d83761efc48abc7ade9289543f44c5f75ab7cc886a72a13bd0`.
The record states schema 1, model `HT-n5262`, UF2 bootloader `0.9.0`, S140
SoftDevice `6.1.1`, nRF52840, and 1 MiB flash.

The staged graphical flow selected `retinue.t114` `0.0.1-v51`, exposed the
immutable package, helper, write-map, preservation, and recovery facts, and
required a separate approval before writing. The built-in Linkboy UF2 writer
copied all 275,456 admitted bytes to `E:\`, verified the copy, acknowledged the
board reboot, and asked the returned application for its identity. The UF2
volume disappeared as expected and the application returned on `COM10`
(`VID_1915&PID_521F`). Signalman's terminal view reported:

```text
Result: Complete
Running: T114 0.0.1
Package: retinue.t114
Artifact SHA-256: 3b802471f5402f38cf4ab30c39d9acb9a9e893aaf2a588455536b57706452f1b
Board: T114 2.x
Board revision evidence: owner confirmation from carrier marking
```

This is the public T114 real-device receipt required by F5. It proves physical
transfer, reboot, and returned application identity, rather than inferring
success from the filesystem copy.

## Boundary

This proves the reproducible Windows package and headed physical routes for V4
and T114. Together with the retained Intel-macOS, Apple-silicon-macOS, and Linux
V4 receipts, it closes F5. It does not prove signing or an installer format,
which remain later distribution work.
