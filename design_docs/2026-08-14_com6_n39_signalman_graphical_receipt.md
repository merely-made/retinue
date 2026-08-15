# COM6 Meshnology N39 Signalman graphical receipt

**Date:** 2026-08-14  
**Host:** Windows  
**Result:** complete graphical Retinue installation and application verification.

## Carrier evidence

- The owner selected **Meshnology N39 WiFi LoRa 32 V4 kit**, revision **4.2**,
  through Signalman's named product-profile action.
- The immutable review and final verified page both displayed the documented
  product-profile source. See
  [`2026-08-14_meshnology_n39_v4_2_profile_evidence.md`](2026-08-14_meshnology_n39_v4_2_profile_evidence.md).
- No case disassembly was used. COM number and USB identity were not used as
  carrier-revision evidence.

## Pre-write facts

- COM6 initially answered `HeltecV4`, `US915`, and `modem`.
- Signalman ran its non-writing ESP ROM `board-info` query before admitting the
  board to the package chooser. The resulting observation satisfied the
  Retinue V4 target's ESP32-S3, 16 MiB flash, and `esp-rom` requirements.
- The review selected `retinue.heltec-v4` version `0.0.1-current`, through
  `espflash 4.5.0`. Its verified application-container SHA-256 was
  `7f5680ee0eb9a8d3a68eda62cd7f47b098ecb24f8096ce10d0f536a2d175fa7a`.
- The immutable plan wrote `0x00000000..0x003f0000` and preserved
  `0x003f0000..0x00400000`.

## Execution and verification

1. The owner approved the displayed immutable plan and activated Signalman's
   explicit **Start installing** control for `serial:COM6`.
2. Signalman reached its graphical **Verified** terminal page with result
   **Complete**.
3. The returned application reported **Heltec V4 0.0.1**. Linkboy only emits
   this terminal result after it has re-identified the application and checked
   its board family and runtime version; any reported region or channel must
   also be within the package's declared capabilities.
4. The terminal page retained the package id, artifact hash, V4.2 selection,
   and documented N39 product-profile evidence.

The terminal page does not render the returned region and channel values, so
this receipt does not claim a fresh post-write region/channel reading beyond
the `Complete` verification result. No recovery action is open.
