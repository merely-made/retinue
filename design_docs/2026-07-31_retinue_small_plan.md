# retinue-small plan

**Status:** every gate closed on the software side. N0 proven (power-loss
unplug leg open); N1–N4 complete; N5 complete except the current figures
(meter needed — Mark's leg); N6 complete — panels drive from board-local
state, RF forwarding survives host attach and detach. `retinue-small` runs:
the board persists its identity, announces, links, exchanges byte-exact data
with loss recovery, survives abuse and mid-transfer reboots, and shows its own
state on its own face. What remains beyond the gates: pressure points 1
(regulatory floor), 2 (channel citizenship), and the supervised reboot are
BUILT; pressure point 3
(quiet-window writes) is DISCHARGED by verification rather than machinery, with
the one gap it exposed closed. Channel citizenship ships built but defaulted
off, on measurement. The airtime-derived retry floors are BUILT
(`tulle::pacing`) and validated, but did not turn out to be what gates
citizenship. Still open: cheaper carrier sense or skipping it inside an owned
turn, and the foreign-mesh channels behind their own gates.
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

**N3: the executive and the channel trait, 2026-08-02.** Structural decision 4's
two pieces exist, and the modem personality is the first channel behind them.

**`Executive` is the authority boundary, not a tidying.** It owns `lora`
privately, so a channel transmits by asking. That is the whole point: the
regulatory clamp (pressure point 1) and channel citizenship (pressure point 2)
both have to sit below every personality in exactly one place, and now there is
exactly one place for them to sit. Nothing clamps yet; the seam they need is
built, and it cost nothing to put it in first.

It is a **borrowed view** rather than an owner, which turned out to matter
immediately. The T114 holds one for the whole of `main` and gets the full
boundary — its own `lora` is unreachable for as long as the executive lives. The
V4 cannot: its low-power proof polls `lora.rx()` by hand so it can enter
Light-sleep from inside the future, so it keeps its own hand on the radio and
constructs an executive per dispatch call. A view lets one board adopt the seam
completely and the other incrementally, instead of forcing a choice between
converting the sleep work now and not building the seam at all.

**`serve` takes an event rather than owning a loop, and the deviation is
deliberate.** The ruling names start, serve, stop, and the obvious reading gives
`serve` the whole loop. That contradicts the same decision's other half, which
keeps "the executor, the select between host and radio, and the text probes" in
the firmware binary — the T114's `GPREGRET` bootloader entry and its display
diagnostic are board facts, and a loop-owning channel would have to absorb them
or every future channel would. So the firmware keeps the select and hands over
what its probes did not claim. One consequence worth naming: probes are only
safe at a frame boundary, and only the channel knows where its parser is, so
`ChannelInfo::at_boundary` exists to let the firmware ask.

**The heartbeat is absent by default, and that is a real decision.** A channel
with no timer gets `core::future::pending()` rather than a fast tick with an
empty body, so the modem's select waits on exactly the two things it waited on
before this existed. It also avoids a hazard: at SF11/250 kHz a 255-byte frame
is seconds long, and a periodic wake that cancelled the receive future could
have made reception impossible. (It would in fact have been safe — the SX1262
keeps receiving in the background and holds DIO1 high until its flags are
cleared, which is why cancellation is survivable at all — but a battery board
should not pay for a wake it ignores.)

**Measured, against v21:** flash 156,994 → **155,906** (19.5%), static RAM
66,908 → **66,956** (28%). The refactor gave back about a kilobyte, because one
shared transmit path replaced the inlined copies.

**Counted RF receipt, v22 against the COM6 control peer: 8 of 8, all outputs
byte-exact.** The best result in the series (v16 8 of 10, v19 7 of 8, v21 7 of
8, wired V4 8 of 8). All four text probes answer, and the SX1262 diagnostic is
clean. Tenth flash; `node=599997c8 identity=loaded slot=A seq=0` unchanged.

**Two process notes.** `main.rs` went *up* to 636 lines under the refactor, over
the 600 ceiling, so the board's SX1262 wiring moved to `radio.rs` and main came
down to 488. That code movement rebuilt the image, which invalidated the receipt
just taken — same byte count, but a receipt attaches to a binary, not to a size
— so it was reflashed and re-run. Both counted blocks passed 8 of 8; the one
above is the committed image. And `radio-hand`'s doc comment no longer claims
the crate is allocation-free, which stopped being true when the node arrived.

Remaining for N3: the node channel behind the trait, boot selection from
`Settings::channel`, and the fixture-corpus replay.

**N3: the node channel and the selector, 2026-08-02.** The board boots into a
personality chosen from its persisted settings, and the node channel drives
`Node` over the radio: ingest on every received frame, poll on its own clock,
`Action::Send` out through the executive's one transmit path. `channel`,
`channel modem`, and `channel node` select and reboot.

**`BoardStore` joins the executive**, because the ruling puts the flash and the
entropy there beside the radio. One trait rather than two, since the T114's
`SettingsStore` holds both the NVMC pages and the hardware RNG and a pair would
need two mutable borrows of one object. `random` is fallible on purpose: a board
without a source refuses rather than announcing itself with zeros, which is also
what makes the heltec doc's entropy-failure case representable at all. The V4
gets `NoStore` and says plainly that it has neither.

**A defect only hardware found, and the best thing this session produced.** The
heartbeat was created inside the host-session loop, so the node's clock ran only
while a cable was plugged in. A fresh unattended boot reported `tx=0`; the
earlier `tx=1` was an artifact of the probe reading the counter being itself the
host that started the clock. A node does not stop being a node because nobody is
watching. `ChannelInfo::without_host` now says which personalities must keep
running unattended — false for the modem, which genuinely has nothing to decide
without a host and would only burn power receiving — and `await_host` serves the
radio and the clock while waiting for one. It is board-agnostic, so it belongs
in `radio-hand` rather than in a firmware, which also took it out of a `main.rs`
that had drifted over the ceiling again.

Worth stating because it generalises: **a receipt taken while the bench is
attached cannot see a bug about not being attached.** Every earlier RF receipt
in this series ran with a host driving both boards, which is exactly the
condition under which this defect is invisible.

**Measured, v24:** flash **209,652** (26.2%), static RAM **74,204** (31%).
Against v22's 155,906, the node channel costs about 54 KB — much of it the
protocol paths that only became reachable once something actually called
`ingest` and `poll`, since a merely-resident `Node` let the linker discard them.
Two channels at 26% of flash keeps decision 4's residency answer comfortable.

**Receipts.**

- Counted RF block, v24 modem channel against the COM6 control peer: **8 of 8**,
  all outputs byte-exact.
- The node channel announces within one beat of an unattended boot: `tx=1
  unsent=0 unseeded=0`, so the entropy path, the announce build, and the
  transmit all ran with nothing attached.
- Channel switching persisted across five reboots, the A/B slot alternating
  `A seq=0 → B seq=1 → … → A seq=6` with `node=599997c8` unchanged throughout.
  This is the first hardware exercise of `SettingsStore::save`; every prior
  receipt only ever read.

Two bench notes. The first `channel set` reply arrived truncated at thirteen
bytes, because a CDC write returning means the packet was queued rather than
sent; the reset now waits 250 ms, and the `bootloader` probe's 20 ms was
evidently always on the edge. And a raw serial peek at the V4 while the T114
announced showed only a stale `EVENT_TX` from the previous harness run — the
harness leaves both boards on SF8/sync 0x12 while a rebooted board returns to
SF11/sync 0x2b, so the two were not on the same air. Hearing an announce is N4's
receipt and wants a real listener, not a serial peek.

**N3 COMPLETE, 2026-08-02: board and desk produce identical Actions.** The gate's
done condition asked for exactly this, and it is now a measurement.

**The machinery.** `radio_hand::replay` is one byte form for a set of Actions
and one fixed identity both sides build a node from. It sits behind its own
feature rather than under `node`, because nothing in it touches a radio and
keeping it clear of `lora-phy`'s ungated `defmt` is what lets a host test link
it — so half the comparison lives in CI and half is a hardware receipt, which is
the same split every RF claim here already uses.

**What makes the comparison mean anything is that every input is pinned.** The
identity is a test key rather than the board's own, so both sides are the same
node. The clock is passed in rather than read, so neither depends on when it
ran. And `poll`'s entropy is caller-supplied — a decision the protocol layer
made at N3's start for exactly this reason, which pays here. What remains is the
protocol's own decisions, which is the only thing being compared.

**The receipt: 13 of 13, byte for byte.** Twelve oracle-captured fixtures
through `ingest` — four valid announces that must learn the same destination,
six invalid ones that must produce nothing, and two link fixtures addressed
elsewhere — plus a first `poll`, whose 231-byte signed announce the board built
byte-identically to the desktop's. That last one is the strongest of the set: it
exercises key derivation, signing, and packet framing, not just parsing.

**Heap, measured rather than asserted.** A `heap` probe reports live allocation
on demand, which is what the heltec doc's high-water condition actually needs
now that something allocates. **6,304 of 49,152 bytes (12.8%)** with a replay
node live, and **zero** in ordinary node operation — N1's bounded tables are all
inline, so nothing persists on the heap until a replay node or a payload-carrying
action exists.

**A design point that earned its keep.** The node channel now assembles host
lines, because a replay line is several hundred bytes arriving across many
64-byte host reads. `ChannelInfo::at_boundary` — added a commit earlier for the
modem's framed parser — is what stops a fragment of one being read as a board
probe. It was speculative when written and load-bearing two days later.

**v25:** flash 212,820 (26.6%), static RAM 75,340 (32%). Counted RF block on the
modem channel: **8 of 8**. 256 tests.

**N4 COMPLETE, 2026-08-03: desktop Retinue links with the board over real RF,
9 of 10.** The board announces from its own clock, the desktop hears it through
a V4 modem, opens a link, and the board's proof completes it — one request, one
proof, ~10.3 s per pass at SF11/250 kHz, a distinct ephemeral link id every run.
The counted block reboots the board per run, so each pass is the whole loop:
boot → announce → discovery → request → proof → established. One setup timeout
logged as the failure.

Two defects, both invisible to every receipt taken before this gate:

**The board's unattended wait never waited.** embassy-usb's `wait_connection()`
is `wait_enabled()`: it completes when the *device* is configured, which a
plugged-in board always is, terminal or none. So the board started phantom
sessions against nobody, banner writes into an unread endpoint stalled, and the
select that should have been listening never ran — announces went out (the beat
still fired between stalls), which made the board look alive while it was deaf.
Diagnosed by the executive's new `AirDiag` counters after three cornered-but-
inconclusive instrumented runs: `beats=0 frames=0` with `txok=1` says the
unattended wait was never reached, which no amount of RF observation could
distinguish from a radio fault. `UsbHost::attached()` now gates on DTR — the
thing a terminal actually asserts. The probe script's own comment claiming
wait_connection gates on DTR was my earlier wrong inference, recorded and
corrected.

**The desktop's link-setup retry collided with the answer it was retrying
for.** `DEFAULT_LINK_SETUP_RETRY_MS = 2000`; at SF11 the request is ~0.9 s of
air and the proof ~1.4 s more, so every retry fired straight into the proof —
while the half-duplex board missed that retry because it was still transmitting
its answer. Both sides lost in perfect synchrony, run after run: the board
held `links=1` with four proofs sent, and the desktop timed out having heard
none of them. The harness sets a 12 s retry; **a follow-up for the tulle lane:
link-setup and reliable-channel retry floors must be derived from the profile's
airtime, not inherited from defaults tuned for fast links.**

**The three-radio method earned a permanent place.** With both V4s on the bench,
one carries the desktop and the other sniffs the same air (`node_sniff`,
printing decoded packet type, destination, RSSI and full hex). That is what
split "the proof was on the air" from "the proof reached the desktop process"
— and the harness's frame-logging radio wrapper (`LoggedRadio` in `node_link`)
closed the remaining gap by showing the desktop's own serial view. A stale-
serial-buffer hazard surfaced on the way: a modem's unread EVENT_RX frames from
a previous run are delivered to whoever opens the port next, so a harness can
"hear" an announce from a board that has not spoken. Drain or ignore-first
applies; the fixed harness matches by destination, which is immune.

**Receipts, all on v27:**

- N4 counted block: **9 of 10** boot-to-link passes, every pass ~10.3 s, every
  failure logged (one setup timeout).
- Board diag on a passing run: `armed=4 armfail=0 rxok=1 rxerr=0 txok=2
  txerr=0 beats=2 frames=1` — one request heard, one proof sent, nothing
  wasted, and the unattended wait demonstrably waiting.
- Modem regression on the same image: **7 of 8** byte-exact, in the established
  band, so the DTR gate did not disturb the personality that already shipped.

The double-announce anomaly seen mid-diagnosis is explained by the same root
cause: phantom sessions restarting around stalled writes. It has not
reproduced since the DTR gate landed — announce cadence was exactly one per
boot across all ten counted runs.

**N5 first leg, 2026-08-03: byte-exact data both directions, 8 of 8, and the
survive-loss mechanism built.** The exchange: desktop publishes a resource to
the board over the link, the board's loopback service — its first application —
publishes the same bytes back, and one comparison proves both directions and
both halves of the board's transfer machinery. Counted block: **8 of 8**
boot-to-exchange passes, 1024 bytes each way, byte-exact every time, uniform
~56 s per run at SF11.

**The first hardware run failed exactly where N3's note said it would.** One
2-second frame lost on the air stalled a five-part transfer forever: the
receiver waited (correctly, per its design) for a retransmit driver that did
not exist. "Link timeouts and resource retransmits `poll` will own as the
gates land" — this is that gate. `Node::poll` now redrives silent transfers on
`RESOURCE_RETRY_INTERVAL` (12 s; the airtime-derived floor remains the recorded
follow-up): a receiver re-requests exactly what it is missing, a sender
re-offers an advertisement nobody answered. `ingest` stops discarding `now`,
and transfers carry a last-activity stamp. The counted block shows the
mechanism load-bearing, not decorative: every run engages it and completes in
the same ~56 s.

**Found while wiring it: an IV-reuse defect.** The derived resource IV counter
was a per-call local, so every ingest call replayed the same IV sequence under
the same link key — against the sealing contract's "must not repeat". The
counter is node state now, never reset, and a test pins that the same request
sealed twice differs. Desk tests also cover the lost-part re-request (with a
before/after-the-interval boundary), the lost-advertisement re-offer, and the
existing corpus is untouched: 123 protocol tests.

**Board on a passing run:** `rxok=14 txok=13 armfail=0 rxerr=0 txerr=0
echoes=1 echorefused=0`, heap high-water 1,392 of 49,152 with the link still
established. The `node` probe now reports the echo counters.

Remaining for N5: recovery after a mid-transfer reboot; the N1 capacity errors
re-proven under live traffic (full tables rejecting with typed outcomes while
the node stays operational); the heltec doc's adversarial set (fuzzed frames,
full route table, full queue, entropy failure, flash corruption, resource
cancellation); and the idle/receive/transmit current figures, which need a
meter on the bench.

