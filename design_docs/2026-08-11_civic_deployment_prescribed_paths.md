# Civic Deployment: Prescribed Paths, Policy Scopes, and the Emergency Lanes

Design doc, 2026-08-11. Provenance: Mark's post-meeting design session (the
meeting surfaced the emergency-response angle) plus the same-day chat
doctrine. Rulings below marked *(Mark, 2026-08-11)* are his calls from that
session; the D-numbered items were ruled by Mark on 2026-08-12 (see Rulings). Composes with the
[mesh scaling doc](2026-08-09_mesh_scaling_and_asymmetric_routing.md) (FT),
[field node security posture](2026-08-09_field_node_security_posture.md) (FS),
[listener executive](2026-08-10_listener_executive_and_protocol_leases.md)
(LE), [stamp cost doc](2026-08-07_stamp_cost_and_roles.md), and the
[BLE scoping brief](2026-08-11_bluetooth_capability_scoping.md) (LB).

## Doctrine

**Emergencies re-price the refusal law; they never suspend it.** Precedence
is a ledger class in the FT1 accountant with a guaranteed but bounded slice.
Unlimited priority inflates to meaninglessness during a real disaster and
congestion-collapses the mesh exactly when it matters. The refusal law binds
tighter under load, not looser.

**Three flows, three trust shapes.** Distress up (unknown sender, priced and
corroborated), alerts down (enrolled authority, signed), responder
coordination (pre-provisioned personae rosters). Mechanisms do not transfer
between flows.

**Hybrid, never private** *(Mark, 2026-08-11)*: a civic deployment that
accepts no public traffic is ruled out. The county shapes its network with
grants, stamps, and keys; the public benefits without compromising the civic
lanes. Prioritization, not exclusion.

**Consent for carriage** *(Mark, 2026-08-11, mechanism open as D1)*: an
individual's node is never conscripted into carrying public traffic.

**The community rule** *(Mark, 2026-08-11)*: every policy in this doc is a
setting a node owner may set directly **or defer to a scope**: their island,
their community, their region. "Go with the flow" is a first-class choice,
and the deployment's defaults are whatever the scope publishes. This is
[[configurability-over-opinionated-defaults]] made into governance: the
county's shaping power IS a published scope policy that its members chose to
defer to. Scope policies are signed data (grants-as-data), so deferral is
verifiable and revocable.

**Scopes are miscible** *(Mark, 2026-08-12)*: deference is a **set of
memberships, not a pointer up a ladder**. One person belongs to the county,
a trail-stewards community, and a radio club at once, and the scopes
overlap rather than nest. Composition is per policy domain:

- **Duties** (carriage, pinning, corroboration defaults): union across
  memberships, each drawing on the owner's capacity, all bounded by the
  owner's own ceilings. Serving two communities costs two duties' worth of
  one node's budget, never more than the owner allows.
- **Divisible resources** (dwell slots, forwarding budget): fractional
  allocation across scopes. The executive's scan plan is natively divisible,
  so "mixing" scopes is literally mixing dwell fractions.
- **Exclusive knobs** (one radio's power, one dwell plan's shape): scopes
  may only request; the owner's ordered preference list resolves conflicts.

One distinction keeps this clean: an **island is a measured fact** (the
airtime shadow of scaling doc section 3), a **scope is a chosen
membership**. A node has one island and many scopes, and per the shared
cross-cutting law, emergence decides where boundaries are while declaration
decides what they are called; neither may define the other. RF reach must
never confer scope membership, and scope membership must never assert RF
reach.

**The commons is a sensor.** Corroboration-as-amplification generalizes past
distress: independently corroborated warning events (ice, fire, quake,
hurricane, shooting) become public event announcements. Emergency management
gains a sensing layer made of ordinary participants, weighted by independent
personae count, never by any single node's say-so.

**Corroboration is cross-cutting, not a class** *(Mark, 2026-08-12)*: to
corroborate is to use your reach to extend the message. It is one act with
three inseparable parts: countersign (vouch), dedicate your own airtime and
forwarding budget (carry), and pin the content so your node becomes an
access point for it (serve). Seeding is the right picture. Any message may
be corroborated, including an authenticated authority alert; corroboration
then extends its **reach and persistence, never its validity**, which flows
from the authority signature alone. Because a corroborator spends from their
own FT1 ledger and their own pin budget, amplification is always paid for by
the amplifiers: the refusal law holds, no free reach exists, and a
corroboration ring drains only its own slice, never the county's. This is
the pinning layer of mere's cost-metered refusal wearing a signature;
whether the corroboration envelope unifies with mere's moot petition and pin
shapes is an open cross-repo question, same posture as the FS2/petition
note. Consequence for warning events: they need no reserved precedence
class. A warning is an event announcement whose reach is purely
corroboration-funded, growing exactly as far as the commons is willing to
carry it, while the reserved emergency slice stays with distress and
authority alerts.

