# Mesh Scaling and Asymmetric Routing

Findings from a design session, filed 2026-08-09. Reticulum behavior at
regional and continental scale. Everything here is local-policy work: none of
it requires a wire format change or a fork. The moot-layer counterpart from the
same session is mere's
[boundary, identity, and grant composition](../../mere/design_docs/moothold_docs/research/2026-08-09_boundary_identity_and_grant_composition.md);
the [channel murmuration design](2026-08-09_channel_murmuration.md) builds on
both.

## 1. What breaks first is announce airtime, not table size

Announce flooding costs O(destinations x transport nodes). At a million
destinations announcing hourly with realistic rebroadcast fanout, discovery
gossip alone exceeds the aggregate capacity of a continental LoRa network. The
network dies of its own path discovery before carrying a single message.

Path table growth is the second-order problem. Flat address hashes carry no
topology, so there is no aggregation and no CIDR equivalent. Every transport
node holds an entry per known destination, plus the retained announce for
replay. Survivable on a Pi, fatal at the microcontroller tier.

**Order of work:**

1. **Implementation completeness.** Announce rate and airtime caps, path TTL and
   expiry, bounded path table with LRU eviction. All three are in the spec and
   absent from Retinue. The airtime cap is the highest-value single change in the
   project.
2. **Existing hooks, used hard.** Boundary and gateway interface modes already
   change announce propagation. Islands announcing densely inward and
   summarizing outward is the shipped hierarchy primitive, underused.
3. **Discovery inversion.** Announce locally, resolve globally on demand. A
   rendezvous or DHT layer built as an ordinary application turns announce scope
   local and makes global reachability a lookup. Trades first-contact latency for
   a table that stops growing.

**Cross-implementation caveat.** A stock RNS node inside the mesh floods. It has
no knowledge of scoped announces and will undo boundary discipline from the
inside. Scaling properties are therefore a function of which implementation holds
the borders. This is a governance problem in routing costume and has no code fix.

## 2. Propagation scope: meter airtime, not hops

Hop count is available (header hops byte, global max) but measures nothing worth
limiting. One TCP hop across an ocean is farther than five hops across a valley,
and an SF12 transmission costs roughly 100x an SF7 one in the only resource that
is actually scarce.

**Metric:** accumulated airtime budget. Sender declares a budget, each forwarder
decrements by what the transmission actually cost on that interface, drop at
zero. Free interfaces cost nothing, so announces cross backhaul freely and stop
at the RF edge. Carried in signed announce data, so it is tamper-evident and
needs no wire change.

**Interop-safe fallback:** originate with the hops byte preloaded near the global
maximum, and the announce dies a fixed distance out, enforced by any
implementation. Cost is real: hops doubles as the path metric, so every receiver
treats the destination as maximally distant and multi-path selection degrades.
Usable for one-off scoped announces, not as architecture.

**Enforcement lives at the forwarder, not the sender.** Signed scope is
tamper-evident, not enforceable. What is unilaterally enforceable is refusing to
relay announces from outside a local set: no sender cooperation, no wire change,
effective against stock nodes.

IP multicast ran this experiment. TTL-scoped multicast was the sender-declared
version and was widely ignored; administratively scoped boundaries, meaning
border routers filtering on their own policy, is what worked.

**Build order:** forwarder-side island policy first (enforcement), sender-side
airtime scope second (courtesy).

## 3. Islands are emergent, not declared

Political geography is the wrong partition. County lines are invisible to a
radio; ridges are not. Political units nest cleanly and RF cells do not, and one
ridge relay serves several counties at once.

If decay works, islands are never defined. An island is the shadow cast by the
cost function: plot which destinations survive an airtime budget from a given
node and the boundary is drawn without anyone agreeing to anything. Decay is
continuous where a declared boundary is binary, so falloff is graceful.

Boundaries settle at bottleneck links. One expensive relay carrying traffic for
many nodes behind it is a cut edge, locally detectable without global knowledge,
and exactly where summarization belongs. A node observing that most of its
traffic is cheap and local while a thin slice costs 50x can begin summarizing
across the expensive link on its own.

Ridge-and-valley terrain is unusually favorable here: RF cells are already
watershed-shaped, so terrain hostile to coverage is generous with partitioning.

**Two failure modes:**

- **Flapping.** Emergent boundaries move whenever a relay reboots, and routing
  churn is worse than a suboptimal partition. Boundaries need hysteresis: slow,
  sticky, requiring sustained evidence to shift.
- **Islands of one.** A node with a bad antenna reads as its own island under
  every natural metric. Needs a floor.

**Sequencing.** Do not build partition discovery first. Ship hand-configured
static island IDs, run the deployment, log where real cuts fall, then encode a
scheme. A partitioning algorithm designed before measurement is designed around
imagined terrain.