**N5 survival and adversarial set, 2026-08-03: receipted on hardware, current
figures excepted.** Refusals are now typed, counted outcomes on the node —
`refused_peers`, `refused_offers` joining `refused_links`, all in the `node`
probe — because a bound that rejects silently cannot be told from a bound that
was never hit. `node_stress` carries the legs over real RF, and every leg ends
in the same verification, a full byte-exact exchange, since "stays operational"
is a claim until traffic passes after the abuse.

- **Mid-transfer reboot:** the board killed mid-publish with 8 parts served;
  the desktop fails typed ("timed out after serving 8 requested part(s)"), the
  next boot exchanges byte-exact, and the identity rides through the kill.
- **Fuzz:** 30 hostile frames received in one boot — 3 structurally
  undecodable and counted, the rest parsed and dropped by validation, which is
  the correct split — and the exchange passes.
- **Flood under live traffic:** 40 valid announces transmitted from the second
  modem while an exchange ran on the first. The exchange completed byte-exact
  through the collisions, 118 s against the clean 56 s, the loss recovery
  visibly load-bearing. This is the three-radio method graduated from
  diagnosis to receipt.
- **Flood in quiet air:** `rx=40 peers=32 refusedpeers=8`. The cap, exact,
  with every refusal counted.
- **Links:** six opens against four slots — four up and held to the end, two
  refused, every refused request counted (`refusedlinks=4`: two attempts and
  their retries).
