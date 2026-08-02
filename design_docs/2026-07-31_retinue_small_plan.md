# retinue-small plan

**Status:** N0 proven on the T114 (power-loss leg open); N1 complete; N2 complete;
N3 protocol half complete (announce, links, resources); channels-in-one-image
ruled (structural decision 4), executive built when the second channel exists
**Design authority:**
[`2026-07-19_modem_embedded_and_meshtastic_research.md`](2026-07-19_modem_embedded_and_meshtastic_research.md)
(*Native Retinue personality*) supplies the boundary and
[`2026-07-19_heltec_rnode_and_embedded_rust.md`](2026-07-19_heltec_rnode_and_embedded_rust.md)
(*Third system*) supplies the runtime choice, scope rulings, and done
conditions. Both carry superseded-in-part banners pointing here; this file
supplies the gates. `retinue-small` was `endpoint-small` in the heltec doc, and
now names a firmware personality rather than a library profile.
**Target:** Heltec Mesh Node T114 (nRF52840, 1 MB flash / 256 KB RAM)
**Oracle:** desktop `retinue` through a V4 over direct PHY
**First evidence path:** the two connected boards over RF

## Goal

Put a bounded Retinue node on the board, so the radio holds the node and hosts
become lenses on it. Today the shape is:

```text
Retinue on computer -> Tulle -> USB -> radio firmware -> SX1262
```

Native mode inverts the ownership:

```text
local apps over USB/BLE
          |
bounded Retinue node on board
          |
embedded Tulle radio services -> SX1262
```

RF forwarding continues when the host disconnects. USB and BLE become a local
interface into a running node rather than the thing that makes it run.

`retinue-small` precedes broad Meshtastic parity. The RNode, Meshtastic, and
MeshCore personalities are all host-controlled: each one leaves the board a
peripheral. This is the only lane that changes what the board is.

## Current boundary

### What already exists

- V4 and T114 SX1262 bring-up, configurable PHY profiles, byte-exact TX/RX.
- Frame limits, airtime policy, bounded firmware queues, diagnostics, UI, wake,
  and sleep machinery.
- Bidirectional 4 KiB Retinue Resources across the direct-PHY pair
  ([acceptance](2026-07-23_direct_phy_resource_acceptance.md)).
- A desktop Retinue that serves as the oracle for embedded behavior.
- `radio-face` as the working precedent for a shared `#![no_std]`
  `forbid(unsafe_code)` crate consumed by both firmware images and by `retinue`.

### What the design authority gets wrong

Line 277 of the research doc reads: *"Current Retinue still uses Tokio, `std`,
unbounded channels, growable maps and queues, and in-memory resource assembly in
its live endpoint."* That is true of `Endpoint`, and of `Endpoint` alone. The
sans-io core is closer than the doc implies.

Non-test, non-feature-gated `std` in the sans-io core is five import sites:

| site | what |
|---|---|
| `channel.rs:30` | `BTreeMap, HashMap, VecDeque` |
| `reliable.rs:30` | `HashMap` |
| `resource.rs:466`, `:657` | two part-reassembly `HashMap`s |
| `address_book.rs:12` | `HashMap` |
| `ratchet.rs:12`, `:70` | `Duration`, `impl std::error::Error` |

`packet`, `identity`, `announce`, `token`, `link`, `request`, `ifac`, `hash`,
`path`, `destination`, and `iface::hdlc` carry zero `std::` paths already; they
are alloc-only. Every `std::mem::take` sits below a `#[cfg(test)]` line.
`resource`'s `std::io` is inside `#[cfg(feature = "compression")]`. `lossy.rs`
is `#[cfg(feature = "tokio")]` end to end, a desk harness rather than core.

So the `no_std + alloc` spike the doc asks for is small, and it is not the
project. Alloc exists on the T114. Unbounded growth is what kills a 256 KB node.
**The work is the capacity contract.**

### What genuinely does not exist

Grepping both firmware images for flash, NVMC, storage, and RNG returns zero
hits. The peripherals are unused rather than unavailable: `embassy-nrf 0.11.0`
declares `pub mod nvmc` and `pub mod rng` for the nRF52840 (gated only against
nrf54l / nrf5340-app / nrf91), and `Nvmc` implements `NorFlash` and
`MultiwriteNorFlash` at a 4096-byte page with `WRITE_SIZE = 4`.

`retinue`'s sans-io core is deliberately RNG-free: `getrandom` is tied to the
`tokio` feature. Entropy must be injected, and it must exist before a link can
be negotiated at all.

Also absent: an embedded Retinue node, and any firmware dependency on the
`tulle` crate. Neither image links `tulle` today; `t114-phy` pulls `selvage`,
`radio-face`, embassy, and `lora-phy`.

## Structural decisions

### 1. Bound in the shared core, not in a fork

`Node::ingest`/`Node::poll` is a **shell** split, and it must stay one. If
`retinue-small` re-implements windowing, retransmit, or resource reassembly
against fixed arrays while `Endpoint` keeps the growable versions, the desktop
stops being an oracle for the board and becomes a different implementation that
happens to interoperate. That forfeits the single most valuable asset the
project has.

Put the capacity parameter on `channel`, `reliable`, `resource`, and
`address_book` in place. The desktop instantiates large, the board instantiates
small, and both exercise one algorithm. The typed capacity errors of N1 then
fall out of one code path rather than two.

`Endpoint` remains the desktop shell and is not ported.

### 2. Lift the embedded radio service out of the firmware loops

There is nothing to strip out of `tulle`; its `PacketRadio` is host-side by
construction (`Vec`, Tokio futures, serial links) and stays that way. There is
something to lift out of `t114-phy/src/main.rs` (782 lines) and
`heltec-v4-phy/src/main.rs` (757 lines), which have visibly converged: config
apply, TX path, RX path, diagnostics, airtime, queue policy. `board.rs` (56 and
52 lines) is already the board seam.

That extraction goes to **`radio-hand`**, a new `#![no_std]` crate on the
`radio-face` pattern, not into `selvage`. The host depends on `selvage` and
should not inherit an async HAL through it. Two firmware consumers plus a
portability subset is enough to justify a crate rather than a module.

Named 2026-07-31. Face shows, hand does: `radio-face` renders the status
surface, `radio-hand` works the radio (config apply, TX, RX, diagnostics,
airtime, queue policy). It is `publish = false` like its sibling, which is the
rule the tree already follows: published crates take family names from the
textile and procession registers (`selvage`, `tulle`, `sennet`, `outrider`),
unpublished firmware-support crates take plain descriptive ones.

### 3. The HostLink seam (ruled 2026-08-01)

The command dispatch is identical in both images apart from the host transport,
so moving it needs a transport seam. The seam is designed against the hardware
ecosystem this family could expand to, not against the two boards on the desk.

The transport landscape the seam must survive:

