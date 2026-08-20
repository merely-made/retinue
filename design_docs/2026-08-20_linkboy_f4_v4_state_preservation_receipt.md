# Linkboy F4 V4 state-preservation receipt

**Date:** 2026-08-20

**Host:** Windows 11 Home Insider Preview 10.0.26220

**Status:** paired physical identity/settings preservation fact complete

## Reproducible package boundary

The receipt used the public Windows stage assembled on 2026-08-19. Its exact
artifacts were:

| Artifact | SHA-256 |
| --- | --- |
| `linkboy.exe` | `fdc7ebc04de0bc87218e6bf9cbb1f021b95f538afc141f7d430d2a2065f2d38c` |
| `firmware/packages/heltec-v4-current.toml` | `aba2b8aaf5260e54bd33c786ffd2b56f103c96d338b689172fec955c4b1f0893` |
| `firmware/heltec-v4-phy/tulle-heltec-v4-phy` | `7f5680ee0eb9a8d3a68eda62cd7f47b098ecb24f8096ce10d0f536a2d175fa7a` |
| `helpers/windows-x86_64/espflash.exe` | `0cc03364c70a86325236f18ad1aaed17eedf267d89312c0cdabe4964f5cb758e` |

The manifest marks the state impact `preserved`, writes
`0x000000..0x3f0000`, and preserves `0x3f0000..0x400000`, where the V4
identity/settings A/B records live.

## Paired physical facts

A non-writing ESP ROM board-info cycle first recovered the V4 on `COM7` from
the documented post-session Windows serial state. It reported ESP32-S3,
16 MiB flash, and MAC `44:1b:f6:6a:fa:64`. Linkboy then asked the running
application for `status`, `region`, and `channel` in one serial session.

| Fact | Before write | After write |
| --- | --- | --- |
| ESP MAC | `44:1b:f6:6a:fa:64` | `44:1b:f6:6a:fa:64` |
| processor / flash | ESP32-S3 / 16 MiB | ESP32-S3 / 16 MiB |
| persisted record | `identity=loaded slot=B seq=5` | `identity=loaded slot=B seq=5` |
| region | `US915` | `US915` |
| channel | `modem` | `modem` |
| application | Heltec V4 `0.0.1` | Heltec V4 `0.0.1` |

The matching MAC establishes that both probes reached the same carrier. The
matching valid A/B slot and sequence establish that the same persisted record
survived, while the matching region and channel expose its applied settings.
The firmware does not export the secret identity body, and this public receipt
does not add it.

## Physical execution

From the staged artifact root:

```text
linkboy.exe flash COM7 firmware/packages/heltec-v4-current.toml v4@4.2 \
  --receipt C:\t\retinue-f4-preservation-20260820\flash-receipt.json
```

Linkboy performed the non-writing hardware inspection, erased and wrote the
admitted application range, verified transfer, rebooted, rediscovered the
application, and emitted `complete`. The retained
[transaction receipt](2026-08-20_linkboy_f4_v4_state_preservation_receipt.json)
has SHA-256
`949cd4d77f73e06aa55dc73001efae99c17e0fc69eb94d75b5072eb09bd7eb50`.

An independent post-write ESP ROM board-info cycle and serial probe produced
the right-hand facts above. This is therefore a paired physical preservation
receipt, not an inference from the manifest's address ranges or Linkboy's
transfer success.

## Boundary

This closes F4's identity/settings preservation claim for one physical V4. It
does not close the factory-as-shipped F3 claim or V4 recovery through the
board's own reset/boot controls; those remain the two open physical flashing
claims.
