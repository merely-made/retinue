# AIR3 T114 on-air bounded-state receipt

**Date:** 2026-08-13  
**Scope:** AIR3's native-node bounded transport state and heap-peak measurement on
the owner-confirmed Heltec T114 (`TULLE-T114-01`), with a Heltec V4 as the
independent physical radio.

This is a hardware receipt. It does not promote a desk fixture to an RF result.

## Firmware and target

- package: `retinue.t114` `0.0.1-v51`
- payload: `firmware/t114-phy/tulle-t114-phy-v51.zip`
- payload SHA-256:
  `f40319be88937315c9f8e7db47d264da869d205f08e2d84ecfc975d749eb5a56`
- application write range: `0x26000..0x693c2`
- preserved ranges: `0xea000..0xec000` and `0xec000..0x100000`
- loader record: `HT-n5262`, nRF52840, 1 MiB flash, UF2 bootloader `0.9.0`,
  SoftDevice `S140 6.1.1`; see
  [`2026-08-13_t114_air3_loader_snapshot.json`](2026-08-13_t114_air3_loader_snapshot.json).
- route: `adafruit-nrfutil dfu serial` on the reset T114's `COM4`; it reported
  `Activating new firmware` and `Device programmed.` The application returned
  on `COM10` as `T114 region=US915 channel=node`.

The native node reported `transport=1` before the flood. The V4 source was the
known `COM6` Heltec V4, switched temporarily from `rnode` to `modem`; both
boards used US915 at 906875000 Hz, SF11, 250 kHz bandwidth, coding rate 4/5,
and sync word `0x2b`.

## Sustained flood

`node_stress COM6 flood-series 0 3 30` held one V4 direct-PHY host session and
sent three disjoint, signed forty-identity waves: `0..39`, `40..79`, and
`80..119`. The 30-second gaps made it possible to query the T114 without
reopening the source serial session.

| Observation | T114 native-node state | Heap result |
| --- | --- | --- |
| Before flood | `tx=1`, `rx=0`, `peers=0`, `routes=0`, `transport=1` | `highwater=332/49152` |
| After wave 1 | `tx=15`, `rx=14`, `peers=14`, `routes=14`, `fwdannounce=14` | `highwater=11112/49152`, live `10632` |
| After wave 2 | `tx=29`, `rx=28`, `peers=28`, `routes=16`, `fwdannounce=28`, `routeevicted=12` | `highwater=18168/49152`, live `17688` |
| During wave 3 after table saturation | `tx=33`, `rx=60`, `peers=32`, `refusedpeers=28`, `routes=16`, `fwdannounce=32`, `routeevicted=16` | unchanged: `highwater=18168/49152`, live `17688` |

The source completed its third wave after the last row. Once the address book
was full, the remaining fresh identities were refused. The running T114 sample
already shows the full peer and route bounds, 28 refusals, and the unchanged
peak while those refusals were arriving. A final post-close T114 query was not
available: its CDC serial endpoint timed out after the source session closed.
That missing read is recorded below rather than reconstructed from the model.

## Independent relay observation

The V4's direct-PHY receive queue was drained after each source wave. It saw:

| Source wave | Received frames | Header-type-2 relays with hop count 1 and a transport identity |
| --- | ---: | ---: |
| `0..39` | 0 | 0 |
| `40..79` | 1 | 1 |
| `80..119` | 1 | 1 |

The first zero is expected from a two-radio, half-duplex test: the V4 begins
its next transmission as the T114 starts relaying. Waves two and three include
an independently observed T114 type-2, hop-one relay. The T114's own
`fwdannounce=32` and `tx=33` counters agree, with one transmit being its boot
announce.

## Result

AIR3's required on-air bounded-state receipt is met:

- the native T114 transit mode relayed valid signed announces;
- physical observation saw type-2, hop-one traffic from the relay;
- routes stayed at their sixteen-entry limit and evicted live entries;
- the address book stayed at its thirty-two-peer limit and refused excess;
- peak heap stabilized at 18,168 bytes, leaving 30,984 bytes of the fixed
  49,152-byte heap free; and
- the source and target stayed up through three sustained waves in one T114
  boot.

This does **not** yet provide the stronger three-part transaction: a link
request forwarded to a destination and its returned proof. That is transport
coverage beyond AIR3's announce-relay/firmware-state gate. It is specified in
[the transit link receipt spec](2026-08-14_transit_link_receipt_spec.md),
which found the tooling gap smaller than expected (`node_link` already drives
a direct-PHY link, `link_peer` already has the responder role) and the
topology requirement larger: three radios in one room hear each other, so the
claim needs a relay-off control, not just a completed link. The final post-wave
three T114 serial read is also absent because the CDC endpoint stopped accepting
writes after the source session; it does not alter the measured bounded state
above, but it remains a T114 host-reliability follow-up.

## Bench restoration

After the receipt, the T114 was reset into its own DFU loader, reflashed with
the same v51 image, and switched from `node` back to its prior `modem` channel.
`COM10` then reported `T114 region=US915 channel=modem`, `channel=modem`, and
`heap=0/49152 highwater=24 free=49152`.

The V4 received `channel rnode` and reset. Its port then ceased answering the
text survey, which is the expected RNode startup posture: that channel writes
no text banner and expects KISS from its first byte. The setting command's
write completed, but its courtesy reply was lost to the immediate reset, so
the restored V4 state is inferred from that reset plus its binary-only port,
not falsely reported as a text-probe confirmation.
