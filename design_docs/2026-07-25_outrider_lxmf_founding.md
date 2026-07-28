# Outrider: LXMF as a boundary crate

**Date:** 2026-07-25
**Status:** gates 1–8 complete locally against pinned LXMF 0.9.6 /
RNS 1.4.2 black-box oracles. Direct and Resource-backed propagation responses
work in both directions, and direct delivery has been repeated over product
RF. Propagation state survives host-controlled process restarts.
Opportunistic delivery passes in both stock directions over ratcheted
single packets; its reduced plaintext grammar is fixture-pinned, and compact
cost-8 messages pass in both directions over product RF.
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

1. **Oracle:** complete for the message object, delivery and propagation
   announces, propagation submit, and the two-request fetch session against
   LXMF 0.9.6 / RNS 1.4.2. Fixed fixtures replay without Python.
2. **Codec:** complete for the captured message object. Decode, encode,
   message id, signature preimage, opaque MessagePack fields, bounds, and
   malformed-input refusal have executable receipts.
3. **Direct delivery:** complete in both directions over live links, judged by
   stock delivery callbacks. Cost-8 stamps are generated and enforced; small
   messages use one data packet and 4 KiB messages use a Resource.
4. **Propagation client:** complete for stamped submit to and two-stage fetch
   from a stock propagation node, including decryption and source-signature
   verification.
5. **Propagation server:** complete for the captured one-message lane. A stock
   client submits to and fetches from Outrider; stamp verification, duplicate
   suppression, owner scoping, acknowledgement, capacity, and expiry are
   deterministic. The default in-memory store caps fetched encrypted messages
   at 240 bytes, while caller-selected larger limits degrade the response to a
   request-bound Resource. A 4 KiB message passed against stock clients in
   both directions. Receipt:
   `2026-07-28_outrider_large_propagation_response.md`.
6. **Opportunistic delivery:** complete. Stock omits the 16-byte LXMF
   destination from the encrypted single-packet plaintext because Reticulum's
   header already carries it; Outrider prepends it and reuses the ordinary
   signed codec. Fixed-fixture replay, cost-8 in-process enforcement, a
   transport-node hop, and live un-stamped stock delivery pass in both
   directions. Receipt: `2026-07-28_outrider_opportunistic_delivery.md`.
7. **Radio receipt:** complete. Gate 3 passed in both directions through the
   Tulle direct-PHY stack between a Heltec V4 and T114: cost-8 stamped Data
   and 4 KiB Resource messages arrived byte-exact and authenticated. Receipt:
   `2026-07-28_outrider_direct_phy_delivery.md`. Opportunistic cost-8 messages
   also passed in both directions as one 243-byte ratcheted RF packet. Receipt:
   `2026-07-28_outrider_direct_phy_opportunistic.md`.
8. **Host persistence:** complete. `PropagationStore` encodes versioned state;
   restore re-derives transient ids and byte counts and reapplies current
   capacity, expiry, duplicate, and owner-scoping rules. A filesystem host
   survived two real process starts against a stock client, while the
   in-process restart receipt also persisted owner-scoped acknowledgement.
   Receipt: `2026-07-28_outrider_propagation_persistence.md`.

## Licensing

MPL-2.0, per the household ruling of 2026-07-23. Exhibit A only, never
Exhibit B. `deny.toml` posture unchanged.

## Next click

Node peering remains deferred. The open compatibility item is a live
cost-bearing stock opportunistic send: cost-8 works in Outrider's executable
test, but the stock oracle did not emit that variant during its bounded run.
Per-interface frame-limit admission is complete: a headed run refused a
291-byte opportunistic packet before the 255-byte direct-PHY queue, kept both
drivers alive, then delivered fitting packets in both directions.
