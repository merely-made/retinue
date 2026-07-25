# Outrider: LXMF as a boundary crate

**Date:** 2026-07-25
**Status:** founded. Name decided and verified free on crates.io (2026-07-25);
no code exists. This records scope, provenance, and gates so the crate starts
with the same discipline sennet did.
**Siblings:** `2026-07-20_mesh_household_tulle_tucket_sennet.md` (household,
layering, licensing ruling), mere's
`2026-07-06_lxmf_key_addressed_mail_research.md` (option C and the 2026-07-24
addendum this crate executes), mere's
`2026-07-24_low_power_managed_network_plan.md` (V9/V10, the offered-service
tie-in).

## Name logic

An outrider rides ahead of or beside the retinue to scout the road and carry
word. The crate carries messages for the household without being the
household's own voice. It joins tulle (the fabric), tucket (one arrival),
sennet (the procession) in the court register.

## What it is

A Rust implementation of LXMF, the message format and delivery system of the
Reticulum ecosystem, as a **boundary crate**: a codec, delivery state
machines, and a propagation client/server, riding entirely on retinue's
destinations, links, and resources. Consumed by merecat/mere at the edge, the
same way tucket and sennet are.

The decided posture (mere research doc, option C, reaffirmed 2026-07-24) is
load-bearing: LXMF is an interop adapter and an offered service, never the
internal message semantics. The internal spine is the shared engram. If
LXMF's message-shaped model starts leaking inward, the abstraction is wrong,
the same discipline sennet holds against Meshtastic framing.

## Why it earns its place

1. **Cold-start interop with the home ecosystem.** Sideband, MeshChat, and
   NomadNet users are the likeliest first buyers of a Merely radio. A fresh
   unit that can message them on day one answers the hardware cold-start
   problem.
2. **Propagation node as the first offered service.** An LXMF propagation
   store is store-and-forward with expiration, which is V10's shape, and
   "I carry mail" is a V9 offer. Merely nodes become wanted infrastructure on
   the existing network before asking anything of it.
3. **A whole-stack conformance oracle.** An exchange with a pinned stock
   client exercises announces with app_data conventions, opportunistic
   packets, link delivery, and resource-backed large messages end to end,
   judged by third-party software.
4. **The first real consumer of R8 and R9.** Opportunistic LXMF delivery
   encrypts single packets to the destination's current ratchet, making R9
   load-bearing. Community networks often run IFAC-protected interfaces,
   doing the same for R8. The remaining spec-parity debts get retired for a
   consumer, not for a checkbox.
5. **Product coherence.** Tucket, sennet, and outrider under the same
   merecat trait: one contact surface that reaches MeshCore text, Meshtastic
   text, and Reticulum mail. LXMF addressing (destination hash plus announced
   display name) drops onto the gazetteer resolution model.

## v1 scope

In:

- the message codec: framing, signature, content-derived ids, the transient
  id, timestamp/title/content/fields, with unrecognized fields carried
  opaque and round-tripped intact;
- direct delivery over a retinue link, and resource-backed large messages;
- opportunistic single-packet delivery (gated on R9 landing in retinue);
- propagation client: hand a message to a propagation node, fetch waiting
  mail for an identity;
- a bounded propagation server: accept, store, expire, deliver to owner,
  with explicit capacity limits and stamp **verification**;
- announce app_data conventions (display name, stamp cost).

Out, until demand is real:

- inter-node propagation sync parity (the moving, deepest half);
- stamp generation beyond what sending requires; ticket ecosystems;
- the fields ecosystem (telemetry, images, voice): chosen fields only, from
  captures and public prose, per the sennet recipe;
- paper messages;
- any conversation/contact semantics. Those belong to the consumer.

## Provenance discipline

The sennet playbook verbatim, recorded in a `PROVENANCE.md` that keeps pace
with the code from the first commit:

- the Python LXMF implementation and Sideband are **never read** (post-2025
  license family, and the black-box posture keeps behavior questions honest);
- implementation facts come from the public LXMF README/spec prose, the
  Reticulum manual, and black-box captures against pinned stock clients;
- captures are committed as fixtures so CI needs no Python;
- release notes pin the stock baseline being matched.

## Sequencing and gates

Outrider sits in the desk lane after the bench session, beside or after R9
(its opportunistic mode depends on it; direct and propagation delivery do
not). Ordered gates, capture first as always:

1. **Oracle:** pin a stock LXMF/Sideband baseline; capture message bytes,
   announce conventions, and a propagation session; commit fixtures.
2. **Codec:** parse and rebuild captured messages byte-exactly, opaque
   fields included.
3. **Direct delivery:** a message from outrider to a pinned stock client
   over a live link, and the reverse, both verified in the stock UI and
   byte-exactly on the wire.
4. **Propagation client:** submit to and fetch from a stock propagation
   node.
5. **Propagation server:** a stock client submits to and fetches from an
   outrider node; capacity and expiry behavior deterministic under test.
6. **Opportunistic delivery** once R9 lands, against the same oracle.
7. **Radio receipt:** gate 3 repeated over a real RF link through the tulle
   stack, which is the receipt that matters for the product story.

## Licensing

MPL-2.0, per the household ruling of 2026-07-23. Exhibit A only, never
Exhibit B. `deny.toml` posture unchanged.

## Next click

Reserve the crates.io name with a 0.0.1 stub stating scope, as retinue,
tulle, sennet, and tucket each did at founding.