| transport | boards | can it tell the peer left? |
|---|---|---|
| USB CDC native | T114, V4, RAK4631, T-Echo, RP2040+SX1262 | yes (enumeration, DTR) |
| UART, bridge chip or bare header | V4 low-power personality, older T-Beam/T3 | no; writes fire into the void |
| BLE, NUS-style byte pipe | every nRF52840 board, ESP32 family | yes (connect/disconnect, MTU) |
| TCP over WiFi | ESP32 family | yes (sessions, possibly several) |

The width axis that matters more than boards is personalities: the RNode KISS
session and the Meshtastic client API are different byte protocols riding these
same transports. The seam therefore carries no `selvage` vocabulary at all, so
the compatibility branches can ride it later without rebuilding it.

**The trait, in `radio-hand`: a byte pipe plus a session, nothing else.**
Illustrative only, not implementation-ready:

```rust
pub enum LinkFault {
    /// The peer is gone and the session is over.
    Detached,
}

pub trait HostLink {
    /// Wait until a peer is attached. A transport with no attachment
    /// concept returns immediately.
    async fn attached(&mut self);
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, LinkFault>;
    async fn write_all(&mut self, bytes: &[u8]) -> Result<(), LinkFault>;
}
```

The shape is `embedded-io-async` (already in the tree; the V4 already bounds
its `write_all` on it) plus the one thing embedded-io does not carry, session
lifecycle. What is excluded is as load-bearing as what is included:

- No MTU. Chunking into 64-byte USB packets or BLE ATT payloads happens inside
  each impl.
- No wake. `CommandStream` already consumes `WAKE_BYTE` in its
  `discard_until_boundary` state; wake is a parser concern, proven in `selvage`.
- No command types. The seam is personality-agnostic.
- No capability negotiation until BLE actually lands and demands it.

**Layering.** Dispatch lives in `radio-hand`, generic over `HostLink` plus
`RadioKind`: it owns `CommandStream`, the select over host bytes and radio RX,
`service::apply_profile`, event emission, and `LocalStatus` mutation, with
board actions (publish-to-screen, enter-bootloader) as narrow hooks. Transport
impls and executor glue stay in each firmware binary, per the heltec doc's
standing ruling: share `embedded-hal`/`embedded-io` traits at the driver edge,
keep executor glue per binary, and never build a common Embassy abstraction
across Espressif and Nordic. `radio-hand` links neither HAL.

**What this dissolves.** The T114-breaks/V4-discards write divergence was two
transports speaking, not a style choice. On USB a failed write means the host
detached, so the impl reports `Detached` and the shared loop ends the session.
On a bridge-chip UART a write cannot meaningfully fail, so its impl never
reports `Detached` and the same loop never ends a session. Both existing
behaviours fall out of one shared dispatch. The one real behaviour change is
the V4's USB personality, which today discards write results and will gain
break-on-detach; that is a correction, since today it pumps radio events into
a dead pipe after the host leaves.

**Known edges, named rather than hidden:**

- STM32WL (Wio-E5, RAK3172) is the one radio family out of reach: its radio is
  on-die and the vendored lora-phy has no `RadioKind` for it. The seam does not
  block it; the radio layer does.
- Multi-client (N3 and later, when USB and BLE serve the on-board node at
  once): one `HostLink` is one session. Dispatch owns no statics and takes its
  state by `&mut`, so instantiating it per link against shared node state later
  is cheap. That is the entire concession to that future.
- The V4 selects its personality at compile time (`host-usb` default,
  `host-uart-low-power`), so one binary speaks one transport and the seam stays
  static generics with no dyn dispatch.

**Build order:** the trait plus the T114 impl first (moving dispatch changes
the image, so the byte-identity shortcut is gone and the RF check runs as a
counted block per the receipt rule below), then the V4's two impls.

### 4. One image, channels, and the executive (ruled 2026-08-01)

Mark's framing, from game consoles: since shipped images are GPLv3 anyway,
ship one image whose personalities are runtime-selectable channels, rather
than one image per personality swapped over DFU.

