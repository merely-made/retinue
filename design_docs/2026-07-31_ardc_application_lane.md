# ARDC grant application lane

**Date:** 2026-07-31
**Status:** lane opened. Next window closes **September 1, 2026**; applications
after that wait for February 1, 2027. Review runs 60-120 days after the due
date, so a decision lands roughly November-December.
**Facts below verified against ardc.net and its application instructions on
2026-07-31.** Submission portal: <https://grants.ardc.net>.

## Fit

ARDC funds open R&D in amateur radio and digital communications, aiming to
grant about $3.8M in 2026 and funding roughly 30% of proposals. Retinue is
squarely in scope: an independent open implementation of Reticulum plus
Meshtastic and MeshCore interop, RF-proven on real boards, everything openly
licensed. Their evaluation criteria also favor geography we can claim honestly:
"areas where few other grants have been made" and low-income areas. Appalachian
Kentucky, with the 2022 flood history, is a genuine off-grid-resilience story,
not a stretch.

## Eligibility and the sponsor requirement

- Eligible applicants: US 501(c)(3) public charities, government agencies,
  schools, universities; international equivalents.
- **For-profit businesses are not eligible.** Merely LLC cannot be the
  applicant. Mark applies as an individual through a fiscal sponsor.
- **Eligible sponsor classes, per ARDC's own instructions: 501(c)(3)s, local
  government organizations, universities, or schools.** ARDC suggests regional
  radio clubs as the practical route.

### Sponsor candidates, ranked

1. **FIVCO ADD.** An Area Development District is a local government
   organization, which is an explicitly eligible sponsor class. Administering
   grants as fiscal agent is an ADD's ordinary trade, and the regional
   disaster-resilience angle aligns with their mission. Unknowns: willingness,
   and whether their approval cycle fits the window. One phone call answers
   both.
2. **A regional 501(c)(3) ham radio club.** ARDC's own suggestion. Even if not
   the sponsor, a club partnership supplies the letter of support and the
   outreach component the criteria ask for. Pursue in parallel with FIVCO, not
   after.
3. **HCB (Hack Club, hackclub.com/hcb).** National 501(c)(3) fiscal sponsor
   for open projects, onboarding measured in days, roughly 7% fee. The
   fallback if the local routes stall.