**Division of labor:** topology decides where a boundary is, people decide what
it is called. Keep those separate or a naming dispute redraws routing.

## 4. Asymmetric links

Path loss is reciprocal and antenna gain appears in both directions. The
asymmetry term is exactly **TX power delta plus noise floor delta**. It compounds
in practice because well-sited nodes are also the noisier ones, so both terms
point the same way. Raising TX is a firmware knob; lowering a noise floor is site
work, so the easy half gets done and the hard half does not.

LoRa widens the damage. Processing gain decodes well under the noise floor, so
usable links span an enormous range of path loss and the marginal band where a
few dB decides direction is wide rather than a cliff.

**Why flood routing cannot cope.** It has no link object at all: no neighbor
table, no per-neighbor delivery ratio, nowhere to record that a link works one
way. It cannot route around asymmetry because it cannot represent it.
Flood-then-source-route is worse in one specific respect, since it validates a
path in the forward direction and then unicasts in reverse, which is precisely
the assumption asymmetry breaks.

**Fixes, both old and cheap:**

- **Advertise who you hear.** A short recently-heard list in the periodic beacon.
  A link is symmetric when you appear in your neighbor's list. This is OLSR link
  sensing, costs a few bytes, and converts asymmetry from invisible to a
  first-class fact.
- **Use a bidirectional metric.** ETX encodes both directions by construction
  (1 / forward delivery ratio x reverse delivery ratio), so perfect-forward
  dead-reverse costs infinity rather than one hop. Babel (RFC 8966) is built on
  this and has years of deployment on community meshes with lossy asymmetric
  links.

**Path selection is local policy, not wire format.** RNS selects on hop count,
and RNode interfaces already surface RSSI and SNR. Substituting a delivery-ratio
metric is a local change that remains fully interoperable. Same category as
forwarder-side island policy.

**Feeds the airtime metric.** A high-power node sterilizes more area per
transmission, so each packet costs the shared medium more than a quiet node's
does. Under an airtime-cost metric it should receive a *smaller* announce budget,
not a larger one. Power currently buys reach with no charge attached; metering by
area cost inverts that without banning anything.

## 5. Capacity ceiling

Coverage and capacity are unrelated and only one is achievable. Continental
coverage is a volunteer-scale problem with direct precedent in amateur repeater
networks. Aggregate capacity is not: LoRa is a shared unslotted medium where
adding nodes to a cell reduces per-node throughput, and continental aggregate
lands in the low tens of Mbps for all users and all traffic.

This is a design envelope, not a defect. It fits text messaging, telemetry,
position and status, emergency coordination, signed bulletins, and key
distribution. It does not fit images at scale, voice, browsing, or model
inference of any size.

**Consequence:** success degrades the network. 10x users is 1/10th throughput per
user, so adoption makes it worse. The only exit is interface tiering, with RF for
last mile and real backhaul on trunk routes. Multi-interface endpoint handling
matters more than routing parity and should be settled before scale.

**Corollary, agreeing with Section 3:** dense regional islands that are actually
useful beat thin continental coverage that is useful nowhere.

## 6. Maintenance is the real constraint

Deployment is a weekend; sustaining is generational. 100k nodes at 2% annual
failure is 2,000 site visits per year. Amateur repeater networks survive because
trustees maintain them for decades, not because someone bolted them up once. This
is a social problem that the codebase does not touch, and it should be named in
any deployment planning rather than discovered later.

## Feature targets

**FT1: Airtime accounting.** Per-interface transmission cost model; announce rate
and airtime caps enforced.
*Validation:* a node saturated with announce traffic holds transmit airtime under
its configured cap; measured cost per announce matches modeled cost within
tolerance across at least two interface types.

**FT2: Path table bounds.** Path TTL, expiry, LRU eviction.
*Validation:* table size plateaus under continuous announce load from more
destinations than the configured bound; eviction preserves recently used paths.

**FT3: Bidirectionality sensing.** Recently-heard list in beacons; symmetric
neighbor determination.
*Validation:* a deliberately asymmetric pair (one node attenuated on receive) is
classified one-way by both ends within one beacon interval.

**FT4: Delivery-ratio path metric.** ETX-style forward and reverse tracking
feeding path selection.
*Validation:* given a one-way link and a longer symmetric alternative, selection
chooses the symmetric path; interop with a stock RNS peer is unaffected.

**FT5: Forwarder-side scope policy.** Configurable refusal to relay announces
from outside a local set.
*Validation:* a stock RNS node inside the boundary does not cause out-of-scope
announces to escape through a Retinue border node.

## Open questions

- Merge rule when two nodes with different policy revisions have already acted.
- What the island floor should be, so a single poorly sited node does not read as
  its own island.
- Whether hysteresis on boundary movement is time-based, evidence-count based, or
  both.