- **Bigoffer:** a 20 KiB offer against the 32-part ceiling, refused on every
  re-advertisement (`refusedoffers=4`) without ever holding a receiver slot,
  and a byte-exact exchange on the same boot after.

**The board caught the harness lying.** The first flood generator varied
byte 0 of the x25519 secret; clamping (`k[0] &= 248`) collapses that into five
distinct keys, so forty "identities" were five peers refreshing — and the
board's `peers=5`, deterministic across two runs, was correct while the
harness was wrong. Desk reproduction confirmed it in seconds
(`learned=40 peers=5 refused=0`). The generator varies byte 1 now, and a desk
test pins forty genuinely distinct identities filling the book to 32 with 8
counted refusals. Lesson recorded: identity-generation in test harnesses must
respect key clamping, and a deterministic "wrong" number from hardware
deserves belief before the harness does.

Entropy failure and the action-queue bound stay desk-proven (the `unseeded`
counter and `Actions::overflowed`); the T114's RNG is a hardware peripheral
that cannot honestly be made to fail from software. Flash corruption was
proven on hardware at N0. **Still open for N5: the idle/receive/transmit
current figures, which need a meter on the bench — Mark's leg.**

**N6 COMPLETE, 2026-08-03: the panels drive from board-local state.** The
Identity, Links, Peers, and Traffic pages render from the node's own state —
the same `HostSnapshot` shape a host used to project, now genuine:
`Personality::Retinue`, the board's real destination as fingerprint and
address tail, live link counts, the three most recently heard peers, and a
local event line. Published on every beat, so 5 s cadence against 15 s
validity keeps the panels fresh with no host attached — that cadence *is* the
done condition's mechanism.

