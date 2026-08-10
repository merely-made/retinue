# Hopspot V4 COM7 interface receipt

**Date:** 2026-08-10

**Board:** Heltec V4 revision 4.2 on COM7

**Package:** `prns.hopspot.heltec-v4` 0.3.4

The preceding Linkboy transfer receipt records all three signed-release parts, the exact
`espflash` 4.5.0 executable digest, and preservation of `0xD000..0xE000`.

At 115200 baud after the helper reset, COM7 reported:

- `HOPSPOT_HELTECV4 boot version=0.3.4`
- `OLED initialized`
- `RNS_ESPNOW interface up, policy Fixed(Channel(6))`

It also reported `wifi-config ... station=false` and `Wi-Fi initialized ... network_stack=false`.
Two Windows Wi-Fi scans did not expose a `Hopspot-XXXX` SSID. The physical interface proof is
therefore the upstream boot console and initialized OLED/ESP-NOW runtime, not a SoftAP claim.
The package's manual check now names that fact explicitly.

Linkboy then restored `retinue.heltec-v4` 0.0.1 on the same COM7 board. Its receipt records the
same V4 hardware facts and exact helper digest; the direct post-write status reported `US915`,
`rnode`, and `identity=loaded slot=A seq=4`.

Signalman was launched with `SIGNALMAN_SERIAL_PORTS=COM7` and rendered COM7 as a silent external
image, which is the expected recovery entry. This host did not deliver mouse or keyboard input to
that live Cambium window, and its UI Automation button patterns were unavailable, so the actual
COM7 restore used Linkboy rather than being misrepresented as a graphical receipt. The graphical
per-board restore acceptance remains open.
