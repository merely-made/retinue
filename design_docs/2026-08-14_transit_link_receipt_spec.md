# Spec: the transit link receipt

**Date:** 2026-08-14
**Status:** Specification for the bench session that closes AIR3's named
follow-up. No code is written by this note. It exists so the session has
something to run when the topology is available, rather than being designed
at the bench.

## The claim to be earned

The [AIR3 on-air receipt](2026-08-13_air3_t114_on_air_receipt.md) proved the
T114 relays signed announces, with independent physical observation of
type-2 hop-one traffic. It explicitly did not prove the stronger
transaction, and named it:

> a link request forwarded to a destination and its returned proof

That is the claim. Three parties, not two: a requester, a relay, and a
destination that is not the relay.

```text
desktop A ── V4 ── RF ── T114 (transit) ── RF ── V4 ── desktop B
 requester                  relay                    destination
```

## What already exists

Less is missing than it first appears.

- **`crates/retinue/examples/node_link.rs`** already drives a link over real
  RF: desktop Endpoint through a V4 direct-PHY modem to the T114's node
  channel, waiting for the announce, resolving the identity, opening the
  link, awaiting the proof. It carries N4's receipt. The seams it uses are
  the ones this work needs: `retinue::iface::tulle::drive`,
  `tulle::direct_phy_serial::DirectPhySerialLink`, `tulle::PhyProfile`,
  `tulle::airtime::AirtimeBudget`.
- **`crates/retinue/examples/link_peer.rs`** already has the role structure,
  initiator and responder from `RETINUE_ROLE`, where the responder registers
  a destination, announces it, and echoes on any inbound stream. Its own
  doc comment says "possibly through a transport node". It runs over TCP.

So the missing piece is one composition, not a harness from scratch: **a
direct-PHY responder**, which is `link_peer`'s responder role over
`node_link`'s radio seam. The requester side is `node_link` pointed at the
destination's identity rather than the board's.

`node_stress`'s `links` mode is not this. It opens links *against* the board
to exercise its own four-link table, which is a different gate.

## The part that is not code

Three radios in one room all hear each other. If the requester can reach the
destination directly, a completed link proves nothing about the relay, and
the receipt would be false without anyone lying. Isolation has to be real,
and one of:

1. **Distance.** At SF11, 250 kHz, 906.875 MHz these carry kilometres in the
   open. A field exercise, not a bench one.
2. **Attenuation.** Inline attenuators, dummy loads in place of antennas, or
   a shielded enclosure on the two endpoints. The only option that fits
   indoors, and the one to plan for.
3. **Logical filtering,** which must be labelled as such. IFAC or an
   identity allowlist can make each endpoint ignore the other. That earns a
   claim about the *routing logic under a constrained view*, and it must not
   be written up as an RF forwarding proof.

## The control is what earns the claim

Whatever isolation is used, the mechanism is not the evidence. The evidence
is a negative control taken in the same session, on the same radios, at the
same settings:

1. **Relay off.** Power down the T114. Run the requester against the
   destination for a full timeout window. It must fail to link, and the
   destination must record no inbound request. Capture both.
2. **Relay on.** Power the T114 into its node channel, unchanged otherwise.
   Run the identical requester command. The link must complete and the proof
   must return.
3. **Relay off again.** Repeat step 1 to show the failure was not a warm-up
   artifact of the first attempt.

Without step 1 the receipt says nothing, because the endpoints may simply
have been talking to each other the whole time. This is the same discipline
as proving an instrument before believing a negative, pointed the other way:
here the absence is the control and the success is the measurement.

## Acceptance

The receipt is met when, in one session with one T114 boot:

- the relay-off control fails to link, twice, bracketing the success;
- the relay-on run completes a link and returns its proof;
- the T114's own counters show the forwarded request and proof, distinct
  from its announce relaying;
- the destination independently records an inbound link request whose
  requester is desktop A;
- the isolation method is stated, and if it is logical rather than physical,
  the receipt says so in its own summary rather than in a footnote; and
- the bench restoration is recorded, including what was inferred rather than
  confirmed, as AIR3 did for the V4's rnode channel.

## Three hosts

Driving each radio from its own computer is worth doing, though it solves
orchestration rather than topology. It gives three independent serial
sessions with no USB contention, and per-node logs with independent
timestamps. It also sidesteps the failure AIR3 recorded, where the T114's
CDC endpoint stopped accepting writes after the source session closed and
the final read was lost. Q-PC, the M4 iMac, and the Fedora ThinkPad are
enough.

Radios remain on the machine that drives them; nothing here needs a shared
filesystem, only three terminals and a way to line up the logs afterwards.

## Ordering

The driver and the control procedure are needed under every topology, so
they come first and can be written and mechanically exercised with all three
radios on one bench, where the link will complete for the wrong reason. That
is fine for shaking out the code, and it must not be written up as a
receipt. The receipt waits for attenuators or a second site.
