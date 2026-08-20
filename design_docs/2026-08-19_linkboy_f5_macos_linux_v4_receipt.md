# Linkboy F5 macOS and Linux V4 receipt

**Date:** 2026-08-19

**Status:** physical V4 flash, external Hopspot check, and Retinue recovery
complete on Intel macOS, Apple-silicon macOS, and x86-64 Linux through
standalone staged Linkboy.

## Stages

The source base was `864645e96f2782675acb1d3d8b827b4f0381bb68`, with the
current Distribution working-tree changes included.  Each host received a
fresh `retinue.linkboy-public-stage/v1` directory containing Linkboy, a
digest-checked native `espflash`, the four-package public catalog, firmware
payloads, notices, and license.  Neither run used an ambient helper, Cargo,
Python package, or an installed Signalman catalog.

| Host | Stage platform | Linkboy SHA-256 | `espflash` SHA-256 | Retained archive SHA-256 |
| --- | --- | --- | --- | --- |
| Q-PC, Intel macOS | `macos-x86_64` | `241b748e4e2a24f83b495c7d84d48468e6b98c21cf4c0b8354b4d266f99ebbf8` | `2e6a1d52173f999a4ab6c6f6445038caa28ab143aa2ac9d03965249a257e8844` | `3c5cb664742d883e4304d4fc611fc875b27a8f8d7d105d22da2f615eb36888a0` |
| Mayola's iMac, macOS 26.5.1 | `macos-aarch64` | `5e21af0dedbdeb761495db4edd8da203e5d09b1fa06f5d14e6ee5a36c895cb15` | `ff92f62238a0bd6df543e0400d2b7b2ee4d97e53823d6165a80878134de860d1` | `6614ff70e523a6bce5f4ccc6459b77275f5e7e900429004bb7eec463c95db28a` |
| ThinkPad L14-F, Fedora 44 | `linux-x86_64` | `9591b482b42367cbef8782f55e24df0d88d5ace1b7f326f15dbc9304fa76af58` | `a1b2a325cc6f64de4cb7a5e9b4fa2a0a4b1212555664c7ca50be29c5abb303bf` | `542c5cc81f0cca384cbead1cacb7ccc9f35072a989b2de0fb95333d814272c22` |

All three native helpers reported `espflash 4.5.0`.  The Q-PC helper came from the
official `espflash-x86_64-apple-darwin.zip`; the ThinkPad helper came from
`espflash-x86_64-unknown-linux-musl.zip`.  The iMac used the official
`espflash-aarch64-apple-darwin.zip`.  Its Linkboy executable was cross-built
on Q-PC using Apple's installed SDK, then confirmed as a native arm64 Mach-O
before staging.  The iMac did not need Cargo or any local build tooling.

## Physical loop

Q-PC used `/dev/cu.usbmodem14801`; Mayola's iMac used
`/dev/cu.usbmodem31101`; ThinkPad used `/dev/ttyACM0`.  Every plan selected a
Heltec V4 revision 4.2 on the ESP ROM route, using the owner's carrier-marking
confirmation.  The iMac and ThinkPad also checked the running Retinue
identity, ESP32-S3 processor, 16 MiB flash, and ROM loader before writing.

Each board received `prns.hopspot.heltec-v4` 0.3.4.  The helper erased and
verified its bootloader (`8a516bf82000501f129eb8bf7cd04ec6a33edb09487890beefe90989d806990d`),
partition table (`e187b5a94e4423b42a5d41a02fd39ce1d89dd65c6c2241c14e9ec9786247a9a4`),
and application (`c029b78248c3bd05bde79b82e31160736cddb7f8e1d88ea6b0fdf374c39762b9`).
The package preserved `0xd000..0xe000` and carried its Minisign publisher
record for signed manifest SHA-256
`e954ed3a5c94990f5ee2c074d7521e41cec803718d3dafaf123cdf14f0abcf0a`.

An external 115200-baud serial capture after a normal DTR reset confirmed on
both boards:

```text
HOPSPOT_HELTECV4 boot version=0.3.4
OLED initialized
RNS_ESPNOW interface up, policy Fixed(Channel(6))
```

The boards were then restored with `retinue.heltec-v4`.  In both cases,
`espflash` verified the 4,192,412-byte application
`7f5680ee0eb9a8d3a68eda62cd7f47b098ecb24f8096ce10d0f536a2d175fa7a`,
Linkboy rediscovered the application, and each receipt reached `complete` with
`application-verified`.

| Host | Returned Retinue identity |
| --- | --- |
| Q-PC | Heltec V4, version `0.0.1`, region `US915`, channel `rnode` |
| Mayola's iMac | Heltec V4, version `0.0.1`, region `US915`, channel `rnode` |
| ThinkPad | Heltec V4, version `0.0.1`, region `US915`, channel `modem` |

## Boundary

This is the missing physical V4 evidence for both macOS architectures and
Linux.  It is a standalone Linkboy stage, not a headed Signalman F6 receipt.
The matching Windows V4 loop is recorded in
`2026-08-19_signalman_public_f5_windows_receipt.md`, along with the public T114
real-device receipt that closes F5.
