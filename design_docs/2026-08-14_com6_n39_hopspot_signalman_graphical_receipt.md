# COM6 Meshnology N39 Hopspot Signalman graphical receipt

**Date:** 2026-08-14  
**Host:** Windows  
**Result:** Signalman transferred the signed Prns Hopspot V4 package. Its
required external serial check passed. This is not a Retichat or SoftAP receipt.

## Carrier and plan evidence

- The owner selected the named **Meshnology N39 WiFi LoRa 32 V4 kit** profile,
  revision **4.2**. The profile records the documented product source rather
  than inferring a board revision from COM6 or USB identity.
- Signalman's non-writing ESP ROM inspection admitted the selected silent COM6
  device as an ESP32-S3 with 16 MiB flash on the `esp-rom` route.
- The immutable review selected `prns.hopspot.heltec-v4` version `0.3.4`,
  publisher `Prns`, source revision
  `fba40b292422d04614f4fbeb7427bac1a12fc8d5`, and the minisign key
  `1BF5C4D8B140811A`.
- The verified signed manifest SHA-256 was
  `e954ed3a5c94990f5ee2c074d7521e41cec803718d3dafaf123cdf14f0abcf0a`.
  Signalman used `espflash 4.5.0`.
- The plan wrote the signed bootloader at `0x00000000`, partition table at
  `0x00008000`, and application at `0x00010000`. It preserved the Prns
  provisioning range `0x0000d000..0x0000e000`; package state impact remained
  explicitly `unknown`.

## Graphical transfer

1. The owner approved the immutable plan and activated **Start installing** for
   `serial:COM6`.
2. Signalman completed its verified transfer, then correctly stopped at
   **Manual check required**. It did not promote the helper's success to an
   application-identity claim.
3. The terminal page requested the package's documented 115200-baud self-check.

## External firmware self-check

With Signalman closed, a read-only 115200-baud COM6 console captured a new
Hopspot boot. It reported all package-required facts:

- `HOPSPOT_HELTECV4 boot version=0.3.4`;
- `OLED initialized`;
- `RNS_ESPNOW interface up, policy Fixed(Channel(6))`.

It also reported `network_stack=false` with no configured Wi-Fi. This receipt
does not claim a SoftAP. The serial console open caused the observed
`USB_UART_CHIP_RESET`; no case disassembly or provisioning-range write was used.

The subsequent Retinue restore is recorded separately in
`2026-08-14_com6_n39_hopspot_retinue_signalman_restore_receipt.md`.