Two small pieces made the panels genuine rather than merely local. The channel
keeps the one thing the address book deliberately does not, a clock — a small
recency table stamped from `Action::Learned` — so the Peers panel shows real
ages. And a `face` probe prints the exact snapshot the screen renders, so
panel content is assertable over the wire while the TFT paints the same
struct.

**Receipts, on the committed image:**

- **Panels with the host detached:** a 40-announce flood into an unattended
  board, then attach and read: `face name=retinue.node peers=[a016f4d9
  age=18s 1db3de06 age=20s e328a0f8 age=21s] overflow=29`, with `ui` showing
  `host=fresh` from publishes that happened while nothing was attached. The
  ages are genuine — the flood ended about 18 s before the probe.
- **RF forwarding across disconnect and reconnect:** a host attached
  mid-exchange, read the live face (`links=1 event=echo 1024b`, the echo just
  queued), detached, and the exchange completed byte-exact in the normal
  55.8 s. The attach/detach cost the transfer nothing.

The `ui` diagnostic's `host=fresh` label now reports a locally-fed face; the
field name is a hangover from the projection era and can rename when the
surface is next touched. The physical-screen leg — eyeballing the four pages
on the TFT through the button cycle — is Mark's, in the standing
v12-acceptance style.

**Pressure point 1 BUILT, 2026-08-03: the regulatory floor, in Mark's shape.**
`radio-hand::region` is the compliance table as data — US915, EU868, EU433,
ANZ915, JP920, each with frequency bounds, power cap, duty limit, and the
trunk's default carrier. `Region::Unset` is zero *because* the settings byte
it decodes from was reserved-as-zero: every record written before regions
existed upgrades into "no region, no transmit", never into someone else's
rules. The migration was proven live: the board's own record came up
`region=unset` after the flash and the board went silent.

**Enforcement sits at the one line every transmission already crosses.** The
executive's `transmit` refuses with no region (`TX_NO_REGION`, counted) and
refuses over a spent duty budget (`TX_OVER_DUTY`); the ledger charges
*measured* airtime — the boot announce cost `duty=1524ms`, which is the
167-byte SF11 frame's real time on air. `apply_profile` rejects out-of-band
frequencies whole (`CONFIG_OUT_OF_REGION`) and clamps power to
min(request, region, hardware +22), applying and reporting the clamped value.
A region-less board still tunes — receiving is unregulated — and the banner
says `region=unset` rather than pretending.

**Receipts on v32:** the upgraded board attempted one announce, was refused
(`noregion=1`), and put nothing on the air across a sniffed window; after
`region us915` (persisted, surviving a further DFU reflash), the boot announce
went out and a byte-exact exchange passed; an EU carrier on the US board was
rejected with wire result 4 end to end through tulle; a 30 dBm request came
online clamped; the counted modem block held at 7 of 8.

**Edges owned honestly:** the V4 has no store, so its region is a build fact
(US915, matching the bench) marked INTERIM — shipping that image outside the
US is wrong by construction until the board gains a settings backend. The
duty-refusal path (`TX_OVER_DUTY`) is enforced in code but has no hardware
receipt: exercising it means an EU region on US air, which the floor itself
forbids; it receipts when an EU bench exists. Table values follow the
Meshtastic table shape and **must be verified against current national rules
before shipping into a region** — the review being cheap is exactly why the
table is data. One design note for the future setup surface: `Region::choices`
already yields the pickable list, so first-boot region selection is a UI over
an existing iterator, not new firmware capability.

**The supervised reboot BUILT, 2026-08-03: crash residue, watchdog, and the
crash-loop refuge.** Structural decision 4's third boundary. Before this the
T114 linked `panic-halt` — any panic was a dead board until reflashed, the one
behavior a field device must never have. Now:

- **A panic writes its message to noinit RAM and reboots**; a hard fault does
  the same with the faulting address. The record survives the reset because
  cortex-m-rt leaves `.uninit` alone.
- **A hang is caught by the hardware watchdog** (8 s, petted by a task, so
  what it proves is that the executor still breathes). The next boot reads
  RESETREAS, counts it as a crash, and names it `msg=WATCHDOG` — a hang loop
  trips the same policy as a panic loop.
- **Three consecutive crash boots distrust the persisted personality**: the
  board boots the modem — the channel that needs nothing — and the banner says
  `FALLBACK=modem`. The persisted settings are never touched by falling back,
  and the count decays after a clean minute, so the refuge is not a trap.
- The banner names the reset reason on every boot; `crash` reports the
  residue, `crash clear` forgets it; `crashtest` and `hangtest` are the
  undisguised bench hooks (a host that can reach them can already reboot the
  board via `bootloader`).