## Scopes are moots (Mark, ratified 2026-08-12)

Mark's modeling, checked against mere's
[boundary, identity, and grant composition](../../mere/design_docs/moothold_docs/research/2026-08-09_boundary_identity_and_grant_composition.md)
and ratified: a scope is a moot at the governance layer, and the
retinue-side object is only the moot's **published policy artifact**, a
signed record a board can verify cold. The split respects the FS ceiling
("smart enough to verify, too poor to authorize"): governance (petitions,
grants, succession, revocation) happens host-side in mere; boards hold the
artifact and its roster, and never participate in governing. Scope
membership belongs to the owner's persona in signalman, never to the relay
keypair on the pole (FS rule 1).

What the modeling buys, clause by clause:

- **Miscibility is native.** A persona is a denizen of many moots; overlap
  needs no new mechanism. Two nodes honoring different scope-policy
  revisions during partition is the moot doc's "honored-here-now" relation,
  already priced there; its open partition merge rule becomes a named
  dependency of CV4.
- **D3 deepens.** Authority enrollment stops being a config-key edit and
  becomes a grant under the scope-moot's constitution, attributed and
  revertible. The scope-governor custody question dissolves into moot
  governance; what remains is the unvotable floor (which key founded the
  moot, how the constitution is located), which the mere doc already
  requires to be a paragraph a person can read.
- **Consent restated structurally.** Suzerainty: containment cannot imply
  authority. The county moot placing itself above a community moot gains no
  power over member nodes; carriage stays a granted duty, never an absorbed
  one.
- **Corroboration weighting gets an economics.** The cast model (pseudonyms
  spend, personas earn) is the sybil answer CV7 was missing: corroboration
  weight rides persona reputation and tessera stake, so ten pseudonyms cost
  ten times as much and amplify nothing. A scope's masking policy (a
  governed moot setting) decides what identity resolution corroboration
  requires in that scope.

The mere-side counterpart is
[radio scopes are moots](../../mere/design_docs/moothold_docs/research/2026-08-12_radio_scopes_as_moots.md),
which owns the governance half: the policy artifact shape, the partition
merge rule, and the corroboration stake economics.

## Prescribed paths

The load-bearing new concept, donor-inspired by MeshCore's explicit routes
(`tucket/src/path.rs`: flood-discover once, then authenticated ordered relay
lists exchanged reciprocally and ridden directly).

A **path prescription** is a signed policy object naming an ordered relay
set (or a constraining node roster) for a class of traffic:

- **Stability by construction.** New nodes joining the mesh never disturb a
  prescription; prescribed traffic ignores them. This is the property the
  county needs: their alert tree behaves identically the day one node joins
  and the day a thousand do.
- **Automatic path diffing.** The mesh keeps measuring candidate paths with
  the FT3/FT4 machinery (recently-heard sensing, ETX-style delivery ratio,
  airtime cost). When a candidate beats the prescription on the owner's
  chosen metric, that is a **diff surfaced to the path's owner**, who
  promotes it or declines. Whether promotion may ever be automatic is D2.
