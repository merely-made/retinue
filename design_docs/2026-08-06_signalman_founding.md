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

## Steps 1 and 2, done the same day

**`postilion` absorbed the host tier.** `Station` is one operator on one
radio: identity that survives restarts, board on a serial port in either
personality, announce cadence, peer table, and a stream of `Event`s. The line
it holds is that **it has no user interface** — prints nothing, prompts for
nothing, decides no policy about how a person is shown a message. That is what
lets a terminal, a future GUI, and a test harness share one implementation,
and it is the thing `park.rs` could not do while the printing was braided
through the logic.

Two things the extraction improved rather than merely moved:

- **Peers now carry their announced display name.** `park` decoded the
  announce for its stamp cost and threw the name away, so `/peers` listed bare
  hex. The name was always on the wire.
- **Refusals became an `Event::Dropped` rather than a `println!`.** Same
  reasoning as the board counters: the commonest cause is a sender never heard
  announcing, and a silent drop is indistinguishable from a dead radio.

**`signalman` is the operator binary.** Its library half holds the terminal
face's vocabulary (`Command`, `parse`, `describe`, `render`, `report`) so a
graphical face can share it, and the binary is only glass: read lines, print
events, keep the prompt where a person expects it. It gained `/who`,
`/announce`, and `/exit`, and a real parser — `and/or` is a message, not an
unknown command, which the old prefix matching got wrong.

**`park` thinned from 380 lines to 127** and stays as outrider's example: the
shortest honest demonstration that delivery works over a real radio, and the
harness the bench drives by name. It takes `postilion` as a dev-dependency, so
outrider itself remains a boundary crate with no host machinery in it.

**Receipts.** Both binaries build; workspace suites green; six new unit tests
(profile invariants, mode parsing, identity persistence, command parsing).
On hardware: `signalman` on a V4 came up reporting **the same address** as
before the extraction, which is the identity-file compatibility that matters
most, then heard `park` on a second V4 announce and took its message —

```
[peer] d305181748ad1c76bd91fc6953e11417 bob appeared
[d305181748ad1c76bd91fc6953e11417] bob: signalman and park still talk
```

## Step 3, also the same day: linkboy

`linkboy list` surveys every serial port; `linkboy flash PORT IMAGE` takes the
right path for whatever is on it; `linkboy bootloader PORT` does the T114's
reboot-and-rediscover dance alone.

**Boards identify themselves rather than being identified.** The obvious way
to tell a T114 from a V4 is USB vendor and product IDs, and this deliberately
does not: a VID/PID says what chip enumerated, not what firmware is on it. A
board in its bootloader enumerates as something else, a board running a
stranger's firmware enumerates the same as ours, and a board on a different
carrier enumerates as that carrier. So linkboy asks, over the `status` probe
that every image answers in every channel. The bench probe built for a bench
turns out to be exactly the identification a flasher needs.

Flashing shells out to `adafruit-nrfutil` and `espflash` rather than
reimplementing either. What linkboy adds is the part that is fiddly by hand
and undocumented in one place: which board this is, sending it to its
bootloader, **finding the port it comes back on** (the T114 re-enumerates, so
the port to flash did not exist a moment earlier and is discovered by watching
the port set change), and refusing to write anything until all of that and the
image's existence are settled.

**Three defects found by running it, each invisible from reading it:**

1. **An off-by-one across answers.** The read loop stopped at the first
   newline, but the V4 answers `status` with its banner *and* its identity
   line, so the remainder sat in the buffer and was read as the answer to the
   next question — the listing reported one board's identity line as its
   region. Now it reads until the board goes quiet.
2. **A false silence on the V4.** The ESP32-S3's USB-serial-JTAG needs a
   moment after a previous host lets go, so a board surveyed right after
   something else closed the port answered nothing. Silence means "will not
   flash this", so a false silence is a false refusal; it now asks twice
   before believing it. Four consecutive surveys clean afterwards.
3. **An installed tool called missing.** The presence check used `--version`
   universally; `espflash` takes it, `adafruit-nrfutil` wants a `version`
   subcommand and errors on the flag. Presence is now "the OS could start it",
   which is what the question actually means.

**Receipts, on the real bench.** Both flash paths ran end to end through the
new door: a V4 on COM7 over `espflash`, and the T114 on COM10 over the full
DFU dance including bootloader entry and port rediscovery. All three boards
surveyed clean afterwards, correctly typed with region and channel. Refusals
were checked too — a missing image and an absent port are both declined before
anything irreversible starts.

## What arrives next

The over-the-link update lane, which is the half of linkboy's name it has not
earned yet: today it carries firmware over a cable. Beyond that, the named
debts are unchanged: LXMF on the board, and the host-side answer to a first
message from a sender who has not announced.

The trunk guard applies unchanged: none of this re-centers the product on
multi-protocol parity, and the LXMF-on-board lane is the named next debt after
the founding.