Adopted, as an evolution rather than a pivot, because the hardware already
enforces the channel model: one SX1262, one PHY configuration, one sync word
(`0x2B` Meshtastic, `0x12` MeshCore and direct-PHY, Reticulum's own framing),
so the board is physically a citizen of exactly one mesh at a time. Reflashing
never enforced one-protocol-at-a-time; the chip did. The selector surfaces a
constraint that exists instead of adding one. The closest shipping precedent is
the Flipper Zero, one image with an executive and protocol apps, and
field-switching without a host computer is a real differentiator: Meshtastic
and MeshCore users reflash to move between them today and resent it before we
existed. One SKU under the stock-hardware user-flash posture.

**The ruling:**

- Personalities become **channels** behind a common trait in `radio-hand`:
  start, serve, stop. The executive owns what the pressure points already put
  below every personality (radio, `HostLink`, the store, the face, the region
  table) and exactly one channel is active.
- The active channel is a persisted field in the settings record. **Switching
  is by reboot** in v1: persist the choice, reset. That is most of the UX for
  a fraction of the complexity, the flash write lands at the reboot boundary
  where writes are already legal, and channel-teardown correctness is
  sidestepped entirely. Hot-switching is a later question if it ever matters.
- This supersedes the research doc's board order item 1, "separate RNode,
  Retinue-small, and Meshtastic-minimum images" on the T114: separate
  **channels** in one image, selected at boot. Each channel's *done conditions*
  in the research doc are unchanged.
- **Trunk guard:** the executive ships with one channel (today's modem
  personality), gains the node channel across N3 to N6, and foreign-mesh
  channels join only after passing their own gates. The selector is built when
  the second channel exists, not before. A channel selector must not re-center
  the product on multi-protocol parity; the trunk doctrine stands.
- **Licensing edge, stated precisely:** GPLv3 applies at the final image link,
  and MPL-2.0 crates compose into it cleanly. The relaxation is only at that
  boundary. Nothing GPL enters the workspace crates, `sennet` stays clean-room
  MPL for independence and crate reusability, and `deny.toml` keeps
  hard-lining GPL dependencies.
- **Costs owned:** every shipped channel is flash-resident always (measure each
  channel's delta as it lands; the region is 800 KB and today's whole image is
  80 KB, so the budget is real but not tight). The receipt matrix multiplies:
  a release re-receipts every shipped channel as a counted block, and
  feature-gated single-channel builds remain possible for debugging. RAM is
  not multiplied: only the active channel instantiates its tables, which the
  N1 bounded-alloc design already makes constructible on demand.

What this is **not** is a hypervisor. There is no isolation or scheduling of
untrusting guests, and the word overpromises exactly the thing the next
section examines honestly.

#### The one panic domain, examined

A combined image means a bug in any channel can halt the whole board. Baseline
truth first: the panic domain is already one, today, with one personality.
Both images link `panic-halt`, so channels multiply exposure to a failure mode
that exists; they do not create it.

The armillary intuition, checked against armillary's actual shape. Its
discipline is a kernel that owns all canonical state, actors that talk to it
only by message, and a boundary the type system enforces (the kernel context
is `!Send` by construction, so moving authority onto an actor thread is a
compile error). That discipline transfers here nearly verbatim. What does not
transfer is armillary's *fault* boundary, because on the host it comes from OS
threads: a panicking actor dies alone and the kernel restarts it. The board
has no threads to die alone. `no_std` firmware is `panic = abort`, and
`catch_unwind` does not exist to want.

So the answer is the hybrid, three boundaries from three different mechanisms:

1. **The memory boundary is the type system.** Channel crates are safe Rust
   under `#![forbid(unsafe_code)]`, the `radio-face` precedent. A channel
   cannot scribble another channel's state, the executive's, or the store's.
   This is the protection an MPU would sell, delivered by the compiler at no
   runtime cost.
2. **The authority boundary is the executive, armillary-shaped.** The
   executive owns the radio, the flash, the region table, and the face;
   channels are clients over mailboxes (embassy channels, mechanically). This
   is the same seam pressure points 1 and 2 already demanded, since the power
   clamp, duty and dwell gating, and CAD want to live below every channel in
   exactly one place. A berserk channel can request nonsense; it cannot bypass
   the clamp, touch the store, or hold the radio.
3. **The fault residue goes to supervised reboot.** `embassy-nrf` has the
   watchdog (`wdt`, verified present for the nRF52840). The executive feeds it
   only on channel liveness, so hangs reboot the board as well as panics.
   Attribution without violating the quiet-window rule: the panic handler
   writes a crash record to noinit RAM (the `panic-persist` pattern) or to
   `GPREGRET`, the same retention register the bootloader entry already uses,
   and the *next boot* reads it and stores it through the A/B record, which is
   a boot-time write and therefore legal. On top of that, a crash-loop policy:
   repeated crashes in a channel within a bounded number of boots fall back to
   the modem channel, or to a status-only face showing the crash record. The
   bootloader itself is the one hardware-isolated component already in the
   system; DFU survives any application crash, which is why the fallback
   always exists.

Why reboot-as-recovery is honest in this domain, rather than a concession:
mesh protocols are engineered for lossy membership, and a node blip is normal
weather on every mesh in question (stock Meshtastic nodes reboot on panic
routinely). The losses that would actually matter are the identity and
settings, which are flash-persisted and proven across six reflashes, and crash
loops, which the fallback policy bounds. Warm state (a NodeDB, routing tables)
is lost on reboot; whether a channel persists any of it is a later,
channel-level decision that must obey the quiet-window rule.

Rejected, with reasons rather than by reflex:

- **MPU process isolation, the Hubris and Tock shape.** Real, proven on
  Cortex-M, and the wrong trade here: it requires separate stacks, privilege
  levels, and a syscall boundary, which means abandoning the embassy async
  stack and the vendored `lora-phy`, and it buys protection against wild
  writes, the failure class safe Rust already excludes. It becomes the right
  trade only if a channel must ever run untrusted code, which is ruled out:
  third-party code is the host-side participant gate's problem, never
  firmware's.
- **Catch-and-restart in place.** Needs unwinding; embedded is abort.
  Unavailable, and no amount of architecture makes it available.
- **Panic-free-by-construction as the sole strategy.** Right discipline, wrong
  guarantee. Adopt it where it can be receipted: the executive and store paths
  lean on fallible APIs (already the N1 pattern) and clippy restriction lints,
  and a panic-never-style link assertion is worth attempting there. But the
  vendored driver stack is not panic-free, so the watchdog stays regardless.

## Gates

Each gate carries linker receipts: flash, static RAM, heap high-water mark, and
maximum task/future size. The T114 is chosen precisely because 256 KB forces
honest limits, so a gate that lands without those numbers has not landed.

**Receipt rule (added 2026-08-01, from the N2 A/B finding):** an RF receipt is
a pass count out of a stated number of runs, never a single pass. The direct-PHY
path measurably fails a fraction of single runs on the shared ISM band (v16,
the N0-proven image, passed 8 of 10), so one passing run certifies nothing and
one failing run condemns nothing. When a change could have altered RF
behaviour, the receipt is an A/B against a control image on the same hardware
in the same session.

### N0 — Board substrate

Entropy and persistence on the T114. A persisted device identity slot over
`Nvmc`, and hardware RNG wired to the seam the sans-io core expects.

Keep user and application identities configurable and separate. The radio needs
a persisted **device** identity for autonomous routing and management. Commons,
Murm, Outrider, and other application services ordinarily stay on the phone or
computer.

Carried from the heltec doc: the flash format is atomic, versioned, and
recoverable, and radio and protocol settings change without reflashing. The
CSPRNG is supplied by the shell, never reached for by the core.

**Done:** identity and settings survive reboot and power loss atomically; the
entropy seam feeds link ephemerals and AES IVs; settings change without a
reflash; a corrupted or absent slot yields a deterministic outcome rather than
a hang.

### N1 — Bounded core

Capacity parameters on `channel`, `reliable`, `resource`, and `address_book`.
`no_std + alloc` build of the sans-io core. Typed capacity errors on every
table, with the node still live after rejection.

Rules carried from the heltec doc: every collection gets an explicit capacity or
a storage trait, and every timeout gets an injected monotonic clock. Resources
need bounded part windows and streaming storage, because the T114 cannot make
resource size proportional to RAM. The first T114 profile omits transport-node
routing and bzip2; both return only after flash and RAM receipts show room.

**Done:** `--no-default-features` builds the core for
`thumbv7em-none-eabihf`; the existing sans-io tests pass unchanged against the
desktop instantiation; every table returns a typed error at capacity and keeps
serving; no collection in the core grows without a declared bound.

### N2 — radio-hand

Extract the shared radio service from both `main.rs` files into `radio-hand`,
with command dispatch moving through the `HostLink` seam (structural decision
3). Both images rebuild on it, with `board.rs` as the only board-specific seam.

**Done:** both images build on `radio-hand`; the direct-PHY exchange of
[2026-07-23](2026-07-23_direct_phy_resource_acceptance.md) passes byte-exact as
a counted block against a control image per the receipt rule; the two `main.rs`
files shrink to board wiring plus shell.

### N3 — The node shell

```text
node.ingest(interface, frame, now) -> Actions
node.poll(now)                     -> Actions
```

Explicit capacities for links, routes, queues, retransmits, and Resource
windows. The shell supplies clock, entropy, persistence, radio, USB/BLE, and
scheduling.

**Done:** the shell runs on the T114; ingest/poll replayed against the desktop
fixture corpus produces identical Actions.

### N4 — Announce and link

Announce over direct PHY. Establish one link with desktop Retinue through the
V4.

**Done:** the board announces and the desktop observes it; a link completes in
both directions over real RF, as a counted block per the receipt rule (e.g.
at least 8 of 10 link setups complete, with every failure logged).

### N5 — Reliable data and survival

Exchange reliable data both ways. Survive loss, reordering, and reboot. Re-prove
the N1 capacity errors under live traffic.

**Done:** byte-exact payload both directions over RF as a counted block per
the receipt rule; recovery after induced loss and after a mid-transfer reboot;
a full table under live traffic rejects with a typed error and the node stays
operational. Bounded outcomes for the
heltec doc's adversarial set: fuzzed frames, a full route table, a full queue,
entropy failure, flash corruption, and resource cancellation. The T114 receipt
adds idle, receive, and transmit current.

### N6 — Panels from local state

Identity, Links, Peers, and Traffic panels drive from board-local Retinue state
instead of host snapshots. Status, Power, and Radio already show firmware truth;
this makes the other four genuine rather than projected.

**Done:** the four panels read local node state with the host disconnected; RF
forwarding continues across host disconnect and reconnect.

## Pressure points ruled ahead (2026-08-01)

Things this session's work surfaced that get harder the longer they wait.
Ruled now so the build does not improvise them; none are built yet.

### 1. The regulatory floor lives in radio-hand, below every personality

Nothing clamps `tx_power_dbm` today: the profile's `i8` is widened and handed
to the driver, verified 2026-08-01. That is tolerable while a host drives the
radio; it is not once the board is autonomous and a persisted profile survives
reboot with no host in the loop. The FCC posture (stock hardware plus
user-flash) makes firmware the only enforcement point that exists.

Ruling: region is a persisted board fact, not a host suggestion, and
`radio-hand`'s apply and TX paths clamp power and gate airtime against it.
A profile may ask for less than the regional cap, never more, and the clamp
result is reported honestly (the applied power, not the requested one).
Retrofitting this after settings persist means changing the stored format and
the wire, which is why it is ruled before either exists.

**Shape decided 2026-08-01 (Mark):** do what Meshtastic and the others do.
`radio-hand` carries a built-in table of compliant region profiles under plain
names (US 915, EU 868, and so on), each supplying frequency bounds, power cap,
duty cycle, and dwell. The user picks one at first setup; the choice persists
as the board fact; every host profile is validated against it. Until a region
is chosen the board does not transmit and the face says why, which is the
current Meshtastic posture and honest by our own no-placebo rule. The table is
data, so adding a region is an entry, not a code path.

### 2. Channel citizenship also lives in radio-hand

Neither image does channel-activity detection before TX; transmit is blind,
verified 2026-08-01. Fine for two boards on a desk, wrong for a shared band,
and duty-cycle and dwell rules are region-coupled (point 1). CAD or
listen-before-talk, the airtime budget, and dwell limits belong in
`radio-hand`'s TX path so that every personality (direct-PHY, RNode,
Meshtastic, and the native node) inherits them, rather than each reimplementing
citizenship. The collision-mitigation ideas doc (2026-07-24) is the design
feedstock.