- **Selectable pathing.** A prescription may constrain by roster ("only
  county-owned nodes," keyed to enrolled relay keys per FS rule 1) rather
  than by explicit hop list. Roster-constrained is looser and survives node
  swaps; hop-listed is tighter and fully deterministic. Both are the same
  object with different constraint kinds.
- **The asymmetry caveat is structural.** The scaling doc names
  flood-then-source-route as the shape asymmetry breaks worst. Therefore a
  prescription is never validated once and trusted; it carries continuous
  bidirectional health from FT3, and a failing prescribed hop is loud
  (diagnostics doctrine), with failover order itself part of the
  prescription (next prescription tier, then open routing, per policy).
- **Cross-protocol by the executive.** Under LE, a prescription names nodes
  and participation, not one protocol. A hop that is reachable over a
  foreign bearer (a Meshtastic-side relay heard by the flock) is still a
  hop; border-gateway refusal (murmuration rule 5) applies at re-injection.

## The emergency lanes as prescriptions

- **Distress up: the escalation path.** Trailhead and backcountry relays
  carry a prescribed escalation route to staffed stations. Distress class
  rides it with preemption rights inside its bounded slice. Sender is
  unknown, so admission is priced (stamps) and amplification is
  corroborated, never granted.
- **Alerts down: the authority tree.** One municipal node, provably owned
  (key enrolled in the regional roster by ceremony, D3), fed by a prescribed
  coordination path, distributing over a prescribed tree. Alert payloads are
  CAP-shaped (OASIS Common Alerting Protocol: area, severity, expiry,
  instruction), which supplies expiry and area semantics and keeps the
  format standard rather than invented. Replay defense per FS2/FS3
  (counter-window, not wall clock).
- **Responder coordination: the station lattice.** Prescribed chat paths
  between emergency stations; responders connect to stations. Personae
  rosters, scoped and private. Deliberately boring.
- **Phasing** *(Mark, 2026-08-11)*: this whole layer is deployment phase 2.
  The pilot ships the open mesh and the measurement machinery (FT ladder);
  prescriptions are drawn from measured paths, not imagined ones, matching
  the scaling doc's "ship static, measure, then encode" sequencing.

## Stamp-manifest triage

Extends the stamp doc. A forwarder's admission policy under load is a
**stamp manifest**: per-precedence-class thresholds for accept, defer
(store-and-forward via outrider), or refuse.

- **Work-conserving:** when the feed is free, sub-threshold traffic flows;
  thresholds bind only under contention. Nobody's message is refused by an
  idle network.
- **The grant lane:** enrolled identities (responders, the alert authority)
  ride on grants, not stamps, inside the reserved bounded slice. No busy
  signal for responders; no free ride for a forged flag.
- **Verification economics stand:** per the stamp doc, boards verify
  deferred, budgeted, and sheddable; rate enforcement lives at host-tier
  gateways. The manifest is policy data (scope-inheritable like everything
  else).

## Carriage roles (D1, open)

The instinct: tie public-carriage duty to declared role (router, hotspot,
transport node) rather than burdening every leaf. Recommendation to ratify:
carriage is a **declared role whose default comes from the deferred scope
policy, always owner-overridable**. A leaf that defers to the region carries
what the region's policy says leaves carry (typically nothing); declaring
router-hood accepts the region's router duty. The participant gate is the
enforcement point.

## Remote shaping (signalman is the home)

FS2 signed command envelopes are already the mechanism; this extends the
command grammar, never the trust model:

- Radio envelope commands: TX power, PHY profile, dwell plans, participation
  levels. "Remotely tuning the radios to shape network coverage" is exactly
  a dwell-plan and power grant sent to enrolled relays.
- **Bounded by the regulatory floor in radio-hand**: no envelope may command
  past region-table limits; the floor is not signable-around.
- The county shapes only nodes that enrolled its config key (public keys
  down, FS rule 2). A public node that never enrolled the county's key is
  untouchable by it, which is the consent rule again at the command layer.
- Remote firmware stays gated by FS4 (no OTA before the key split); remote
  config is the warm-key tier and ships first.

## The atlas scene (signalman)

The situational surface an EOC actually needs, and the fourth cut of the
dataset the murmuration doc already named (geographic map, topology,
cross-transport reachability, now over a basemap):

- Node positions, last-heard, coverage shadows, island boundaries (airtime
  shadows per scaling doc section 3), prescription health, alert-tree state.
- **Offline-first is an emergency requirement**: basemap from PMTiles
  single-file archives ([pmtiles-rs](https://github.com/stadiamaps/pmtiles-rs)),
  rendering candidates led by [galileo](https://github.com/galileo-map/galileo)
  (Rust GIS engine, wgpu-based, raster + vector MVT, same graphics substrate
  as genet). Integration shape to evaluate: galileo as a chisel custom-paint
  leaf on the cambium canvas versus consuming `galileo-mvt` parsing under a
  genet-native layer. D4; needs a spike receipt, not a doc ruling.
- Later overlay, named now: RF coverage modeling (terrain viewshed,
  ITM/Longley-Rice class) for siting and shaping; feeds the remote-shaping
  loop. Research lane, not scoped here.

## Personal hotspotting

The consumer wedge that seeds the very mesh the county rides. A pocket board
paired to a phone (BLE now per the LB ladder, V4 Wi-Fi SoftAP later, the
donor's literal Hopspot shape) makes every hiker a distress-capable
endpoint on the trail lanes, no infrastructure required. The same facet
model covers it: hotspot is a facet class plus a carriage role, not a fork.

## Proposed targets

CV numbering, clear of N/CM/FT/FS/H/LE/LB. Pre-decision; each gates on the
FT machinery it consumes.

**CV1: Path prescription object.** Signed prescription (hop-listed and
roster-constrained kinds), forwarder honoring, failover order; diff engine
on FT3/FT4 metrics surfacing candidate improvements to the owner.
*Validation:* on a three-node bench, adding a fourth node changes nothing on
the prescribed flow; degrading a prescribed hop fails over in the prescribed
order, loudly; a better candidate produces a diff and no behavior change.

**CV2: Precedence classes and the stamp manifest.** Ledger classes in FT1
with a guaranteed bounded emergency slice; manifest thresholds accept /
defer / refuse; grant lane for enrolled identities.
*Validation:* under saturating background traffic, distress-class delivery
holds within its slice; with the feed idle, sub-threshold traffic flows
untouched; a forged priority flag without stamp or grant gets classified
routine.

**CV3: Alert envelope.** CAP-shaped signed alert from an enrolled authority
key over a prescribed tree, counter-window replay defense, expiry honored.
*Validation:* replayed and expired alerts are rejected on every node;
an alert injected from a non-enrolled key propagates nowhere.

**CV4: Scope-deferred settings.** Settings records gain scope deference
(node, island, community, region); signalman edits scope policies; a
deferred node tracks its scope's published policy live (no forced restarts).
*Validation:* flipping a region policy reaches a deferred node and not an
overriding one; deference is visible and revocable in signalman.

**CV5: Remote shaping commands.** FS2 envelope grammar for power, profile,
dwell, participation; regulatory floor unoverridable.
*Validation:* an envelope commanding out-of-region power is refused on the
board; a valid shaping command round-trips over a foreign bearer; a
non-enrolled node ignores the county key entirely.

**CV6: Atlas scene MVP.** Offline PMTiles basemap in a cambium canvas scene
with live node positions, last-heard, and prescription health.
*Validation:* renders with networking disabled from local archives; a
prescription failure is visible within one beacon interval.

**CV7: Corroboration primitive.** Countersign envelope; corroborator-funded
relay and pin spend; corroborated reach scaling with independent personae;
authority alerts corroborable without touching validity.
*Validation:* a message's measured reach scales with corroborator count and
stops at the corroborators' aggregate budget; a ring of one persona behind
many nodes multiplies nothing; corroborating an alert changes its reach and
never its authenticity status; a corroborated item remains retrievable from
a corroborator after the originator goes dark.

## Rulings (Mark, 2026-08-12)

- **D1, carriage:** role plus scope default. Carriage is a declared role
  (router, hotspot, transport node); each role's duty comes from the scope
  policy the node defers to; the owner's explicit setting always overrides.
- **D2, diff promotion:** manual by default, policy opt-in. Auto-promotion
  is offered only for roster-constrained prescriptions; hop-listed
  prescriptions always promote by a human act.
- **D3, authority enrollment:** scope-owned rosters. The authority roster is
  part of the scope policy itself; enrollment and revocation are scope-policy
  edits by the scope's governor; bootstrap is the scope's founding record.
  No separate roster object, no deployment root key.
- **D4, atlas spike:** galileo-as-chisel-leaf runs first. Receipt: offline
  PMTiles basemap inside signalman-desktop with networking disabled.
- **D5, warnings and corroboration:** dissolved rather than chosen.
  Corroboration is a cross-cutting primitive (doctrine section above), not a
  class; warning events are corroboration-funded announcements with no
  reserved slice; CV7 added to carry the primitive.

## Open questions

- ~~Whether the corroboration envelope unifies with mere's petition and pin
  shapes~~: **unification is the default design as of the 2026-08-12
  ratification.** The corroboration envelope is petition-shaped; what
  remains open is the format spec itself, and the standing constraint that
  the R4 request/HMU/proof formats must not preclude it.
- Independence weighting for corroborators: under the moot modeling, the
  cast economics (pseudonyms spend, personas earn) supplies the sybil story;
  the remaining spec work is how tessera stake and area scoping meter
  corroboration weight before CV7.
- Scope-policy partition merge rule: two nodes honoring different revisions
  during hour-scale propagation is the mere doc's open merge-rule question,
  now a named dependency of CV4. Decide there, consume here.
- The board-tier policy artifact format: small enough for the 256 KB parts,
  verifiable cold, carrying policy + roster + revision; and what a board
  does between artifact revisions when its scopes disagree (owner order
  applies, but the stale-revision case needs stating).
- Corroboration-driven pinning versus board storage reality: the T114 tier
  cannot pin much (stamp doc, 48 KB heap); pin duty likely lands on host-tier
  and gateway nodes by role, which folds into D1's role duties.
- Deanonymization by overlap: several personae active across overlapping
  scopes deanonymize by intersection (mere doc, cast section). Since scope
  membership is host-side, the exposure is the owner's, not the board's;
  signalman should surface it when an owner joins overlapping scopes under
  linked personae.