`panic-halt` is removed from the T114 — not discretionary: the design installs
its own `#[panic_handler]` and the linker permits exactly one.

**Receipts, all live on v33:** `crashtest` reboots the board itself with
`reset=soft crash=1` and the residue carrying the exact panic file and line;
three crashes produce `crash=3 FALLBACK=modem` with the node banner line
absent while `channel` still reports node; `hangtest` starves the executor and
the board returns inside the timeout with `reset=watchdog`; `crash clear` plus
`channel node` exits the refuge; one crash plus seventy seconds decays the
count to zero with the message retained; and a byte-exact exchange passes
under the armed watchdog.

**Honest limit, recorded:** a stuck `await` that still yields keeps the
executor breathing and the watchdog fed — that class needs per-turn deadlines,
future work. The V4 keeps `esp-backtrace` semantics and gains none of this
yet. The ruling's "store the crash record through the A/B flash record at the
next boot" is deliberately deferred: the RAM residue covers loop detection and
post-mortem across soft resets, which is the load-bearing part; flash
persistence of the last message across power loss can ride a later settings
change.

**Pressure point 2 BUILT, 2026-08-03: channel citizenship.** Listen before talk
in the executive, beside the regulatory floor, because every transmission
already crossed that one line. The collision-mitigation notes' own verdict
chose this: the PHY stack they describe needs silicon these boards do not
have, and the MAC answer is the one that works on stock certified radios.

**The design turns entirely on what happens when the courtesy budget runs out,
and hardware settled it.** Deferring indefinitely *starves* against a peer that
transmits blind. The desktop's modem retransmits on its own timer, keeps the
channel occupied, the polite board never gets a turn, so the peer never gets
its answer and retransmits again — a livelock in which politeness is the
losing strategy. Measured: the byte-exact modem exchange that had run 7-of-8
for weeks fell to **4 of 8**, and the counters convicted the right thing —
`cadgiveup=0` said nothing was dropped, so every frame merely arrived too late.
Passing runs took 22-25 s against a 90 s timeout, so the failures were stalls,
not slowness.

So courtesy is bounded: defer while it is cheap, then take the turn, counted
as `cadover`. A collision costs one retransmit; starvation costs a node that
never speaks. Where a region **mandates** carrier sense the refusal is correct
instead, so `RegionProfile` gains `listen_required` — true for JP920 under
ARIB STD-T108, false for the FCC part 15.247 entries — and `TX_CHANNEL_BUSY`
is what those regions return.

**A second defect the same jam exposed.** `Node::poll` stamps its announce when
it *decides* to send one, so a frame the shell could not carry cost a whole
interval of invisibility: a ten-second jam making the board unfindable for ten
minutes. `Node::retry_announce` lets a shell report the failure, and the node
channel schedules re-attempts on an exponential backoff to 32 beats. A fixed
retry budget was tried first and was wrong in an instructive way — it spent
itself while the channel was still busy and gave up exactly when the air
cleared, which is the worst possible moment.

**Receipts, and a correction I had to make to my own.** The first version of
this entry claimed the counted modem block at "8 of 8, the best the series has
recorded" as proof the bounded budget fixed the starvation. That was wrong, and
the counters had already said so: `cadgiveup=0` in *both* the 4-of-8 and the
8-of-8 blocks means the policy change could not have been the difference. I saw
the inconsistency and shipped the narrative anyway. Three more blocks settled
it — v36, differing only in probe reply ordering and comments, scored 3 of 8 —
and then the A/B that should have come first, same image and same session:

| listen-before-talk | counted modem block |
|---|---|
| on | **3 of 8** |
| off | **8 of 8** |

So the cost is real and CAD is its cause. It is *latency*, not loss:
`cadgiveup=0` and `cadover=0` throughout, so nothing was dropped and no frame
exhausted its budget. About 66 ms of carrier sense before every frame, plus
randomised backoffs, desyncs a request/response loop whose retry intervals were
tuned without it — the same defect N4 found in link setup, where retry floors
must derive from the profile's airtime rather than from constants picked for
fast links.

**Listen-before-talk therefore defaults OFF.** The mechanism stays built and
reachable with `cad on`, and is receipted where it matters: under `node_jam`
(a new bench instrument holding the channel busy) a deferring board shows
`cadbusy=8 cadover=1 txok=1 unsent=0` — deferring through the courtesy budget,
taking its turn, losing nothing. The same jam against the defer-forever build
gave `cadgiveup=1 txok=0 unsent=1`. Quiet air gives `cadclear=1` on the boot
announce. v37 with the corrected default: 6 of 8, inside the historical band.

**Turning it on by default is gated on the airtime-derived retry work**, which
was already a recorded follow-up. Doing it sooner trades a measured,
load-bearing receipt for a politeness nobody has asked for yet.

**The lesson, since it cost real time: a single counted block is not a receipt
when the mechanism does not explain it.** The receipt rule was written for
flaky RF, but its deeper point is that agreement between a number and a story
has to be checked, not assumed — and the A/B with a control on the same
hardware is what checks it.

**Pressure point 3 DISCHARGED by verification, 2026-08-03.** The ruling asks
that flash writes stage in RAM and commit in a declared quiet window. That
machinery has no caller, and the check is what establishes it rather than the
assumption: there are exactly two write paths, `load_or_create` on a boot that
finds nothing valid (before the radio is configured) and `save` from the
`region` and `channel` probes (each followed immediately by a reset).

**The check found one real gap.** Both probes returned `HostGone` if the reply
write failed — *before* the reset — so a host vanishing mid-reply left the
board running on stale in-memory settings with a page erase already spent
outside any quiet window. The reply is a courtesy; the reboot is the contract.
Both now reset unconditionally once the settings are committed, proven on
hardware by sending `region us915` and closing the port without reading:
`reset=soft`, sequence advanced, board back on the new settings.

