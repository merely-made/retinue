# AIR3 bounded transport software receipt

**Date:** 2026-08-12  
**Scope:** Retinue's native-node transport model and the T114 image that hosts it.

AIR3 found an important split in the existing code. The desktop `Endpoint` already
had a TTL-bound, capacity-bound path table. The no-std `retinue::node::Node` had
bounded peers, links, and actions, but no paths or transit. Repeating the host
route tests would not satisfy FT2 or FS6.

This change gives the firmware model its own bounded transport state. It does
not claim an on-air T114 result from a desk test.

## Implemented model

`Node` now has an explicit `TransportConfig`. Its default is `none()`, preserving
ordinary endpoint-node behaviour. The T114's `channel node` personality selects
`transit()`; modem and RNode personalities remain host-driven.

The T114 profile is:

| State | Capacity | Reclamation |
| --- | ---: | --- |
| Address book | 32 peers | existing refusal rule |
| Routes | 16 | 30-minute TTL, then oldest live route evicted |
| Carried-link bridges | 16 | one-hour idle TTL, then oldest live bridge evicted |
| Recent transit hashes | 16 | 60-second TTL, then oldest hash evicted |
| Actions per call | 8 | counted overflow |

Only a verified announce admitted to the address book learns a route. A route
retains its interface, hop count, and next transport identity. A shorter route
wins; a refresh only keeps the selected path fresh when it arrives through that
same interface. A full table evicts the oldest live route only after expired
entries have been removed.

The transport profile re-broadcasts a verified announce as header type 2 with
this node's identity as its transport hop. It carries header-type-2 traffic
addressed to that identity toward the learned route, and remembers a bounded
bridge for the request's later proof and link data. All forwarded traffic is
deduplicated on a shared radio and stops at 128 hops. `node` now reports the
route count, transit state, forwarding, route expiry/eviction, hop drops, and
unroutable packets.

## Peak-memory instrument

The prior `heap` probe reported only live allocation. A quiet probe after a
flood could therefore report a small number even when a packet buffer had
already reached a larger peak.

The T114's fixed 48 KiB LLFF allocator is now wrapped with an atomic high-water
counter updated after each successful allocation. `heap` reports:

```text
heap=<live>/49152 highwater=<peak-since-boot> free=<available>
```

The boot node line includes the same peak field. This is real firmware
instrumentation, but it is not yet a measured value from a flashed board.

## Desk receipts

`cargo test -p retinue node::tests --lib` passed 28 tests, including three new
transport receipts:

1. A two-route table evicts the quietest route, then expires both remaining
   routes after its TTL.
2. A relay re-broadcasts a verified announce, forwards a header-type-2 link
   request to its destination, and carries the returned proof through its
   remembered bridge to complete the source link.
3. Thirty-two distinct signed announces against a four-route test profile keep
   route residency at four and record 28 evictions while producing at most one
   learn and one relay action per input.

The full `cargo test -p retinue` suite passed 162 tests. The no-std core check,
`cargo check -p retinue --no-default-features --target thumbv7em-none-eabihf`,
`cargo check -p radio-hand`, and
`cargo check -p retinue --example node_stress --features tulle-radio` also
passed. The exact T114 image cross-compiled and linked in its production profile:

```text
cargo build -p tulle-t114-phy --release --target thumbv7em-none-eabihf --locked
```

`llvm-size` reports 274,862 bytes of text, 368 bytes of data, and 80,012 bytes
of BSS. That is 275,230 bytes of the 802,816-byte application flash region and
80,380 bytes of the T114's 237,568-byte application RAM region. No DFU package
was produced or flashed for this software receipt.

## On-metal receipt still required

No board was flashed or probed for this change. FT2/FS6 remains open until the
new image is run on the T114.

The existing `node_stress` flood generator now accepts a starting identity
number, so consecutive waves are distinct:

```text
node_stress COMx flood 0
node_stress COMx flood 40
node_stress COMx flood 80
```

Run that against a T114 booted in `channel node`, keep an independent receiver
on the same profile, then record all three facts from one boot:

1. `node` shows `transport=1`, a route table at or below 16, nonzero
   `fwdannounce`, and a nonzero `routeevicted` after the first forty distinct
   announces.
2. The independent receiver sees header-type-2 relays carrying the T114's
   transport identity and hop count one. A link request and returned proof
   provide the stronger relay transaction.
3. `heap` after each wave shows a stable `highwater` under 49,152 bytes while
   ordinary traffic still crosses the node.

That is the required hardware receipt. This document closes AIR3's model,
instrumentation, and cross-compilation work, not its RF measurement gate.