Ruled out: Open Source Collective (501(c)(6), fails ARDC's sponsor classes),
Open Collective Foundation (dissolved 2024), Software Freedom Conservancy and
SPI (intake far slower than this window), and mission-distant local nonprofits.
Sponsor mission alignment is part of what reviewers read; a sponsor whose remit
plausibly covers communications, resilience, or STEM strengthens the
application rather than merely enabling it.

## What the application contains

Form at grants.ardc.net, or PDF upload; skip irrelevant questions. Reviewers
say lack of project-plan detail is the most common rejection reason, and also
ask that applications stay brief. The design-doc corpus is the raw material;
the application is a compression of it.

- **Project description.** What retinue is, what the grant period delivers.
  Candidate scope: R8 IFAC completion, R9 ratchets, retinue-small native node
  (N-gates), outrider LXMF propagation hardening, documented interop matrix,
  firmware for the two proven boards.
- **Roadmap with milestones and risk assessment** (required for R&D grants).
  The existing gates translate directly; the receipts culture is a
  differentiator worth showing (acceptance docs with bench configs and RF
  evidence).
- **Team.** Solo developer with shipped, verifiable work: wavicle on
  crates.io (bit-exact against the reference oracle), the retinue workspace,
  the acceptance receipts.
- **Measurable outcomes.** Interop matrix versus reference Reticulum,
  Meshtastic, MeshCore; boards supported; public documentation.
- **Sustainability.** Community adoption plus the stock-hardware,
  user-flash channel keeping firmware maintained past the grant period.
- **Outreach component.** Amateur-radio projects are expected to introduce
  new people; the club partnership carries this.
- **Letters of support.** Club, county emergency management if the resilience
  framing is used, FIVCO if not the sponsor.
- **Budget.** Spreadsheet preferred. Categories: personnel (salaries are
  fundable), supplies and equipment (boards, antennas, test gear), travel,
  marketing, indirect up to 20%. Group small items; never group major ones.
  No lobbying. Size to the roadmap, and calibrate the ask in the pre-app
  conversation rather than guessing.

## Licensing check

Funded work must be freely available; ARDC's example license list is GPL, MIT,
BSD, CERN OHL, and CC variants. The firmware's GPLv3 is squarely on the list.
The workspace's MPL-2.0 is not named in the examples; the list reads as
illustrative, but confirm MPL acceptability in the pre-app contact rather than
discovering the answer in review.

## Commercial separation

Merely LLC's planned radio business sits adjacent to this application. Current
facts, which make the separation easy: the LLC has sold nothing to date, holds
no inventory, and the only hardware is Mark's three personal test radios. The
clean structure has three parts:

1. **Money flow.** The grant pays Mark as an individual through the fiscal
   sponsor. The LLC takes no grant funds and invoices nothing.
2. **Outputs.** Everything grant-funded is openly licensed, so the funded
   artifact is available to everyone, competitors included. The LLC's eventual
   edge is convenience, testing, and support on pre-flashed stock hardware,
   the standard open-hardware vendor position (Meshtastic's vendor ring,
   RNode's commercial boards). ARDC funded Meshtastic while vendors sold
   boards running it; the model has direct precedent.
3. **No product-specific spending.** No grant dollars on inventory,
   certification, branding, or anything that exists only for the LLC. Dev
   boards and test gear for the open work are fine; equipment title follows
   the sponsor's policy (ask when signing).

Framing: fiscal sponsorship sponsors the *project*, never the vendor. The
phrase "sponsored vendor" stays out of every conversation. In the
sustainability section, the vendor ecosystem is stated proudly as the
future-funding plan, with Mark intending to be one participant in it.
Optional extra-clean posture: no LLC sales during the grant period. Not
structurally required (Meshtastic's vendors sold throughout), so volunteering
it is a choice, not an obligation.

## Gates

- **G0 — pre-app contact sent** to giving@ardc.net (or +1-858-477-9900).
  Draft below. Done when their read on viability, MPL, and the ADD-as-sponsor
  question is in hand.
- **G1 — sponsor committed in writing**, with the agreement and the sponsor
  details the form needs. FIVCO call and club contact in parallel; HCB
  application filed as the fallback clock.
- **G2 — public developer release on GitHub.** Workspace tag, firmware images
  for the T114 and V4, RF receipts in the release notes, honest interop matrix
  including the open R8/R9 items. This is the evidence base reviewers can
  check.
- **G3 — proposal and budget drafted, letters requested.**
- **G4 — submitted at grants.ardc.net before September 1.**

## Pre-app email draft (G0)

> Subject: Pre-application question: open-source Reticulum-compatible LoRa
> mesh stack, Appalachian Kentucky
>
> Hello — before applying in the September 1 window I'd like a read on fit and
> three structural questions.
>
> The project: Retinue, an independent open-source Rust implementation of the
> Reticulum protocol with LXMF messaging, plus wire-level interop with
> Meshtastic and MeshCore, running on commodity LoRa boards (Heltec T114 and
> V4) with our own embedded firmware. Links are proven over real RF with
> published acceptance receipts. The grant would fund completing the stack
> (authenticated interop, forward-secrecy ratchets, a self-contained node
> personality on the board) and documenting it for community use, from
> eastern Kentucky.
>
> Questions: (1) I'm an individual developer, not a nonprofit; our regional
> Area Development District (a Kentucky local-government planning body that
> routinely administers grants) may act as fiscal sponsor — does that satisfy
> the local-government-organization sponsor class? (2) Most of the workspace
> is MPL-2.0 with GPLv3 firmware; is MPL-2.0 acceptable under the open-access
> requirement? (3) I own an LLC that has made no sales and holds no
> inventory; its eventual plan is selling stock hardware pre-flashed with
> this open firmware, the same downstream use the licenses give everyone, on
> the model of Meshtastic's vendor ecosystem. The LLC would take no grant
> funds, and all grant outputs would be openly licensed. Any structural
> concerns we should address in the application?
>
> Happy to share the repository and receipts. Thank you.