The invariant is recorded in `store.rs` for whoever adds the third path: **a
write is either before the radio starts, or immediately before a reset.** A
caller that can do neither — a runtime-persisted peer table, a crash record
written where it happened — is the one that must build the staged window, and
the stale claim that "boot is the only place this module writes" is corrected
along with it.

**Costs and limits, owned.** CAD is 8 symbols at the current profile, about
66 ms at SF11/250 kHz, before every frame. Deferral adds a randomised 20-180 ms
per busy attempt. `cad_fault` fails *open* — a radio that cannot perform the
check transmits anyway — because failing closed would turn one bad register
write into a silent board, the exact class that cost a whole session at N4; a
region that mandates LBT would need that inverted. Dwell-time limits are not
implemented: no region entry in the table currently imposes one, and adding
them is a table field plus a check in the same place, not a new mechanism.

**Retry floors derived, 2026-08-03: `tulle::pacing`.** Three defects traced to
one root — a retry interval picked as a constant, tuned for links far faster
than LoRa. N4's link setup fired its retry into the proof it was waiting for;
N5's resource retry did the same a layer up; each was fixed by hand-picking a
bigger number, which worked without saying why. The module computes them from
the profile's own time on air, so a slower spreading factor moves the floors
with it. Floors, not schedules: a caller may wait longer, never less.

**Validated on the case that defeated the constant.** `node_link` with the
derived 3.0 s setup retry: **4 of 4**, and *faster* than the hand-picked 12 s —
5.2 to 9.8 s per pass against a uniform 10.3 s, because the retry now fits the
round trip instead of dwarfing it. The 2 s default it replaces failed every
run at N4. The counted modem block with derived resource pacing held at 7 of 8.

**It does not rescue channel citizenship, and that is the finding.** This work
was recorded as the follow-up that would let listen-before-talk default on. It
does not: with derived pacing and listen on, the modem block failed 6 of 6
before I stopped it — *worse* than the hand-picked pacing's 3 of 8, because a
shorter floor tightens the interaction rather than relieving it. The counters
say what is happening: `cadclear=770 cadbusy=258`, `cadover=0`,
`cadgiveup=0`, and `txok=1453` against `rxok=507` — the board transmitting
three times what it hears, answering retries that its own latency provoked.
Nothing is dropped; everything is late.

So the hypothesis that timing was the root cause is dead, and the honest
statement is narrower than before: **eight symbols of carrier sense before
every frame is too expensive for a half-duplex request/response exchange at
SF11, independent of retry derivation.** Two levers remain untried and are
recorded rather than guessed at:

- **Fewer CAD symbols.** `lora-phy` hardcodes `CADSymbols::_8`, Semtech's
  conservative recommendation, so trying two or four is a vendor patch.
- **Skip the check inside a turn the board already owns.** In a request/response
  protocol the reply is expected, the peer is listening, and carrier sense is
  answering a question nobody asked. This is the standard technique and the
  more likely fix.

Citizenship stays built, receipted under jam, reachable with `cad on`, and
defaulted off until one of those lands.

**The V4 gains a settings backend, 2026-08-03.** Pressure point 5 predicted the
shape and the prediction held: the record format, the A/B decision, the
settings body and the boot-line vocabulary ported unchanged, and only the
board's own `store.rs` is new. The INTERIM build-time region is gone; the V4
now persists identity and region like the T114, and gains the `region` probe
with the same vocabulary and the same persist-and-reset contract.

**One thing there would have shipped silently wrong.** `esp_hal::rng::Rng` is
true random on the ESP32-S3 only while the RF subsystem runs or an ADC feeds
the sampler, and this firmware runs neither — its radio is an external SX1262,
Wi-Fi and Bluetooth are off. The obvious wiring would have minted the board's
**identity key** from a pseudo-random sequence: a predictable private key that
nothing downstream could detect. The store holds a `TrngSource` over ADC1 for
its whole life instead, and `Trng::try_new` fails loudly if that source is ever
absent — `Error::NoEntropy` rather than a weak key. The T114's bias-corrected
physical noise source set the standard; a second board joins on those terms or
refuses.

**Receipts:** first boot `identity=created slot=A`; two resets both
`identity=loaded slot=A seq=0`; `region=unset` → `region=US915; rebooting` →
`region=US915` at `slot=B seq=1`. The regulatory floor proved itself on the
second board on the way: with the build-time `Us915` replaced by the persisted
`Unset`, the V4 rejected the harness profile with wire result 4 until a region
was chosen. Counted block with both boards on persisted settings: **6 of 6**.

**The `status` probe defect, chased and found to be a transport bug,
2026-08-04.** It returned nothing while every other probe answered. The
tempting conclusion was an empty banner; instrumenting the probe to report
`online.len()` and its content gave `len=128` with the text intact, which said
the data was present and the transport was losing it.

A USB bulk transfer ends when the host sees a packet *shorter* than the
endpoint size. `UsbHost::write_all` chunked into 64-byte packets and stopped,
so a payload that is an exact multiple of 64 left the host waiting for a
continuation that never came. The banner is 128 bytes, exactly two full
packets. Every other probe reply happened not to be a multiple of 64, which is
why only this one looked broken.

**It was not confined to the diagnostic surface.** The same trap sits on the
data path: an `EVENT_RX` carrying a 57-byte radio frame is 7 + 57 = 64 bytes
and was being dropped between board and host, as were 121-, 185- and 249-byte
frames. `write_all` now sends an explicit zero-length packet to terminate such
a transfer. The counted modem block stays at 7 of 8, in band, because this
harness's frames rarely land on those sizes; the defect was real regardless,
and any host protocol with fixed-size records would have hit it constantly.

