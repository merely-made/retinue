# Linkboy notices

An installed build resolves a package helper from its own
`helpers/<os>-<arch>` directory and verifies its version and manifest-pinned digest before any
write. A developer may set `LINKBOY_HELPER_DIR` for a staging run or explicitly opt into ambient
`PATH` with `LINKBOY_ALLOW_PATH_HELPERS=1`; neither is a public-install instruction.

| Helper | Pinned version | License reported by the installed distribution | Source |
| --- | --- | --- | --- |
| `espflash` | 4.5.0 | MIT OR Apache-2.0 | https://github.com/esp-rs/espflash |
| `adafruit-nrfutil` | 0.5.3.post16 | Nordic Semiconductor proprietary license | https://github.com/adafruit/Adafruit_nRF52_nrfutil |

`adafruit-nrfutil` is not bundled by this repository. Its reported license requires a separate
redistribution review before a packaged public build can ship it. `espflash` has a permissive
license, but its release bundle and the resulting Windows, macOS, and Linux receipts are separate
evidence from this notice.

The Linkboy and Retinue workspace code remains under the repository's MPL-2.0 terms. This file
does not grant redistribution rights for either helper.
