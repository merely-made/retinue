# AIR0 and AIR1 software receipt

**Date:** 2026-08-12

This receipt closes the host-testable portion of Air's opening slice. It does
not claim LE3 or FT1 physical acceptance.

## AIR0: detection is not capture

`radio_hand::profiles` now holds a bounded, `no_std` listener registry:

- `DetectionProfile` is frequency, SF, BW, and CAD parameters only.
- `ReceiveProfile` references one detection group and owns the exact packet
  configuration, including sync word, header mode, CRC, IQ polarity, coding
  rate, preamble, and its own capture dwell.
- `ScanPlan` emits one CAD step for a detection group followed by one capture
  step for every subscribed receive profile. Two otherwise equal profiles with
  different sync words are therefore two capture steps, never a fictional
  shared receiver.

The host tests prove the shared-detection/two-sync-word schedule, rejection of
an unregistered detection group, and separate CAD groups for different SF.

This is the registry and schedule model for the later executive boundary. It
does not configure an SX1262, collect CAD data, measure off-time or acquisition,
or assert a physical miss rate. LE3 remains open for those on-air facts.

## AIR1: announce pressure stays beneath the shared gate

`tulle::AirtimeBudget` now accepts an optional `AnnouncePacing` policy. A
limited cap spaces announce starts from their modeled LoRa airtime, rounding up
so small frames cannot leak past the cap. Ordinary frames and announces both
continue to debit the same sliding-window budget. The default is unlimited:
Tulle is shared by several protocols, so the Reticulum policy is selected by
the caller constructing its radio budget.

The Retinue Tulle bridge classifies `PacketType::Announce` and calls the
announce path. `RNodeSerialLink` and `DirectPhySerialLink` carry that class to
their pump, where the same cap is enforced before their respective wire command
is emitted.

The two pump tests use a 25% cap and record host-side command timestamps. Each
keeps the next three-byte announce at least four modeled airtimes after the
previous one. These are serial-emulator receipts, not antenna measurements.

## Verification

```text
cargo test -p radio-hand --lib
# 45 passed

cargo test -p tulle --lib --features serial-async
# 48 passed

cargo test -p retinue --test tulle_interface --features tulle-radio
# 4 passed
```

## Still open

- FT1 needs modeled-versus-measured **on-air** airtime on two real interface
  types under saturated announce traffic. The present direct-PHY event only
  reports completion, not measured transmit duration, so that receipt needs a
  measurement export or a separate instrumented capture.
- LE3 needs CAD hit/miss, retune, handoff, acquisition, and per-profile capture
  measurements on hardware.
- LE1/LE2 remain the work that makes the resident executive own this registry
  and enforce bounded leases. AIR2 and later are intentionally untouched.