The V4 is unaffected: its host link is a byte stream with an explicit flush
rather than raw bulk packets. Lesson worth keeping: a surface that answers
*everything except one thing* is more likely a size or boundary bug in the
transport than a logic bug in the one thing.

**The RNode channel, 2026-08-04: stock Reticulum software drives the board.**
The third channel, and the one that connects this firmware to software that
already exists. Sideband, MeshChat and NomadNet all drive an RNode; with
`channel rnode` selected they drive this board, with nothing in between.

**Built from the same oracle as the other half.** `tulle::rnode` speaks this
protocol as a host, pinned by a black-box capture of RNS 1.3.8 driving RNode
firmware 1.86 through a serial tee. That capture holds *both* directions, so
the device side is pinned by the same bytes: `radio-hand::rnode` decodes what
the host sent and answers what the device answered, and
`tests/rnode_device.rs` replays the capture frame for frame. Every command RNS
sent is one this device implements, our answers match the real device's byte
for byte, and the five settings rebuild the profile the capture's own config
block records. The GPL firmware source stays unread.

That gold test found one thing worth having found: 915 MHz is `36 89 CA C0`,
and its last byte *is* the frame delimiter. The escape path is not an edge case
in this protocol, it is the first setting every US host sends.

**The pieces, and where each sits.** KISS framing moved to `selvage::kiss`,
because both ends of the conversation are now in this workspace and a second
copy of the escape rules is a second place for a resync bug to hide; `tulle`
keeps its `Vec` storage, the board takes a fixed array, and the rules are
shared. The `Scan` state machine also gained a rule that matters more on a
board than on a desk: **a frame starts at a delimiter, so bytes before the
first one are not frame contents.** Without it a mistyped line would leave the
deframer permanently mid-frame, `at_boundary` would go false forever, and the
`channel modem` probe that switches the board back would stop being
recognised — a channel you can only leave by reflashing.

`ChannelInfo` gained `banner`. The board introduces itself in plain text on
every attach, which is right for a person on a terminal and wrong for a host
that opens with a binary frame; a real RNode says nothing until asked, and now
so does this one. Receipted directly: with the channel selected, attaching
reads back zero bytes where the modem channel reads 187.

**What it does not claim.** Speaking the host protocol is not being on the air
with a stock RNode. Those two firmwares were swept against each other across
seven sync words and inverted IQ in both directions and never crossed
(`2026-07-25_rnode_direct_phy_rf_opacity.md`); the protocol has no sync-word
command, so whatever stock RNode programs stays invisible from the host side.
This channel therefore uses **our** on-air settings — sync `0x12`, preamble 16,
the same as every other personality here — which is what makes our own boards
hear each other. The 500-byte lane is likewise not smuggled in: the protocol's
MTU is 500, this radio carries 255, and an over-long transmit is refused with
an `ERROR` frame carrying `TX_TOO_LONG` rather than truncated. The announce the
capture actually sent is 167 bytes, so finding peers fits.

**Receipts, on v41/v43, T114 on COM10:** `channel rnode` persists and boots;
`tulle`'s own RNode host — the one pinned to real hardware by gold tests —
detects the board, reads firmware 1.86, applies the profile and transmits
(`txok=1`, `duty=131ms`, which is that frame's real time on air, charged to the
regulatory floor). The `rnode` probe reports `radio=on overmtu=0 refused=0
unhandled=0 dropped=0 airmtu=255`. Flash cost: **+4,416 bytes**, the per-channel
linker receipt.

`rnode_phy_cross` is the new instrument, and it exists because
`rnode_bulk_probe` says in its own header that a direct-PHY listener cannot
serve as its receiver. That was true of *stock* RNode. Ours crosses: on v43,
**24 of 24 rnode to phy** and **21 of 24 the other way**, with the missing three
fully accounted for below.

**What that instrument found is bigger than the channel: the SX126x driver was
delivering CRC-failed packets as good ones.** In
`vendor/lora-phy/src/sx126x/mod.rs`, `process_irq_event` logged `CRCError`
through a `debug!` and fell straight through to `RxDone`. The chip raises both
together, so every corrupt packet was handed up as a valid frame — and
`debug!` compiles to nothing in a firmware with no defmt consumer, so the flag
was not merely ignored, it was unobservable.

It surfaced because this is the first harness that compares a *single* frame
byte for byte in that direction and says where it differs. The signature was
unmistakable once printed: a contiguous burst of four to nine wrong bytes near
the tail, with the round tag intact — `DAMAGED first at byte 60 (4 of 64 bytes
differ) [60:9a/5a 61:74/5a 62:18/5a 63:da/5a]`. It followed the *role*, not the
board: both V4s produced it, and the reverse direction never did.

The fix reports `CRCError` as `RadioError::PayloadCrcError`; the executive
counts it as `rx_damaged` and listens again rather than reporting a fault,
because the radio is fine and the air is not — and on a channel speaking
somebody else's binary protocol, answering with the board's `radio rx failed`
line would inject text into their stream. `air` gained `rxbad`. `HeaderError`
was deliberately left as it was after a first attempt included it: a header
that did not decode delivers no packet, so turning it into an error only makes
the caller re-arm the receiver on every burst of noise.

**The A/B, same instrument and same bench:**

| image | result |
|---|---|
| v41, before | frames delivered `DAMAGED`, tail bursts, ~1 in 20 |
| v42, after | 6 blocks, **zero** damaged deliveries, `rxok=47 rxbad=1` against 48 sent |

Forty-eight frames sent, forty-seven delivered good, one rejected and counted.
The accounting closes exactly, which is the part that makes it a receipt rather
than a number: the mechanism explains the count.

