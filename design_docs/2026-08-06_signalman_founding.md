# Signalman, postilion, linkboy — the host tier, named and claimed

**Date:** 2026-08-06
**Status:** names decided by Mark, stubs founded and published. Code arrives by
absorbing the `park` example.

## The decision

The radio-management host application (decided earlier in principle: a new app
treating retinue as a library, because turnstone is the browser that consumes
radio info downstream and graphshell is the reference host that surfaces meres
rather than owning radios) now has its names and its topology:

- **retinue stays the workspace.** The redshank-as-umbrella alternative (a new
  workspace with retinue as a member) was considered and declined on cost:
  mere and sibylla pin `merely-made/retinue` as git dependencies, the family's
  recorded posture is one workspace, and `retinue` is the protocol brand doing
  the most public work in the family.
- **signalman** (`apps/signalman`) — the radio-management application. Chosen
  over the banked `postilion` for the app slot because it brings a working
  lexicon, not just a label: the signal box, block working, and the
  single-line **token** — the object exactly one train may hold to enter a
  single-track section, which is this hardware's channel model by another
  name. Future UI vocabulary (semaphores, "line clear", home and distant
  signals) comes with the name.
- **postilion** (`crates/postilion`) — the shared host library beneath the
  app. The rider mounted on the lead horse, guiding the team from inside the
  motive apparatus, read against `outrider` who escorts from alongside: a
  library embedded in every radio-driving app. This is the crate the `park`
  example's guts move into; `park` was written as the consumer that reveals
  the library's shape, and it has.
- **linkboy** (`apps/linkboy`) — the firmware and link-update tool. Slightly
  goofy on purpose: it is the most consumer-facing tool in the stock-hardware
  user-flash posture, and friendly is a feature exactly where solemnity would
  be a bug. The pun is double (linkboys carried light; this carries firmware
  over the link). Register caution recorded; `farrier` is the free, sober
  fallback (the one who shoes the horses flashes the boards).
- Banked, free as of today: **redshank** (sentinel shorebird, turnstone's
  pair, for a future app), **farrier** (fallback flasher), **pilotman** (the
  official who escorts trains when token working fails — a degraded/recovery
  mode name if one is ever needed).

All three published to crates.io as 0.0.1 stubs the same day, MPL-2.0,
matching the family. The `heddle` lesson (banked unchecked on 2026-07-31,
claimed by a stranger on 2026-08-04) is why stubs went up with the decision
rather than with the code.

## What arrives next

1. `postilion` absorbs the shared parts of `crates/outrider/examples/park.rs`:
   identity files, peer tables, announce cadence, the radio-mode selection
   over `tulle::radio_io::PacketRadio`, held recipients.
2. `signalman` becomes the operator-facing binary over it; `park` thins to an
   example or retires.
3. `linkboy` wraps the two flash paths the bench already uses
   (`adafruit-nrfutil` serial DFU for the T114, `espflash` for the V4) behind
   one door, and later grows the over-the-link update lane.

The trunk guard applies unchanged: none of this re-centers the product on
multi-protocol parity, and the LXMF-on-board lane is the named next debt after
the founding.
