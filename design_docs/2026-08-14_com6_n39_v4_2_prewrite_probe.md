# COM6 Meshnology N39 V4.2 pre-write probe

**Date:** 2026-08-14  
**Board:** COM6, selected as the owner's Meshnology N39 WiFi LoRa 32 V4 kit  
**Result:** compatible immutable plan reached; no package approval or flash occurred.

Signalman first found COM6 silent and did not infer a carrier revision from that
serial location. The owner selected the named N39 V4.2 product profile, whose
source is recorded in
[`2026-08-14_meshnology_n39_v4_2_profile_evidence.md`](2026-08-14_meshnology_n39_v4_2_profile_evidence.md).

It then ran only Linkboy's non-writing ESP ROM query:

```text
espflash board-info -p COM6 --before default-reset --after hard-reset --non-interactive
```

The result admitted COM6 to the firmware chooser. Selecting `retinue.heltec-v4`
then reached the immutable review, which establishes that the probe supplied the
package target's required ESP32-S3, 16 MiB flash, and `esp-rom` facts. The review
showed the ESP ROM route, `espflash 4.5.0`, the write range
`0x00000000..0x003f0000`, and the preserved range
`0x003f0000..0x00400000`.

`Approve these changes` was not activated. No package helper wrote to COM6, no
V4 application was verified, and no flash receipt exists from this step.
