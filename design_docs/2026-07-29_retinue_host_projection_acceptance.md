# Retinue headed host-projection acceptance

**Date:** 2026-07-29
**Status:** complete; headed RF and fitted-panel receipts passed
**Plan rung:** U4 in `2026-07-28_on_device_ui_implementation_plan.md`

## Boundary

The radio remains a modem. Retinue and Outrider own identity, IFAC, peers,
links, delivery, and propagation on the host. `radio-face::HostSnapshot` is a
bounded display projection of those facts, not a second protocol model.

`DirectPhyUiControl` is cloneable because the Retinue packet pump owns
`DirectPhySerialLink` during a live session. The control handle can publish
opaque snapshot bytes and nothing else. Tulle still has no Retinue, Outrider,
or display-schema dependency.

## Bench

- COM6: Heltec WiFi LoRa 32 V4
- COM10: Heltec Mesh Node T114 with fitted ST7789
- T114 production v15 UI firmware
- 906.875 MHz, BW 250 kHz, SF8, CR 4/5, 17 dBm
- sync word `0x12`, preamble 16, explicit header, CRC enabled
- eight-byte IFAC for the authenticated phases
- Resource timeout 180 seconds, retry 3 seconds, request window 1
- cost-8 direct and propagation stamps

Command:

```text
cargo run -p outrider --example direct_phy_ui --features tulle-radio -- COM6 COM10 250 180 0
```

The optional final argument holds every published snapshot for that many
seconds during physical inspection:

```text
cargo run -p outrider --example direct_phy_ui --features tulle-radio -- COM6 COM10 250 180 0 20
```

## RF receipt

```text
ui: minimal open-interface projection accepted on both radios
radios online: COM6=left, COM10=right; interface=IFAC authenticated
discovery: IFAC-authenticated delivery announces crossed RF
host failure: unannounced destination rejected without a local radio fault
direct delivery: authenticated Data message passed and UI receipt published
propagation phase: fresh IFAC pair and delivery identities ready
discovery: propagation-node announce crossed RF
propagation batch: 286 packed bytes
propagation submit: sender starting
propagation submit: link admitted on interface 0
propagation submit: payload received as Resource
propagation submit: sender completed as Resource
propagation submit: UI admission receipt accepted
propagation storage: inserted=1 entries=1 bytes=240
propagation fetch: offered=1 served=1 authenticated; UI receipt published
OUTRIDER DIRECT-PHY UI HEADED PASSED
```

The direct receipt verifies source identity, signature, cost-8 stamp, message
id, title, and content before publishing `DIRECT DELIVERED`.

The propagation receipt verifies the submitted transient id, stores exactly
one encrypted entry, serves it through the real propagation request grammar,
then verifies source identity, signature, title, content, and transient id
before publishing `PROP FETCHED`.

## Display distinctions

| Distinction | Evidence projected |
| --- | --- |
| modem online / link admitted | firmware-owned online state / accepted Retinue Resource session |
| open / IFAC | minimal `INTERFACE OPEN` / named `IFAC ENABLED` and `IfacState::On` |
| direct / propagation | `DIRECT DELIVERED` / `PROP STORED` and `PROP FETCHED` |
| radio fault / host failure | firmware FAULT receipt from U2 / `NotFound` mapped to `DELIVERY FAILED` |
| minimal / named | no node or peers / bounded node identity and direct peer |

The event strings are labels for completed operations. They are not injected
acceptance fixtures: each is published only after its corresponding live
state transition or authenticated receipt. A snapshot carries one current
event. TRAFFIC and LINKS render it as a bottom ticker; the firmware does not
retain host-event scrollback.

## IFAC Resource defect found

The first two propagation attempts admitted the Resource link but timed out
receiving its payload. A reduced 286-byte Resource passed both ways on an open
carrier. Under IFAC, a packet trace showed valid advertisement and request
exchange, and the sender matched repeated requested parts, but no part reached
the radio queue.

The cause was `Endpoint::set_link_mtu`: the requested logical MTU of 247 was
silently clamped back to 255. Resource parts therefore occupied 255 logical
bytes, then the eight-byte IFAC envelope made them 263 bytes on a carrier
capped at 255. Queue admission correctly refused them.

Retinue now permits the headed 247-byte link MTU. A Tulle in-memory carrier
test pins a 286-byte Resource at the exact 247 logical plus eight IFAC
boundary. Resource timeout errors also distinguish no request, unmatched
parts, and parts served without a final proof.

## Automated receipts

- Retinue all-features passed 95 library tests, every integration lane, and
  its doctest, including the exact IFAC carrier regression
- Outrider all-features passed 21 library and nine integration tests
- Tulle passed 36 unit and five capture tests; `radio-face` passed 17 tests
- strict no-dependency Clippy passed for Retinue, Outrider, and Tulle
- formatting and `git diff --check` passed

## Physical receipt

The first rapid run exposed a presentation problem rather than a protocol
failure: `HostSnapshot` retains one current event, so later snapshots replaced
earlier ticker text before it could be inspected. Resetting the T114 also
cleared the snapshot and stopped the serial refresher.

The headed harness now accepts an optional staging interval. A repeated run
held every real snapshot for 20 seconds. On the fitted T114, the bottom ticker
visibly showed the staged host events and the LINKS body showed `IFAC ON`.
The final `PROP FETCHED` snapshot was then refreshed until observation was
confirmed. The refresher was stopped afterward, releasing COM6 and COM10.

## Evidence boundary

This proves real one-hop direct delivery and propagation submit/store/fetch
through Retinue, Outrider, IFAC, Tulle, and both production radios. It does not
measure range, power, multi-hop forwarding, loss rate, or stock-node RF
interoperability.
