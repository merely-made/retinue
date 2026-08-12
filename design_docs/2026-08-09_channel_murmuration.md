# Channel Murmuration: Runtime Channel Scheduling

> **Framing superseded, 2026-08-10.** The
> [listener executive and protocol leases](2026-08-10_listener_executive_and_protocol_leases.md)
> design removes this doc's center: there is no home channel and no visit.
> The executive's DetectionProfile/ReceiveProfile scan plan is the resident
> identity, and speaking any protocol is a bounded lease. The design rules and CM ladder
> below survive translated (mapping in that doc's "What dies, what survives"),
> and this doc remains their authority read through the lease model. CM1 is
> absorbed into LE2; CM2 through CM5 carry, with visit schedules read as scan
> plans and coverage division.

Design doc, 2026-08-09. Lifts the question the
[retinue-small plan](2026-07-31_retinue_small_plan.md) deliberately deferred.
Structural decision 4 made personalities boot-selected channels behind the
`Executive`, ruled switching by reboot, and noted "hot-switching is a later
question if it ever matters." This is that question mattering, and the shape it
should take when it does.

**Naming note.** *Murmuration* is TESS-walled as a product or crate name
(Murmuration, Inc., Bloomberg-backed civic-tech; rejected twice in mere's
naming rounds). Lowercase descriptive use only, as in this doc. No new crate is
implied anyway: the scheduler is a firmware module inside the executive, and
the host-side surface belongs to signalman. The murm / murmur / murmuring crate
family is already claimed and unaffected.

## The idea

One SX1262 speaks one PHY profile at a time, so today a board is a citizen of
exactly one mesh until reboot. The murmuration model keeps the home channel as
the resident identity and adds **visits**: the executive retunes to a foreign
channel in its registry, does bounded work there (deliver a text to a
Meshtastic channel, pick up queued traffic), and returns home. Before leaving
it signals a neighbor, so the flock covers what any one bird stops watching.
Individually every node hops; collectively every channel in the local registry
stays heard. Local rules, global coverage.

This is a known class: single-radio multi-channel MAC with rendezvous (802.11
off-channel operation, Bluetooth adaptive hopping, the SSCH/MMAC family).
Retunes cost milliseconds. The novelty is not the hop, it is the coverage
choreography and the multi-mesh citizenship.

## What already exists

- **The channel trait and the executive** (retinue-small N3): channels are
  start / serve / stop behind a common trait, the executive owns the radio
  privately, and `ChannelInfo::at_boundary` already lets the firmware ask when
  a channel's parser is at a frame boundary. A visit begins exactly there: the
  hop point is the boundary the probe machinery already knows how to find.
- **Flash residency is settled**: several channels fit resident in the 800 KB
  region (measured, retinue-small N2/N3), so hot-switching is a scheduling
  problem, not a memory problem.
- **What decision 4 sidestepped now comes due**: switch-by-reboot avoided
  channel-teardown correctness. A visit is stop, retune, start without a
  reboot, so teardown correctness is the first engineering gate (CM1).
- The [mesh scaling doc](2026-08-09_mesh_scaling_and_asymmetric_routing.md)
  supplies the metering, sensing, and scope machinery this design rides on
  (its FT1, FT3, FT5 in particular).

## Design rules

**1. Dwell time is the fifth surface of cost-metered refusal.** The mere
boundary doc names one law at four layers (airtime budgets, forwarder policy,
non-replication, pinning). Listening time is the same law at a fifth: a radio
has one unit of attention to spend across channels, and the scheduler meters it
with the same accounting FT1 builds for transmit airtime. One accountant, one
more ledger column. The anti-herd rule falls out: a node refuses to leave home
when it is the last cover a neighbor depends on.

**2. Coverage is emergent, with hysteresis and a floor.** Which channels a
neighborhood covers should be the shadow of observed demand, not a declared
assignment. Both island failure modes apply directly: schedules need hysteresis
or coverage flaps on every reboot, and a lone node needs a floor rule (stay
home; you cannot murmurate alone). Sequencing follows the island lesson too:
ship hand-configured dwell schedules first, log real per-channel demand, then
encode the emergent scheduler.

**3. The beacon is the control plane.** FT3's recently-heard list widens from
"who I hear" to "who I hear, on which channel, during which window." Schedule
announcement becomes an extension of link sensing rather than a new protocol,
and the link metric gains a temporal term: a neighbor is reachable only during
schedule overlap, so ETX is conditioned on it.

**4. Personae are disjoint per channel, and the schedule is the cover.** A
board that leaves the RNS channel moments before a matching message appears on
Meshtastic links its identities across meshes by timing alone, to anyone
watching both. Per the mere boundary doc's cross-transport rule, the board's
identity on each channel is unlinked by default, and linkage is policy. The
mitigation is structural: visits happen on schedule whether or not traffic is
pending, so transmission timing decorrelates from switching. Coverage duty
doubles as cover traffic.

**5. A visiting node is a mobile border gateway.** FT5's forwarder-side scope
policy must be channel-aware, because a murmuration node carries traffic across
mesh boundaries as its job. What it re-injects after a visit passes the same
refusal policy as any border relay, or every visitor punches a hole in island
containment. The authorization complement (signed command envelopes that make
a visit auth-neutral no matter what bearer carried it) is the
[field node security posture](2026-08-09_field_node_security_posture.md).

**6. A visit is a store-and-forward window.** The mere boundary doc already
rules that reconciliation splits by transport class, with the constrained tier
reduced to store-and-forward entry shipping. A dwell window is exactly that
shape: a bounded slot with a known per-channel MDU, into which the scheduler
bin-packs queued entries. Grants-as-data makes the payloads legal end to end,
since a signed grant is valid however it arrived, including via a thirty-second
Meshtastic visit. The delivery machinery partly exists: outrider's un-stamped
opportunistic delivery already passes both directions on hardware
([acceptance](2026-07-28_outrider_opportunistic_delivery.md)), and a visit
window is a scheduled occasion for exactly that opportunism.

**7. Visits are honest citizenship, abbreviated.** The Meshtastic done
conditions ([modem research doc](2026-07-19_modem_embedded_and_meshtastic_research.md))
stand: a visitor observes CSMA, waits for the
implicit ACK, retries on miss. The right posture is the mute-client role:
transmit, confirm, decline relay duty it cannot honor, leave. On the home side,
RNS links are stateful, so dwell windows must fit inside link keepalive
tolerances or announce their absence ("gone 800 ms, back at the frame
boundary"). The trunk guard from decision 4 also stands: foreign channels join
the registry only after passing their own gates, and the scheduler must not
re-center the product on multi-protocol parity. The board is the trunk; visits
are how branches get watered.

## The signalman lens

The scaling doc's island definition ("plot which destinations survive an
airtime budget from a given node") plus a channel dimension plus the dwell
schedule yields the cross-transport reachability view: what can get from A to
B, under what budget, via which channel sequence, animated over the schedule.
Three cuts of one dataset: geographic map, network topology, cross-transport
reachability. This is the surface no other mesh tool can draw, because no other
tool has nodes that are citizens of several meshes.

## Feature targets

Ordered; each gates the next. CM numbering to stay clear of the scaling doc's
FT series.

**CM1: Teardown-correct hot switch.** Stop, retune, start between two channels
on one board without reboot, at a frame boundary, under the executive.
*Validation:* a board alternates modem and node channels N times with no missed
teardown invariant (asserted at runtime, loud on divergence); flash writes stay
at legal boundaries; the receive path is provably quiescent before retune.

**CM2: Scheduled visit, static schedule.** Hand-configured dwell schedule; a
board delivers a queued frame on a second channel during a visit and returns
home.
*Validation:* home-channel frames sent during the visit by a control peer are
counted; the miss rate matches the configured dwell fraction within tolerance;
the visit fits the announced window.

**CM3: Beacon schedule advertisement.** The recently-heard beacon carries
channel and window; a neighbor learns a peer's schedule without configuration.
*Validation:* two boards with disjoint static schedules converge on a shared
rendezvous window within one beacon interval; an attenuated pair classifies the
schedule link one-way, matching FT3.

**CM4: Coverage rule with anti-herd.** The last-cover refusal and the
stay-home floor, on top of static schedules.
*Validation:* in a three-board neighborhood, injected foreign-channel demand
never draws all boards off the home channel at once; a lone board never leaves.

**CM5: Metered dwell.** Dwell accounting joins the FT1 airtime accountant;
per-channel attention budgets enforced.
*Validation:* a board saturated with foreign-channel demand holds home-channel
coverage above its configured floor; measured dwell matches the ledger within
tolerance.

Emergent scheduling (demand-driven dwell) is deliberately not a target yet, per
design rule 2: it waits on logged demand from CM2 through CM5 deployments.

## Open questions

- Dwell window versus RNS link keepalive: announce absence, tighten windows, or
  both. Interacts with the receive-future cancellation findings
  ([2026-08-08](2026-08-08_receive_future_cancellation.md)).
- Whether visit schedules are per-board settings records or a
  neighborhood-negotiated object, and where signalman edits land (settings
  write versus schedule petition).
- Per-channel persona derivation: whether channel identities derive from one
  root (hardened, unlinkable) or are independent roots, and what that does to
  attestation when a board *wants* to prove it is the same device across
  meshes (gateway advertisement is exactly that claim).
- Whether the V4's hand-on-radio low-power path (executive-per-dispatch) can
  host visits at all, or whether murmuration is T114-first until the V4 adopts
  the full boundary.
- Timing-correlation residue: scheduled visits decorrelate transmission from
  switching, but queue depth still leaks through visit utilization to an
  observer counting frames on both meshes. Whether padding is worth its
  airtime.
- Regulatory posture of deliberate retuning: the
  [collision mitigation notes](2026-07-24_lora_collision_mitigation_ideas.md)
  flag that deliberate hopping can move a device under FCC 15.247's
  frequency-hopping rules (channel count, minimum dwell). Second-scale
  protocol-level retuning is a scanner's behavior rather than FHSS modulation,
  but the question interacts with the region-locked-firmware posture
  ([FCC reselling doc](2026-07-20_fcc_reselling_flashed_radios.md)) and should
  be answered before murmuration ships in a sold unit.