**It is not a reliability win, and saying so matters.** A damaged frame is
still a lost frame; the pass rate does not move. What changed is that a corrupt
frame no longer *masquerades* as a good one — it becomes a missing frame, which
every layer above already knows how to handle, instead of a wrong one, which
nothing above can detect.

**Regression, because this change is under every channel:** the counted modem
block on v43 held at **7 of 8**, squarely in the historical band. Its counters
are the striking part — `rxok=399 rxbad=244`. Thirty-eight percent of what the
board heard during that block failed CRC, and before this every one of those
frames went to the host as ours. Some of it is certainly the documented
neighbours on 906.875 MHz being demodulated and correctly rejected; how much is
a question the counter can now answer, and could not before.

**Left open, honestly:**

- The V4 has no channel selector and no `channel` probe, so this lands on the
  T114 only. Its image also could not be rebuilt this session: the `esp`
  toolchain on this machine has no `xtensa-esp32s3-none-elf` core installed, so
  the driver fix has not reached that board. Both V4s still run the old driver,
  which is exactly why they were usable as the *unfixed* transmitters above.
- Unsolicited device frames (channel utilisation, battery, PHY parameters) are
  not emitted. `tulle` ignores all three while driving real hardware, so a host
  that only needs the link does not depend on them; a host that wants them will
  say so.
- The sx127x path in the vendored driver has the same shape and was not
  touched. No board here uses it.
- Real RNS has not yet been pointed at the board. `tulle`'s host is pinned to
  the same capture and is the closest stand-in, but it is a stand-in. That is
  the next receipt, and the one that makes Sideband real.

**Real Reticulum drives the board, 2026-08-04.** The receipt the entry above
left open. Stock RNS, its own `RNodeInterface`, our firmware, no shim:

```
[Notice] Opening serial port COM10...
[Notice] RNodeInterface[rnode] is configured and powered up
interface: RNodeInterface[rnode] online=True
destination: <718c57fe88eea2f07bfef2570899a6a5>
```

**Counted, 8 of 8 sessions online**, with `txok` matching the announces one for
one and `refused=0 overmtu=0 dropped=0` throughout. The whole conversation was
captured through a tee, and it is the 1.86 oracle's conversation exactly:
`DETECT/FW_VERSION/PLATFORM/MCU`, the five settings each echoed verbatim,
`RADIO_STATE` echoed as 1, then the announce as a `DATA` frame.

**A compatibility datum worth recording:** the installed RNS is **1.4.0**, and
the capture that pins both halves of this protocol was taken from **1.3.8**.
The parts implemented here did not change between them. That is the first
evidence that the wire this work is pinned to is stable across an RNS minor
release rather than a snapshot of one.

**RNS also validates.** It reads back the radio parameters after configuring
and refuses the interface if they disagree, which is what makes the settings
echo load-bearing rather than a courtesy. It is also why the settings are
echoed from the *decoded* value: a decode that misread a field would be caught
here rather than becoming a silently wrong channel.

**Two things left open, and both are named rather than smoothed over.**

**One intermittent startup failure.** Early in the session one attempt aborted
with *"Spreading factor mismatch. After configuring RNodeInterface, the
reported radio parameters did not match your configuration."* It has not
recurred in the fifteen-plus sessions since, including the counted block, and
the captured conversation from a good run shows the SF echo arriving 40 ms
after the command with the right value. So it is recorded as a real
observation with no mechanism yet, not as a fixed bug. The next occurrence
should be caught with a tee running; the harness for that is
`scratchpad/rns_capture.py`'s shape, which logs raw bytes and defers all
deframing so nothing expensive runs in the serial read path.

**RNS sends one command this device does not implement, twice per session.**
The counter said so immediately; what it could not say was *which*, and a count
that cannot name itself leaves the only way forward a packet capture of
somebody else's client. So the probe gained `last=`, and the board answered on
the first run after reflashing: `unhandled=2 last=0x0a`, exactly twice per
session, eighteen across nine. The tee never saw it, which is its own small
lesson about trusting one instrument. Nothing observable depends on it: nine
sessions came online, announced, and shut down cleanly without it. It is left
unimplemented rather than guessed at, because a device that answers a command
whose semantics it inferred is worse than one that visibly does not answer.

Design point worth keeping: **counted is good, named is better.** Every counter
that exists to explain a foreign peer's behaviour should carry the identifying
byte alongside the count, or it can only ever say that something happened.

**One Reticulum network, two implementations, over real RF, 2026-08-04.** The
step past "RNS drives our hardware", and the one that matters for the park
test. Stock RNS announced through the T114 on the RNode channel; `outrider`'s
`park` example, running our own Rust stack over a direct-PHY V4, heard each
announce, validated it, and learned the peer:

```
[peer] afc3c8028bb732654bcfc5cfd2267947 appeared
[peer] 3c45e53a18d3695cd0a34b62d191141b appeared
[peer] 2d655cc0a6dcfc601a9177e6397fe701 appeared
[peer] 24623b955aa99acd22595c01c4c70a11 appeared
```

**4 of 4**, each address matching the destination RNS printed on the other
side. The path is: reference Reticulum, our RNode firmware, the air, our
direct-PHY firmware, our Rust Reticulum. Two implementations of the protocol
and two firmware personalities, one network.

It works because the RNode channel programs the on-air settings the rest of
this firmware uses, which is the same reason it does *not* cross with stock
RNode hardware. The one choice buys the fleet and costs the foreign device;
that trade is deliberate and stated where it is made.

The reverse direction, our stack announcing into RNS, is not receipted here:
`rns_live.py` announces but does not log what it hears, so proving it needs a
listener on the RNS side rather than new firmware. Cheap, and the obvious next
thing.

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