### 3. Flash writes are boot-only until a quiet-window write path exists

N0 proved direct NVMC access works **with the SoftDevice dormant**. Enabling
any BLE stack on the nRF52 changes the flash rules: radio timing constrains
when flash may stall, and an S140-based stack takes over flash access
entirely. Independently of BLE, a page erase stalls the CPU for tens of
milliseconds, which blanks RX even today.

The current invariant, kept deliberately: the store writes only at boot, before
the radio starts. Settings persistence (the open N0 item) must not break this
by scattering runtime writes; it stages changes in RAM and commits them in a
declared radio-quiet window. Designing that window in from the start is cheap;
retrofitting it out of scattered write sites after BLE lands is the intractable
version.

**On the starvation question (Mark, 2026-08-01): the window is taken, not
awaited.** The firmware does not wait for the band to go quiet, which on a busy
mesh it never would. It makes quiet: defers its own TX, accepts that RX is
blanked for the erase and write, and commits. So there is no unbounded-wait
failure mode, and the residual risks are small and bounded:

- A clipped frame. At LongFast SF11 a frame is roughly 700 ms of airtime, so a
  tens-of-ms blank can clip one. That is indistinguishable from ordinary RF
  loss, which the reliability layer exists to survive. Cost: one retransmit.
- Starvation by policy, not physics: "commit when convenient" left vague lets a
  busy relay defer forever. So staged settings carry a deadline and commit
  within a bounded time of staging, taking the one-frame risk.
- Power cut before commit: staged settings are lost and the board boots on the
  old profile. Stale but valid, never corrupt, which the A/B store already
  guarantees.

When BLE lands the window additionally coordinates with the stack's radio
timing, which is a scheduling constraint on the same bounded window, not a new
mechanism.

### 4. The 255/500 MTU fork is named, and the trunk takes 255

The SX126x carries 255-byte frames; Reticulum's standard interface MTU is 500.
The trunk (retinue-to-retinue over direct PHY) negotiates link MTU 255, which
the core already supports, and every gate in this plan runs on that. Carrying
500-byte packets for stock-RNS peers over RF requires the long-packet
fragmentation lane, and that belongs to the RNode personality where the
research doc already scoped it. It is never bolted onto direct-PHY ad hoc:
on-air formats ossify the moment anything third-party deploys against them.

### 5. What a new board costs, so expansion stays a known quantity

The T114 and V4 establish the shape. Adding a board is:

- `board.rs` (~55 lines of pins and board facts), plus revision hedges where
  sources disagree (the T114 listens on both candidate button pins; that
  pattern is the precedent, not a wart);
- a `HostLink` impl over whatever the board's host transport is;
- a persistence backend for `radio-hand::store`'s record format, which is
  deliberately format-only: NVMC here, a partition on ESP32, XIP-flash rules on
  RP2040. The format is portable, the backend never is;
- a flash/DFU path and its receipts, counted per the receipt rule.

