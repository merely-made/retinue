# Linkboy notices

The current public flashing slice invokes these helpers from `PATH`. Package manifests pin the
versions they were measured against and carry the corresponding source and license facts.

| Helper | Pinned version | License reported by the installed distribution | Source |
| --- | --- | --- | --- |
| `espflash` | 4.5.0 | MIT OR Apache-2.0 | https://github.com/esp-rs/espflash |
| `adafruit-nrfutil` | 0.5.3.post16 | Nordic Semiconductor proprietary license | https://github.com/adafruit/Adafruit_nRF52_nrfutil |

`adafruit-nrfutil` is not bundled by this repository. Its reported license requires a separate
redistribution review before a packaged public build can ship it. `espflash` has a permissive
license, but bundling and the resulting Windows, macOS, and Linux receipts remain unmeasured.

The Linkboy and Retinue workspace code remains under the repository's MPL-2.0 terms. This file
does not grant redistribution rights for either helper.
