# Retinue project prospectus

**Date:** 2026-08-26
**Status:** draft for Mark's review. Grant-agnostic: written to serve whatever
FIVCO facilitates, a future ARDC window (next: February 1, 2027), or any
funder asking what this project is. It compresses the design-doc corpus; the
corpus, not this file, is the authority on any technical claim. This is not
`PROJECT_DESCRIPTION.md` (core §7 reserves that for the maintainer; it still
does not exist).

## The project in one paragraph

Retinue is an independent open-source Rust implementation of the Reticulum
networking protocol with LXMF messaging, plus wire-level compatibility work
for the Meshtastic and MeshCore ecosystems, running both as host software and
as self-contained firmware on commodity LoRa radios (Heltec T114 and Heltec
V4). The workspace is MPL-2.0 with itemized third-party notices. Development
is receipt-driven: every capability claim is backed by a dated acceptance
document recording the bench configuration and, where the claim is about
radio, real RF evidence.

## Why it matters here

The project is developed in Appalachian Kentucky, with the 2022 flood history
close by. Off-grid, infrastructure-independent messaging is a genuine
resilience need here, not a framing exercise, and this is the only local
effort with a working line on the technology. The differentiated technical
claim, in one sentence: one inexpensive radio that can hear multiple mesh
ecosystems — Reticulum, Meshtastic, MeshCore — through a shared detection
plan with counted misses, so a county can deploy nodes without first picking
a vendor ecosystem. Alongside it sits bounded-airtime transmit policy work
shaped by the question a county actually asks: will these nodes flood the
wider mesh.

## What exists today, verifiably

- Public workspace at <https://github.com/merely-made/retinue>, with CI
  gates for licensing, an audited unsafe-token policy, sustained fuzzing of
  the RF ingest path, and a validation registry that refuses evidence from a
  dirty tree.
- `wavicle` published on crates.io, bit-exact against its reference oracle.
- LXMF messaging over real RF between the two proven boards, with published
  acceptance receipts (the AIR gate family, including on-air).
- Interoperability receipts against the reference Reticulum implementation
  (RNS 1.5.2, local-TCP scope), and against Meshtastic and MeshCore stacks,
  including cross-firmware flash and restore of foreign images.
- Owner-grade tooling: Linkboy (flashing with immutable plans and machine
  receipts), Signalman (graphical owner flow), and a receipted firmware
  catalog with an authenticated-update foundation.
- A corpus of 100+ dated acceptance documents, many as machine-readable JSON
  receipts.

## What a funded period delivers

Candidate scope, held from the ARDC lane assessment; each milestone closes
with a public dated receipt, which is how this project already works.

| Milestone | Done condition |
| --- | --- |
| Authenticated interop (R8, IFAC) | IFAC-authenticated links interoperate with reference Reticulum, receipted over RF |
| Forward secrecy (R9, ratchets) | Ratchet rotation and restore receipted against the reference implementation |
| Native node personality | A T114 running Retinue stands alone as a mesh node through reboot and field power budgets, receipted on hardware |
| Multi-protocol listening | One radio captures Meshtastic and MeshCore frames through a shared detection plan, with per-profile capture and miss counters, two transmitters on the bench |
| Propagation hardening | Outrider LXMF store-and-forward survives its adversarial suite and a physical two-node receipt |
| Documented interop matrix | A published matrix stating, per protocol pair, what is proven, at what scope, with links to the receipts — including what remains open |
| Firmware releases | Tagged releases with reproducible images for both proven boards, SHAs and receipts in the release notes |

## Measurable outcomes

The interop matrix against three ecosystems; the number of boards supported
with owner-grade flashing; public documentation for deployment; the count and
kind of RF receipts published. All outcomes are inspectable in the repository
by anyone, reviewer or neighbor.

## Team

One developer, with shipped verifiable work: the published crate, the public
workspace, the receipts corpus. The receipts culture is the differentiator: a
reviewer does not have to trust a progress report, because every claim links
to its evidence.

## Sustainability

Everything funded is openly licensed, so it outlives the grant period by
construction. The maintenance channel is the stock-hardware, user-flash
posture: commodity boards anyone can buy, firmware anyone can flash, and an
expected vendor ecosystem of pre-flashed hardware on the Meshtastic
precedent, in which the developer intends to be one participant among
several.

## Budget skeleton

Numbers are set per funder in the pre-application conversation, never
guessed. Categories, per the ARDC lane research, which generalize:

| Category | Notes |
| --- | --- |
| Personnel | Developer time; salaries are ordinarily fundable |
| Supplies and equipment | Boards, antennas, test gear; group small items, never major ones |
| Travel | Field deployment and outreach |
| Outreach | The component amateur-radio and community funders expect |
| Indirect | Per sponsor policy; ARDC caps at 20% |

## Commercial separation

Merely LLC, the developer's company, takes no grant funds, invoices nothing
against them, and receives no product-specific spending. All funded outputs
are openly licensed and available to everyone, competitors included. The
LLC's eventual position — convenience, testing, and support on pre-flashed
stock hardware — is the standard open-hardware vendor model with direct
funding precedent (ARDC funded Meshtastic while vendors sold boards running
it). Fiscal sponsorship sponsors the project, never the vendor.

## Letters and partnerships

FIVCO ADD is engaged directly and is actively working to facilitate grant
funding for this work. Depending on the application: a regional amateur-radio
club partnership (outreach and letter of support), county emergency
management (where the resilience framing is used), and FIVCO itself in
whichever role — sponsor, facilitator, or supporter — fits the instrument.