What disqualifies a board today: no `RadioKind` in the vendored lora-phy
(sx126x and sx127x exist; STM32WL's on-die radio does not), or a persistent
store the platform cannot offer atomically. On the T114 specifically, the two
carved pages are the entire persistent budget until `memory.x` is re-carved,
and the `build.rs` assertions move with it; one A/B record with a versioned
body should carry identity plus settings atomically rather than multiplying
pages.

**N2 second half, 2026-08-01: the HostLink seam is built, T114 side.**

`radio-hand` gains `link` (the trait), `dispatch` (the shared command loop),
and `board_status` (a third function both images carried identically). The
T114's `host.rs` implements `HostLink` over CDC, chunking to 64 bytes inside
the impl, which is what keeps MTU out of the trait. `main.rs` is **801 -> 586
lines**, under the 600 ceiling for the first time.

One design change fell out of building it, and it was a real wire bug avoided.
Dispatch writes the `EVENT_TX` reply itself, so a chip diagnostic emitted by
the *caller* afterwards would arrive after the reply. The host takes the most
recent diagnostic when a transmit fails and attaches it to that failure
(`last_diagnostic.take()` in `tulle`), so the reordering would have stripped
`irq=/errors=/sync=` from every TX-timeout error and misattributed it to the
next one. Hence `ChipDiagnostics`, a trait the board implements that takes
dispatch's own `lora` borrow, so the diagnostic still precedes its reply.

**Counted RF receipt, per the receipt rule, same hardware and same day:**

| image | passed |
|---|---|
| v16, the pre-N2 control | 8 of 10 |
| v17, `apply_profile` | 5 of 8 |
| **v18, the HostLink seam** | **7 of 8** |

All seven v18 outputs byte-exact against the input, zero mismatches, and every
run at 24.2s. v18 sits at the top of the observed band, so no regression is
detectable. Note this is exactly the receipt that a single run would have
gotten wrong in either direction: v17's first run failed and v18's fifth did.

The text probes were exercised on the flashed image too: `radio` returns
`84 00 00 00 00 24 B4` through the new `ChipDiagnostics` path with the sync
word reading `24B4`, and the identity survived a fifth reflash.

**Two misses caught after the first receipt, both worth recording.**

First, `radio-hand`'s host tests had been failing to link since `lora-phy` was
added at 85b24fd. The vendored fork depends on `defmt` ungated, and `defmt`
needs a global logger only a firmware binary provides, so the test binary could
not link. It went unnoticed because after adding the dependency only target
checks were run, never `cargo test -p radio-hand`. Fix: `radio` is an opt-IN
feature gating `service` and `dispatch`, so the failing configuration is the one
a caller has to ask for and plain `cargo test` keeps `store` and `phy` on the
desk. The two images enable it explicitly. 23 tests restored.

Second, that manifest change altered the image (79,746 -> 79,714 bytes), which
invalidated the v18 receipt: it no longer described the committed source. So the
block was re-run against v19, the image the tree actually builds. **v19: 7 of 8,
all outputs byte-exact**, matching v18 and the top of the control band. The
lesson generalises: a counted receipt is attached to a *binary*, so any change
after it — even one that looks manifest-only — retires it.

Still open in N2: the V4's two `HostLink` impls. Deliberately not done in the
same session, because the V4 is the RF test peer — reflashing it would have
destroyed the known-good control the A/B above depends on. Its transports split
into `embedded_io_async` rx/tx halves, so one generic impl covers both
personalities, with `attached()` returning immediately and writes never
reporting `Detached` on the UART side.

**Channels ruled, and the panic domain examined, 2026-08-01.** Structural
decision 4, from Mark's console framing: one GPLv3 image, personalities as
boot-selected channels, switch-by-reboot, executive built when the second
channel exists. The panic-domain treatment is the hybrid Mark's intuition
pointed at: the memory boundary from the type system, the authority boundary
from armillary's kernel-and-actors shape, and the fault residue to a
liveness-fed watchdog with a crash record in retained RAM and a crash-loop
fallback to the modem channel. MPU isolation (Hubris, Tock) rejected with
reasons. The research doc's board order carries the superseded-images note.

**N2 COMPLETE, 2026-08-01: the V4 rides the seam too.**

Both images now dispatch through `radio-hand`. V4 `main.rs` is 757 -> 618
lines; T114 is 801 -> 586. Its `SplitHost` covers both personalities because
both split into `embedded_io_async` halves, and `ignore_host` turned out to
ignore its arguments entirely (it only pends), so the sleep-proof read path
needed no link at all.

**Two deliberate behaviour changes on the V4**, both corrections:

- Writes now end a session on `Detached`. Its transports never report it, so
  in practice nothing changes; the shared loop simply expresses it.
- **Transmit gained a deadline.** The V4 had none: it awaited `lora.tx()`
  indefinitely, so a wedged radio hung the loop forever. It now shares the
  T114's three-second deadline, and with it the `ChipDiagnostics` path the V4
  never had, since nothing could previously time out to ask.

**Left alone deliberately:** the V4's RX arm. `sleep_proof_receipt` reads
`local_status.rx_frames`, so the status update must precede the sleep-proof
block, whereas `on_radio_frame` writes the event before updating status. Using
it would have silently changed a receipt's contents under a feature this bench
cannot exercise. Fifteen lines of duplication is the right price.

**The bug the hardware caught, which compilation could not.** The V4's original
`write_all` helper did write *and flush*; `SplitHost` dropped the flush. USB
Serial/JTAG holds written bytes in the peripheral until flushed, so the board
booted, answered its banner, and then silently never delivered a config
acknowledgement: 4 of 4 runs failed with a transport fault. One line, and only
a real host could find it.

**Counted receipt, two boards, same session, same channel.** The T114 (v19) was
the fixed peer, and because a second V4 exists, the wired board and an
untouched control ran side by side:

| board | firmware | passed |
|---|---|---|
| COM7 | wired through `radio-hand` | **8 of 8** |
| COM6 | untouched control | **8 of 8** |

All outputs byte-exact. No detectable difference. Note the channel was quieter
than the morning's blocks (5 of 8, 7 of 8, 8 of 10), which is precisely why a
receipt compares a control taken in the same session rather than against a
number from hours earlier.

**An operational trap worth never rediscovering.** Mid-block the *untouched*
control began failing 4 of 4 while the wired board passed. Taken at face value
that reads as "the change improved things", which is unsupportable. The cause
was the bench probe: asserting DTR **and** RTS together is the ESP32-S3's
download-mode entry sequence, so the probe had dropped COM6 into the ROM
bootloader, where it answers nothing and looks dead. Recovery is
`espflash reset --port COMn`. The probe now asserts DTR only, which the T114's
`wait_connection` still needs and which leaves a V4 alone. Also: `espflash
flash --no-stub` fails at `FlashEnd` on this setup; the default stub works.

**N3 first increment, 2026-08-01: `retinue::node` exists and announces.**

`Node::ingest(interface, packet, now) -> Actions` and
`Node::poll(now, interface, rand_hash) -> Actions`, sans-io and bounded.
Nothing in it reads a clock, allocates without a bound, or performs I/O: time
arrives as an argument, entropy arrives as caller-supplied bytes (the discipline
`announce::build` already followed so fixtures reproduce byte for byte), and
everything the node wants leaves as an `Action` for a shell to carry out.
`Actions<N>` is bounded and reports `overflowed()` rather than dropping
silently. It cross-compiles for `thumbv7em-none-eabihf`.

This gate's slice is announce in both directions, which is also N4's first
half. The oracle is the fixture corpus: **all six RNS-generated invalid
announces are refused** and leave no trace in the address book, which is the
property that matters most on a board, since accepting one would let a peer
populate its tables with unverified identity. Eight node tests, 171 in the
crate.

**A sequencing consequence worth stating.** N3 is where structural decision 4
stops being theoretical: putting a node on the T114 creates the *second*
channel, which is precisely the condition under which the executive was ruled
to be built. So the remaining N3 work is not only "add links and resources to
`Node`" but also "found the executive and make the modem a channel beside the
node". The desk-side protocol work and the firmware-side channel work are
separable, and the protocol work comes first because it is verifiable without
hardware.

Remaining for N3: link handling and resource windows in `Node`, then the
executive and the node channel on the T114, then the fixture-corpus replay that
the gate's done condition names.

**N3 links, 2026-08-01.** `Node` now establishes links in both directions,
carries encrypted data on them, and tears them down. `open_link` on a peer the
address book knows, `accept` for a request addressed to this node, proof
completion for a link this node opened, `Inbound::Data` out as an action, and
`Inbound::Close` dropping the link. Bounded by `LINKS`, refusals counted in
`refused_links()`. Offered MTU is 255 per pressure point 4: the trunk is
retinue-to-retinue over direct PHY, and stock RNS's 500 belongs to the RNode
personality's long-packet lane.

Two decisions inside it worth recording.

**A retransmitted request is answered with the same proof.** The established
link keeps the proof that made it, so a peer that did not hear the first answer
gets that exact answer again. Accepting twice would hand the two sides
different keys for what the initiator believes is one link, failing later and
confusingly. On a medium measured at a 20 to 40 percent single-run failure
rate, this is the common case rather than an edge one, and it is pinned by a
byte-for-byte test.

**The responder's ephemeral seed is derived, not random.** An initiator is
handed a seed by its shell, one per attempt. A responder answers packets it did
not ask for, so entropy would have to be threaded through every `ingest`.
Instead the seed is `full_hash(tag || our secret || link id)` for each half:
unpredictable without the node's private key, different per request, and
reproducible, which is exactly what makes the same-proof property above work.

Fifteen node tests, 178 in the crate, clippy clean in both configurations, and
the sans-io core still cross-compiles for the board.

Remaining for N3: resource windows in `Node`, then the executive and the node
channel on the T114.

**N3 resources, 2026-08-01: the protocol half of N3 is complete.** `Node` now
publishes and receives resources, one transfer per link at a time because a
board cannot hold two, with `MAX_RESOURCE_PARTS = 32` (roughly 13 KB of
reassembly) against the desktop's 4096.

**A gap N1 left, found by trying to use it.** N1 capped `Incoming`, but
`ResourceReceiver` called `Incoming::new` with the desktop default and exposed
no way to lower it, so a board could not actually defend itself: a peer could
advertise a 1.7 MB resource and the board would try. `ResourceReceiver` now
takes `with_limits(link, request_window, max_parts)`. Capping a type is not the
same as making the cap reachable, and only wiring it up revealed the difference.

**A bug the test caught immediately.** A refused advertisement left an empty
receiver holding a table slot. With a handful of slots that is the difference
between refusing one oversized offer and refusing every peer afterwards. A
receiver created for a packet that then says nothing is now dropped, since
saying nothing is exactly how the refusal is expressed.

Also handled: closing a link discards the transfer riding on it, because
reassembly state without a link is memory held for a peer that is gone.

Nineteen node tests, 182 in the crate. The resource test drives a 3,000-byte
multi-part transfer through a desk pump rather than one part, so the request
window, the hashmap and reassembly are all exercised; loss is the medium's
business and is measured on hardware at the gates.

**N3's remaining work is entirely firmware:** the executive, the node channel,
and the fixture-corpus replay on the T114. The protocol is done and verified at
the desk.

**N3 firmware, first step, 2026-08-01: the store carries settings, and the
board keeps the identity it already had.**

`radio_hand::settings::Settings` is the record body: an identity plus the
channel to boot into. `IdentityStore` became `SettingsStore`, with a `save` for
the executive's eventual channel selection.

**The body grows; it is not versioned.** Bumping the record header's version
would be the obvious way to add a field and is the wrong way here: a board
flashed with the new firmware would find its record at the old version, refuse
it, and mint a fresh identity. The device would silently become a different
device, losing the address every peer knows it by. So a 64-byte body is an
identity and nothing else, which is exactly what the first firmware wrote, and
it decodes as that identity with default settings. Fields append after it. An
unknown channel byte falls back to the default rather than failing the record,
because losing an identity over an unreadable preference is a far worse trade.

**Receipt, on the board that has carried the same identity for seven flashes.**
Before v20: `identity=loaded slot=A seq=0`. After: `identity=loaded slot=A
seq=0`, byte-identical. Same slot, same sequence, so the legacy record was read
rather than replaced. Had the version been bumped instead, this would have read
`created slot=B seq=1` and the identity would be gone. The unit test for the
legacy body is real, but this is the property actually mattering and it needed
hardware to mean anything.

220 tests across the three crates, both images build, clippy and fmt clean.

Remaining for N3: the channel trait and the executive, the node channel, and
the fixture-corpus replay on the T114.

**N3: the board carries a Retinue node, 2026-08-01.** The T114 links `retinue`
and builds a `Node` from its persisted identity at boot, reporting
`node=599997c8 heap=0/49152`: a real destination hash derived from the key the
board has held across eight flashes.

**The allocator arrived exactly where the plan said it would.** Linking
`retinue` failed with "no global memory allocator found", because N1 chose
`no_std + alloc` and this is the gate where a board pays for it. N0's receipt of
a zero heap high-water by construction ends here, which is why the heltec doc's
done condition asks for a heap figure at all. What replaces "no heap" as the
guarantee: a fixed 48 KB array that cannot take memory from anything else,
every table above it already bounded by N1 so live allocations have a ceiling,
and `used()` making the figure a measurement rather than an assertion. LLFF
over TLSF because this workload is a few short-lived buffers, not a churn of
many sizes.

**The protocol's cost, measured rather than estimated.** A first attempt put
the node behind `#[allow(dead_code)]` and reported +128 bytes, which was the
linker discarding `retinue` entirely: the probe measured nothing. Instantiating
it for real:

| | flash | static RAM |
|---|---|---|
| v20, no protocol | 79,618 (9.9%) | 11,356 |
| v21, protocol linked | **156,994 (19.6%)** | **66,908 (28%)** |

`retinue` costs about 77 KB of flash; 48 KB of the RAM growth is the heap
array. Both comfortable, and it settles structural decision 4's flash-residency
question with a number: several channels fit.

**Counted RF receipt, v21 against the COM6 control peer: 7 of 8, all outputs
byte-exact.** In the established band (v16 8 of 10, v19 7 of 8, wired V4 8 of
8), so nearly doubling the image did not disturb the modem path.

Two bench notes. Nineteen `cargo` and `rustc` processes accumulated across
repeated `cargo run` invocations and stalled both a counted block and a test
run; build the example once and run the binary directly instead. And the
scratchpad was cleared mid-session, taking the probe and the RF inputs, so both
were rebuilt with the probe keeping its DTR-only fix.

Remaining for N3: the channel trait and executive, so the node is *driven*
rather than merely resident, and the fixture-corpus replay.

## Non-goals

- Porting `Endpoint`. It stays the desktop shell.
- Meshtastic, MeshCore, or RNode parity work. Those are compatibility lanes
  around this trunk, and their gates stay in the research doc.
- Retinue over the Meshtastic bearer. That remains a V4 measurement question
  after the Meshtastic personality exists.
- Moving application services onto the board.
- BLE. USB first; BLE is a later interface onto the same running node.

## Progress

**Plan founded 2026-07-31** from a direction call that native Retinue is the
product and the foreign meshes are branches.

**N0, desk half, 2026-07-31.** `radio-hand` was founded here rather than at N2,
because the A/B record logic is exactly the code that wants desk tests and a
`no_std` + `no_main` binary cannot run them. N2 now moves the radio service into
a crate that already exists.

Board fact found while reading the linker script, and recorded because neither
design authority had it: the T114 runs the Adafruit bootloader over SoftDevice
S140 v6, so flash below `0x26000` is MBR and SoftDevice, and `0xEC000` upward is
the bootloader, its settings page, and the MBR parameter page. Writing outside
`0x26000..0xEC000` destroys DFU. The store therefore takes the top two pages of
the application region, and `FLASH` shrinks by exactly those two pages so the
linker can never place code into them.

Landed:

- `radio-hand::store`, the A/B slot record: magic, version, body length,
  sequence, CRC-32, opaque body. Word-padded so an encoded record is a legal
  NVMC write length.
- `memory.x` carve to `FLASH 0x26000..0xEA000` and `STORE 0xEA000..0xEC000`,
  with `build.rs` parsing both regions out of the one file and failing the build
  unless `FLASH` ends exactly where `STORE` begins, the store is page-aligned,
  the store is two pages, and the store stays below the bootloader.
- T114 glue: `Nvmc` for the pages, `Rng` with bias correction on for key
  material, load-or-create at boot ahead of every other task because erase
  stalls the CPU, and read-back verification so failing flash surfaces at the
  write instead of one power cycle later.
- A boot line over USB reporting slot and sequence. Key material is never
  rendered, and slot plus sequence is what actually proves persistence.

Receipts:

- 19 store tests pass, including torn-write recovery, blank-versus-corrupt,
  single-bit flips in body and header, padding outside the checksum, and slot
  alternation. The CRC is checked against the published `123456789` vector, so
  it is IEEE CRC-32 rather than a self-consistent invention.
- Flash 75,266 of 802,816 bytes (9.38%), against 73,666 before N0.
- text 74,870, data 368, bss 10,916, so static RAM is 11,284 of 237,568 (4.75%).
- Heap high-water is zero by construction: the image has no allocator.
- Clippy clean at `-D warnings` for `radio-hand` and for the firmware.

**N0 hardware receipt, 2026-07-31, T114 on COM10 against a V4 on COM6.**

Control first: the pre-N0 image answered on COM10 with its banner and no
`identity=` line, and caught an unrelated 83-byte LongFast frame at -113 dBm
while listening, so the board was known good before anything was flashed.

- v16 flashed over serial DFU, 75,268 bytes, 4.85s.
- First boot: `identity=created slot=A`. Blank flash, identity generated from
  the hardware RNG, written to slot A, and read back, since a failed read-back
  reports `identity=unavailable` instead.
- **The SoftDevice risk is retired.** The image links above S140 and never
  enables it, and direct NVMC access worked, which was the open question and the
  thing that would have faulted on the first write.
- RF regression through the existing `direct_phy_bytes` harness: 4,096 bytes
  byte-exact V4 to T114 in 26.2s, `RETINUE DIRECT-PHY BYTES HEADED PASSED`, and
  the payload independently confirmed by matching SHA-256. The flash carve did
  not disturb the radio path.
- Reset and full application reflash, then: `identity=loaded slot=A seq=0`. Same
  slot, same sequence, so the record was read rather than reminted. Because DFU
  rewrites only the application from `0x26000`, this is the stronger claim: the
  identity survives an application update, not merely a reboot.

Task storage, read off the ELF: the largest future is `screen_task` at 6,776
bytes, then `embassy_main` at 1,968, `usb_task` at 576, and `button_task` at
136, so all four pools together are 9,456 of the 10,916 bss. The store's buffers
are stack rather than future, because nothing in the load path awaits.

**Scope call: settings over the wire moves to N2.** N0 is the substrate, meaning
entropy and persistence, and both are proven. What a stored record *contains*
beyond the identity belongs with whoever owns config apply, and that is
`radio-hand` at N2. Persisting a profile before the crate that applies profiles
exists would put the wire format in the firmware and move it again immediately.

Open:

- **Power loss specifically.** Reset and reflash are proven; pulling the plug is
  not, and it is the literal wording of the done condition.

**N1, first half, 2026-07-31.** `channel` and `reliable` are bounded; `resource`
and `address_book` are not, and the crate is still `std`.

The shape, after two corrections. The first: `Channel<C: Capacity>` reading
associated consts **does not compile on stable**, because feeding `C::WINDOW` to
a `heapless` collection needs `generic_const_exprs`. The second: I argued for
associated types on the grounds that a static bound on `reorder` would cost 256
× 423 bytes, and that argument assumed alloc-free payloads. N1's target is
`no_std + alloc`, so payloads stay heap-allocated and an entry costs a `Vec`
header. What needs bounding is entry *count*, which is the thing that grows
without limit. So: const-generic parameters with desktop defaults, and
[`capacity::small_types`] aliases so a board writes a name rather than five
positional arguments.

Bounding found two real defects, which is the argument for doing it at all:

- `reliable`'s `sent` map leaked, recorded above.
- `endpoint`'s reliable driver dropped application bytes. `write` could not fail
  before, so the driver ignored its result; with a bounded queue a short write
  silently discarded the remainder. It now holds refused bytes, stops reading
  from the app while any are pending, retries the eof frame until the queue
  takes it, and refuses to call the stream done while either is outstanding.

Backpressure now runs the length of the stack: a full inbox withholds the link
proof, exactly as a full reorder buffer already did, so the peer stops sending
rather than the receiver growing. `Buffer::write` returns bytes accepted and
`finish` returns whether the eof was queued.

Receipts: 98 lib and 61 integration tests pass, including a new end-to-end test
that carries 3,000 bytes through `SmallReliableChannel`, proving the board
profile runs the same code as the desktop. Clippy clean at `-D warnings
--no-deps`.

**N1 COMPLETE, 2026-07-31.** All four done conditions are met.

`resource` and `address_book` took runtime caps rather than fixed tables, and
the distinction is deliberate: `channel`'s tables are small and fixed, so
`heapless` suits them, while these two are policy-sized or data-driven and a
structural bound would commit the desktop's whole worst case as static storage.

The resource cap closed a real hole. `Advertisement.parts` is a wire `u64`
chosen by the sender and was never validated, so a peer could claim an
arbitrarily large resource and this node would hold reassembly state for it.
`Incoming` now refuses past its ceiling with `Error::CapacityExceeded`, and
`ingest_hmu` stops appending at the same ceiling; bounding `order` bounds
`parts` too, since a part is only accepted for a hash already listed there.
`AddressBook` holds `max_peers` destinations and reports `Learned`, `Refreshed`
or `Refused`; a full book still refreshes what it knows, so a flood of unknown
destinations cannot displace established peers.

The `no_std + alloc` flip: `#![no_std]` with `extern crate alloc`, and `std`
back only for the tokio shell and the test harness, which genuinely have an
operating system. The remaining `HashMap`s became `alloc::collections::BTreeMap`
rather than taking on `hashbrown` — the keys are `Ord` and the tables are small.

- `cargo check -p retinue --no-default-features --target thumbv7em-none-eabihf`
  succeeds. The sans-io core cross-compiles for the T114.
- 162 tests across 15 suites pass, unchanged in behaviour.
- Capacity is typed and visible everywhere it can be reached: `CapacityExceeded`
  on an oversized advertisement, `Err` from `Channel::send`, `Ingested::Refused`
  from the address book, `unrecorded()` on the reliable channel, and short
  returns from `Buffer::write`.
- Clippy clean at `-D warnings --no-deps` in **both** configurations.

One honest qualification on "no collection grows without a declared bound":
that holds for the receive paths, which are what a peer controls. `Outgoing`'s
`by_hash` and `map_hashes` are sender-side and bounded by the data this node
chose to send. `endpoint`'s own tables are the desktop shell and out of N1's
scope by design.

**N1 review pass, 2026-07-31, before starting N2.** Method: re-read the paths
the bounding changed with fresh eyes, and run every CI configuration the
sessions above had skipped. Findings, worst first:

1. **A liveness bug in the bounded inbox, mine, now fixed.** A frame buffered
   out of order is proved on arrival, so the sender never retransmits it. When
   the inbox filled mid-drain, the next contiguous frame stranded in `reorder`
   with `recv_next` pointing at it, and the only path that re-ran the drain was
   an arrival carrying exactly `recv_next`, which the proof guarantees never
   comes. The app could empty the inbox and the stream still stalled with the
   data sitting on the receiver. The unbounded inbox had made "the drain always
   completes" true by construction, and the bound silently falsified it. Fix:
   the reorder drain is now a `pump` that also runs on the read path, so the
   application making room is what frees the stranded frame. Regression test
   `a_proved_frame_never_strands_when_the_inbox_fills_mid_drain` pins it at
   `QUEUE = 2`. Lesson for N2 and N3: every place a bound replaces "always
   completes", ask what used to be driven by the completing loop.
2. **CI line 35 (`cargo test -p retinue --no-default-features`) was already
   red before this plan's work** — verified at the plan commit in a scratch
   worktree. Eight test files predating this plan lacked `required-features`
   declarations; five import tokio-gated modules unconditionally. The five are
   now declared; `link_session`, `oracle_fixtures`, and `tcp_framing` are
   genuinely sans-io and still run without default features.
3. **The MSRV job had never been run on this work.** Now run on the installed
   1.88.0: all suites pass, in both feature configurations, so heapless 0.9,
   the const-generic defaults, and the `no_std` flip hold on the declared
   floor.
4. **`cargo fmt --check` failed on `radio-hand`** (committed unformatted at
   N0) and on the python-edited files. Formatted; check is clean.
5. **One broken rustdoc link** (`capacity::SmallChannel` for
   `capacity::small_types::SmallChannel`). Fixed; docs build warning-free.
6. **Pre-existing, not fixed here:** the exact CI clippy line
   (`--all-targets --all-features -- -D warnings` at the workspace root) fails
   in `vendor/embedded-graphics-core` on `doc list item overindented`, a lint
   newer than the vendor copy. Untouched by this plan's work; the vendored
   crate keeps its own lint policy and the fix belongs with the vendoring.

Re-reads that found nothing wrong, recorded so they are not re-litigated:
`pending` in the endpoint driver is bounded by one `WRITE_CHUNK` because the
read arm gates on `pending.is_empty()`; the `poll_transmit` rollback pops only
fresh envelopes because retransmits are appended after the fill loop; the
sweep in `on_proof` keeps duplicate hashes for a sequence until it is proved,
which is correct because either transmission's proof may return.

**N2 first half, 2026-07-31, and a finding that outranks it.**

Landed in `radio-hand`: `phy` (the wire-value-to-`lora-modulation` mapping, which
was duplicated byte for byte) and `service::apply_profile` (the sixty lines of
nesting each image spelled out to apply a host profile, generic over
`RadioKind` so neither board's SPI arrangement reaches it). `selvage` now names
the config result codes `CONFIG_ACCEPTED/MALFORMED/UNSUPPORTED/RADIO_FAULT`,
matching the `UI_SNAPSHOT_*` precedent it already set; they had been bare
numbers written twice. `main.rs` is 801 -> 709 lines on the T114 and 757 -> 669
on the V4.

The `phy` extraction produced an image **byte-identical** to the flashed,
RF-proven v16 (sha256 `b99102ae...`), so that step needed no hardware at all.
The `service` extraction grew the image 384 bytes, so it did.

**THE RF RECEIPTS IN THIS PROJECT HAVE BEEN SINGLE-RUN, AND THE PATH IS FLAKY.**
The v17 RF regression failed. Rather than accept or dismiss that, it was run as
an A/B against v16, the image N0 proved:

| image | result |
|---|---|
| v17 (refactored) | 5 of 8 passed |
| v16 (the N0-proven control) | 8 of 10 passed |

The control's first block was 4 of 4, which looked like a clean regression
signal; extending it to ten runs showed that block was luck. At these sample
sizes the two rates are statistically indistinguishable (Fisher exact on
[8,2] vs [5,3] gives p ~ 0.6), so there is **no evidence the extraction changed
RF behaviour**, and clear evidence that **a single passing run is not a
receipt**.

This applies backwards. The 2026-07-23 direct-PHY acceptance and N0's own RF
check were each a single run on a shared ISM band; the N0 control capture even
recorded an unrelated LongFast frame at -113 dBm, so ambient traffic on
906.875 MHz is documented. Neither receipt is wrong, but both are weaker than
they read.

**Consequence for N4 and N5:** their done conditions say the board "exchanges
reliable data" and "recovers after induced loss". Those must be written as a
pass count out of a stated number of runs, not as a single pass, or they will
certify a path that fails a third of the time. The gate receipts above should
be read with the same caution.

Still open in N2: the command dispatch itself (identical in both images apart
from the host transport) needs a `HostLink` seam before it can move. One real
behavioural difference to settle first: the T114 checks every `write_all` result
and breaks the connection loop on failure, while the V4 discards it. Unifying
them changes one board's behaviour, so it is a decision rather than a
refactor.

**HostLink seam ruled, 2026-08-01.** The design is structural decision 3 above,
made against the wider hardware ecosystem (RAK4631, T-Echo, T-Beam, RP2040
boards) and against the compatibility personalities that will ride the same
transports. The write-divergence question dissolved rather than being decided:
`Detached` is a transport fact, USB impls report it and UART impls cannot, so
both boards' behaviours fall out of one shared loop. The one deliberate change
is the V4 USB personality gaining break-on-detach, which is a correction. The
receipt rule was also added to the gates preamble, and N2, N4, and N5's done
conditions now demand counted blocks instead of single passes. Next session
builds: trait plus T114 impl, counted A/B, then the V4's two impls.

Next is N2: move the radio service out of the two firmware `main.rs` files into
`radio-hand`, which N0 already founded.

Harness note: the direct-PHY harness is `crates/retinue/examples/direct_phy_*`
behind the `tulle-radio` feature, alongside the `oracle/` drivers. There is no
`Code/testing/retinue/`, which is where the family convention would put it.
