# LE3 T114 scan-physics receipt

**Date:** 2026-08-20

**Status:** LE3a/LE3b physical scan slice complete; unattended rotating-scheduler
miss-rate calibration remains outside this receipt

## Bench and artifacts

- listener: T114 on `COM10`, application USB `1915:521f`, US915 at
  906.875 MHz
- Meshtastic-sync transmitter: Heltec V4 on `COM6`, MAC
  `44:1b:f6:6a:fb:28`
- MeshCore-sync transmitter: Heltec V4 on `COM7`, MAC
  `44:1b:f6:6a:fa:64`
- T114 serial-DFU package: `firmware/t114-phy/tulle-t114-phy-v56.zip`,
  SHA-256 `a69c36f0a83eb2691a9c203fa3d58b7c32aac38658329f5d2c6e5432cc11327b`
- T114 application binary: `firmware/t114-phy/tulle-t114-phy-v56.bin`,
  SHA-256 `7f2b21353cb89c9a1e9e3cc8004a0feff1a3d7aa4652113dd4188677903ec1fa`
- V4 ELF: `firmware/heltec-v4-phy/tulle-heltec-v4-phy`, SHA-256
  `bd2e59a53bdc31d23a932a9cc219a0070cc32c6d5aac650a561faacfffc7b517`

`adafruit-nrfutil` reported `Activating new firmware` and `Device
programmed.` for the T114. `espflash 4.5.0` identified both V4s as ESP32-S3
with 16 MiB flash and wrote a 191,024-byte application/partition image.
Post-write probes showed the same persisted state as before the write:
`COM6` remained `identity=loaded slot=B seq=9`, and `COM7` remained
`identity=loaded slot=B seq=5`; both remained `region=US915 channel=modem`.

## Admitted plan

The T114's runtime consumer admitted this fixed registry:

```text
le3 plan detections=2 receives=3 steps=5 dwell=2840ms budget=3000ms fits=1 overfull_rejected=1 sequence=d1,r1-12,r2-2b,d2,r3-2b
```

`D1` is SF11/BW250. `R1` and `R2` share it but hold separate `0x12` and
`0x2b` packet configurations. `D2` is SF9/BW250 and `R3` is its exact `0x2b`
capture profile. Rounded measured slots are 100 ms for D1 CAD, 40 ms for D2
CAD, and 900 ms for each receive window. The deliberately smaller budget is
rejected at runtime rather than silently dropping coverage.

## Physical result

The reproducible host command was:

```text
cargo run -p tulle --features serial-async --example le3_scan_probe --locked --offline -- COM10 COM6 COM7
```

The final continuous run reported:

```text
le3 rx id=1 result=capture len=64 rssi=-91 snr=0 apply=335us handoff=12603us acquisition=709411us dwell=900ms restored=1
le3 cad id=1 hits=11 misses=1 faults=0 apply_avg=366us retune_avg=10620us cad_avg=83353us symbols=8 restored=1
le3 cad id=2 hits=0 misses=12 faults=0 apply_avg=368us retune_avg=1917us cad_avg=30110us symbols=8 restored=1
le3 rx id=1 result=miss len=0 rssi=0 snr=0 apply=9796us handoff=12725us acquisition=900054us dwell=900ms restored=1
le3 rx id=2 result=capture len=64 rssi=-92 snr=4 apply=366us handoff=12603us acquisition=702880us dwell=900ms restored=1
le3 cad id=2 hits=4 misses=8 faults=0 apply_avg=366us retune_avg=1935us cad_avg=30092us symbols=8 restored=1
le3 cad id=1 hits=0 misses=12 faults=0 apply_avg=366us retune_avg=10620us cad_avg=83355us symbols=8 restored=1
le3 rx id=3 result=capture len=64 rssi=-91 snr=6 apply=366us handoff=12603us acquisition=187194us dwell=900ms restored=1
air region=US915 duty=0ms listen=off armed=28 armfail=0 rxok=6 rxerr=0 rxbad=0 txok=0 txerr=0 noregion=0 overduty=0 cadclear=0 cadbusy=0 cadgiveup=0 cadover=0 cadfault=0 beats=0 frames=0
scan cad1=23/37 cad2=8/40 rx1=2/3 rx2=2/0 rx3=2/0
LE3 SCAN PHYSICS PASSED
```

The scan counters are cumulative across the two v56 attempts. The first
attempt completed every COM6 fact before its late COM7 witness timed out; the
successful run moved that witness first. The counters therefore retain two
captures for every exact profile and the extra bounded `R1` miss from the
abandoned late window.

The important physical distinctions are direct:

- D1 sees SF11 traffic while D2 mostly or entirely misses it.
- D2 sees SF9 traffic while D1 misses it.
- a fixed `0x12` window rejects the otherwise-matching `0x2b` frame for the
  entire 900 ms dwell.
- the immediately following `0x2b` window captures that profile, and separate
  exact windows capture SF9/`0x2b` and SF11/`0x12`.
- every operation restored the resident profile, with zero CAD faults, RX arm
  failures, or RX errors in the final diagnostics.

## Defects paid for

The bench found two receive-loop defects rather than hiding them with resets.
The SX126x driver previously kept `rx_collect` inside a second IRQ wait after
a preamble-only or header-damaged event. A partial mismatched frame could
therefore strand the host loop forever. Collection now consumes exactly one
IRQ: preamble-only activity returns `ReceivePending`, header and CRC damage
return explicit benign errors, and the outer loop regains control.

An exact receipt window now continues through those non-captures until a valid
frame arrives or its declared dwell expires. That is why the `0x12` versus
`0x2b` case reports a bounded miss instead of either a false capture or an
early damaged result.

The V4 direct-PHY host pump also gained acknowledged live profile
reconfiguration. COM6 changed from SF11 to SF9 without closing its one-shot
native-USB session. A dedicated pump test verifies that the next transmission
uses the new profile's airtime model.

## Software verification

```text
cargo test -p radio-hand -p tulle --features tulle/serial-async --locked --offline
```

Passed 107 unit and device tests: 53 in `radio-hand` and 54 in `tulle`.
The T114 release build for `thumbv7em-none-eabihf` passed. The V4 Espressif
release build for `xtensa-esp32s3-none-elf` passed with four existing
feature-gated dead-code warnings in its power/wake instrumentation.

## Boundary

This closes the physical LE3a/LE3b scan slice: two detection groups, three
exact receive profiles, measured CAD/retune/handoff/acquisition, a runtime
cycle-budget refusal, counted misses, and exact cross-sync captures on real
radios. It does not claim that the current boot-selected modem personality is
the final unattended multi-protocol executive, nor does this short bench
calibrate long-run per-profile miss probability. Those remain work for the
resident scheduler and lease gates rather than facts inferred from this
receipt.
