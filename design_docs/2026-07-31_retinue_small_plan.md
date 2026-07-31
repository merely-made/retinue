# retinue-small plan

**Status:** N0 proven on the T114 across reset and application reflash, power-loss
leg open; N1 complete, the sans-io core cross-compiles for the board; N2 next
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

## Two structural decisions

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

## Gates

Each gate carries linker receipts: flash, static RAM, heap high-water mark, and
maximum task/future size. The T114 is chosen precisely because 256 KB forces
honest limits, so a gate that lands without those numbers has not landed.

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

Extract the shared radio service from both `main.rs` files into `radio-hand`.
Both images rebuild on it, with `board.rs` as the only board-specific seam.

**Done:** both images build on `radio-hand`; the direct-PHY RF receipts of
[2026-07-23](2026-07-23_direct_phy_resource_acceptance.md) still pass
byte-exact; the two `main.rs` files shrink to board wiring plus shell.

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
both directions over real RF.

### N5 — Reliable data and survival

Exchange reliable data both ways. Survive loss, reordering, and reboot. Re-prove
the N1 capacity errors under live traffic.

**Done:** byte-exact payload both directions over RF; recovery after induced
loss and after a mid-transfer reboot; a full table under live traffic rejects
with a typed error and the node stays operational. Bounded outcomes for the
heltec doc's adversarial set: fuzzed frames, a full route table, a full queue,
entropy failure, flash corruption, and resource cancellation. The T114 receipt
adds idle, receive, and transmit current.

### N6 — Panels from local state

Identity, Links, Peers, and Traffic panels drive from board-local Retinue state
instead of host snapshots. Status, Power, and Radio already show firmware truth;
this makes the other four genuine rather than projected.

**Done:** the four panels read local node state with the host disconnected; RF
forwarding continues across host disconnect and reconnect.

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

Next is N2: move the radio service out of the two firmware `main.rs` files into
`radio-hand`, which N0 already founded.

Harness note: the direct-PHY harness is `crates/retinue/examples/direct_phy_*`
behind the `tulle-radio` feature, alongside the `oracle/` drivers. There is no
`Code/testing/retinue/`, which is where the family convention would put it.
