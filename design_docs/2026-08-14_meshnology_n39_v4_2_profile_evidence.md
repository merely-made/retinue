# Meshnology N39 V4.2 product-profile evidence

**Date:** 2026-08-14  
**Scope:** pre-write carrier-revision evidence for the owner's purchased N39 kit.

The owner's [purchase listing](https://www.amazon.com/dp/B0GQ3BD4RQ?th=1)
identifies the kit as a two-pack of ESP32 LoRa V4 boards with SX1262 and 16 MB
flash. The [Meshnology N39 documentation](https://wiki.meshnology.com/N39/Meshnology%20N39/)
identifies the N39 hardware as ESP32-S3R2/SX1262 with 2 MB PSRAM and 16 MB flash,
and links its V4.2 schematic.

Signalman therefore offers an explicit **Use Meshnology N39 V4.2 profile**
selection. It records this named product and documentation URL in Linkboy's
immutable plan and in schema-4 receipts. It is not a family-wide claim for
every Heltec V4 carrier and does not establish V4.3 compatibility.

The product profile does not substitute for the non-writing ESP ROM probe:
that probe must still report the required ESP32-S3 processor, 16 MB flash, and
`esp-rom` bootloader before Linkboy will plan a package. Nor does this note
approve a flash or demonstrate a V4 physical recovery. Those remain separate
hardware gates.
