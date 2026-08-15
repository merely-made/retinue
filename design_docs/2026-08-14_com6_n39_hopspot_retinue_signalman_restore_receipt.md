# COM6 Meshnology N39 Hopspot-to-Retinue Signalman restore receipt

**Date:** 2026-08-14  
**Host:** Windows  
**Result:** Complete graphical restoration from Prns Hopspot to Retinue.

## Recovery context

- COM6 had just passed the documented Hopspot 0.3.4 serial self-check recorded
  in `2026-08-14_com6_n39_hopspot_signalman_graphical_receipt.md`.
- Signalman consequently presented COM6 as silent rather than falsely naming
  an external application. The owner selected its V4 family and the named
  **Meshnology N39 V4.2** product profile.
- Signalman ran its non-writing ESP ROM board inspection before it offered
  packages. The observation satisfied the Retinue V4 target's ESP32-S3, 16 MiB
  flash, and `esp-rom` requirements.

## Immutable restore plan

- The reviewed package was `retinue.heltec-v4` version `0.0.1-current`,
  publisher `Merely Made`, source `firmware/heltec-v4-phy`, and application
  container SHA-256
  `7f5680ee0eb9a8d3a68eda62cd7f47b098ecb24f8096ce10d0f536a2d175fa7a`.
- The reviewed route was ESP ROM through `espflash 4.5.0`.
- The plan wrote `0x00000000..0x003f0000` and preserved
  `0x003f0000..0x00400000`. Signalman displayed the state impact as
  **Preserved — your settings and keys survive**.

## Terminal verification

1. The owner approved the displayed plan and activated Signalman's explicit
   **Start installing** control for `serial:COM6`.
2. Signalman reached its graphical **Verified** terminal page with result
   **Complete**.
3. The returned application reported **Heltec V4 0.0.1** and retained package
   id `retinue.heltec-v4`, its container hash, board revision 4.2, and the
   documented N39 profile evidence.

This is a complete graphical cross-firmware recovery receipt. It proves the
returned Retinue application, not any external Hopspot persistent-state claim.
