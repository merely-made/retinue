# Signalman T114 graphical receipt

**Date:** 2026-08-14  
**Host:** Windows  
**Result:** Complete graphical cross-firmware and direct-DFU recovery receipt.
Signalman wrote the verified Retinue package and verified the returned T114
application.

## Hardware evidence

- The mounted stock loader was `E:/`, volume label `HT-n5262`.
- `INFO_UF2.TXT` reported model `HT-n5262`, UF2 bootloader `0.9.0`, and
  SoftDevice `S140 6.1.1`.
- Signalman captured those facts through its mounted-volume face into
  `design_docs/2026-08-14_signalman_t114_loader_snapshot.json`.
- COM6 and COM7 were excluded from the run and were not written.

## Graphical upstream install

1. Signalman selected the owner-confirmed T114 revision 2.x and the mounted
   stock UF2 volume.
2. It correctly refused `retinue.t114` on the UF2 route because that package
   requires the S140 serial-DFU route.
3. It accepted the signed `meshtastic.heltec-mesh-node-t114` package, reviewed
   the UF2 mass-storage plan, wrote the verified artifact, and reported
   `Manual check required` rather than inventing foreign application facts.
4. The board left the mounted volume and re-enumerated as COM3,
   `VID_239A/PID_4405`.

The exact Meshtastic version was not independently read through Meshtastic's
own interface, so this is a verified package-transfer receipt rather than an
upstream application-version receipt.

## Graphical Retinue restore

1. Signalman selected silent COM3 as an owner-confirmed T114 revision 2.x and
   consumed the captured loader record.
2. It reviewed `retinue.t114` version `0.0.1-v51` on the serial DFU
   (`adafruit-nrfutil`) route.
3. The helper entered Nordic DFU on COM10, erased, wrote, verified the transfer,
   and rebooted the board.
4. The first receipt incorrectly reported a 12-second rediscovery timeout.
   Relaunching Signalman immediately identified COM10 as `T114, region US915,
   channel modem`, proving that Retinue had returned.

## Graphical already-in-DFU replay

Signalman now has an explicit `Use selected T114 DFU port` action. It requires
all of the evidence that the ordinary silent-device path requires: the selected
silent port, an owner-declared T114 family and revision, and the loader record
captured from that board. It records `serial-dfu:PORT` as a distinct transport;
Linkboy therefore skips application bootloader entry without pretending that
the COM number identifies the board.

The physical replay selected silent COM10, declared T114 revision 2.x, loaded
the captured `HT-n5262` record, reviewed `retinue.t114` `0.0.1-v51` on
`serial-dfu:COM10`, and approved the immutable plan in Signalman. Linkboy began
at inspection rather than bootloader entry. `adafruit-nrfutil` erased, wrote,
verified, activated, and rebooted the exact signed package successfully.

Windows then reported the returned USB product descriptor `T114 direct PHY`
and serial `TULLE-T114-01`. That is the Retinue application USB personality,
not the stock loader. Its first CDC status session did not answer, however, so
Signalman correctly emitted `Recovery required` instead of promoting the USB
descriptor or the helper's success into application verification.

## Defects found and fixed

`LiveDeviceRunner::rediscover_application` excluded the former bootloader port
from application candidates. This T114 returned the application on the same
COM10 number, so a successful restore became a false recovery receipt.

The selector now re-identifies the former bootloader port and accepts it only
when it answers as the immutable package's expected board family. The exact
COM3 to COM10 case has a regression test.

Bootloader entry had the symmetrical assumption: it accepted only a newly
numbered port. It now also accepts the owner-initiated sequence in which the
original port is observed disappearing and then returning under the same name.
That transition, rather than the COM number alone, is the evidence. New-port
and reused-port entry both have regression tests.

The graphical flow also lacked a factual way to start from a T114 already in
serial DFU. The new explicit recovery action carries the retained loader facts
into a distinct `SerialDfuPort` transport. Linkboy skips only bootloader entry;
package checks, transfer verification, returned-application discovery, and the
receipt boundary remain unchanged.

The replay exposed one more timing fault. A returned USB serial port can exist
before the T114 application reaches its host loop. Opening it immediately can
assert DTR against the half-started application and strand that first CDC
session. Linkboy now gives a returned application a bounded two-second startup
window before probing and explicitly lowers DTR after every probe, including
failed or silent probes. Application rediscovery timeouts now say
`application did not answer` rather than incorrectly naming the bootloader.

Closing the Windows CDC handle did not by itself produce a reliable low DTR
interval. Linkboy now holds DTR low for 250 ms, spanning several of the
firmware's 50 ms samples. Two consecutive command-line probes succeeded after
that change before the final graphical replay.

## Final corrected replay

1. The board was put into its stock `HT-n5262` loader, which Windows exposed as
   COM4, `VID_239A/PID_0071`, while COM6 and COM7 remained excluded.
2. Signalman selected silent COM4, recorded the owner-declared T114 revision
   2.x, consumed the captured loader record, and used the explicit
   `Use selected T114 DFU port` action.
3. The immutable review named `retinue.t114` version `0.0.1-v51`, the serial
   DFU route, and `adafruit-nrfutil` `0.5.3.post16` before approval.
4. Signalman erased, wrote, verified, activated, and rebooted the package. The
   board returned on COM10, `VID_1915/PID_521F`.
5. Signalman's terminal screen reported `Complete`, running `T114 0.0.1`,
   package `retinue.t114`, and board `T114 2.x`. This is the corrected
   graphical application-verification receipt, not merely a helper-success or
   USB-return receipt.

An independent command-line probe attempted after the graphical receipt twice
failed with Windows `os error 121`, `The semaphore timeout period has
expired`. That later host-session failure does not replace Signalman's terminal
application receipt, but it keeps repeated T114 CDC-session reliability open as
a separate defect.

Software verification after the fixes:

- Linkboy: 61 unit tests and 2 release-integrity tests, 0 failures.
- Signalman: 6 library tests, 0 failures.
- Signalman desktop: 1 library test, 5 accessibility tests, and 12 owner-flow
  tests, 0 failures.

## Remaining physical boundary

The T114 already-in-DFU recovery route now has a terminal graphical `Complete`
receipt. Physical V4 recovery, manual screen-reader acceptance, and the
separate repeated-CDC-session defect remain open.
