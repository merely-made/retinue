# Signalman radio survey and node placement

**Date:** 2026-09-06
**Status:** SP0 implemented and software-verified on 2026-09-10. SP1-SP5 remain
planned and none is physically accepted by this plan. The first slice consumes
saved files and does not depend on a new board image.

## Goal and scope

Let an owner answer where to put a radio, what a proposed placement might
change, and whether the installed radio actually helped. The working cycle is
**plan, place, measure, revise**. The Map section holds surveys and placement
scenarios over the same stable device identities used by Network.

This plan owns survey imports, geographic correlation, candidate placements,
terrain estimates, comparisons, and their Signalman presentation. It extends
[management S6/S7](2026-08-15_signalman_management_surface_implementation_plan.md)
and consumes [position PD6](2026-09-01_position_disclosure_plan.md) and the
[radio observation contract](2026-09-06_radio_observation_plan.md). Those plans
retain ownership of placement persistence, disclosed positions, and raw radio
facts respectively. SP gates do not close S, PD, WN, or LE gates.

The first useful result is a saved survey that shows a walk or drive, directed
reception samples, explicit unknown intervals, and a before/after comparison
for moving one radio. Terrain prediction and placement ranking follow that
result. No new GUI toolkit, firmware authority, routing protocol, or public
position broadcast is required to import an owner's files.

## Reference and evidence limits

[Reticulum Network Planner](https://github.com/0xSeren/Reticulum-Network-Planner)
was inspected at `e5a85a1567bd5c7d7d0d618062f03d5bdcf402dd` on 2026-09-06.
Its [MIT license](https://github.com/0xSeren/Reticulum-Network-Planner/blob/e5a85a1567bd5c7d7d0d618062f03d5bdcf402dd/LICENSE)
permits source reference with attribution. No source is incorporated here.
Any later port must record the exact upstream files and notices; licensing of
terrain, population, and basemap datasets is a separate import concern.

Its [correlation code](https://github.com/0xSeren/Reticulum-Network-Planner/blob/e5a85a1567bd5c7d7d0d618062f03d5bdcf402dd/src/correlate.jl)
joins GPS tracks to signal samples and checks for reverse observations. Its
[terrain test](https://github.com/0xSeren/Reticulum-Network-Planner/blob/e5a85a1567bd5c7d7d0d618062f03d5bdcf402dd/src/los.jl)
compares sampled elevation with a straight line between antenna heights.
The [optimizer](https://github.com/0xSeren/Reticulum-Network-Planner/blob/e5a85a1567bd5c7d7d0d618062f03d5bdcf402dd/src/optimize.jl)
combines configured distance limits, terrain visibility, weighted incremental
coverage, several starts, and connectivity-preserving swaps. These are useful
baseline algorithms, not a measured RF model or a global optimality proof.

Signalman must not inherit two convenient but misleading implications:
missing reverse samples do not alone prove a one-way link, and missing terrain
samples do not establish clear terrain. Both remain unknown until evidence
supports a stronger classification.

## Authority and proposed file ownership

Paths below marked new are implementation targets, not existing APIs.

| Owner | Files and responsibility |
| --- | --- |
| Observation lane | `crates/radio-hand/src/observation/` owns bounded event facts, source clocks, losses, and listening intervals; Signalman's `observation.rs` and `observation/persistence.rs` own the versioned `ObservationBundle` and `StoredCapture` export. SP imports that saved evidence rather than inventing a second firmware event codec or treating Postilion as capture authority. |
| Signalman survey library | `apps/signalman/src/survey.rs` (implemented for SP0), with `survey/` reserved for later extraction, and registration in `src/lib.rs`: bounded imports, evidence classification, scenarios and reproducible analysis. This is independent of desktop rendering. |
| Mere Signalman port | `repos/mere/ports/signalman/`: owner placement and scenario storage, stable device association, retention/export authority. Extend the S6 record once; do not duplicate it in desktop state. |
| Desktop face | `apps/signalman-desktop/src/map.rs` (new), `state.rs`, `views.rs`, `lib.rs`: Map projection, selection, controls, layer explanations and accessibility using Cambium and Scenomise. One agent owns these shared files during integration. |
| Placement analysis | Initially `apps/signalman/src/survey/terrain.rs` and `placement.rs` (new): caller-supplied terrain and candidate data, cancellation and work budgets. Extract a general crate only when another consumer warrants it. |
| Hardware and control | Existing board owners, Postilion and Linkboy remain authoritative. A proposed map pin cannot configure, provision, transmit, or flash a radio. Installing a candidate is an explicit management action. |

Current Mere is a concurrently modified checkout. Its edits and dependency pin
moves require a separately owned integration slice; a Retinue-only survey
model can be implemented and tested without moving those pins.

## Evidence model

Keep source captures immutable and analysis derived. Each import records its
format/version, source hash, collector identity when known, image/profile
metadata when supplied, import diagnostics, and chosen time alignment. Owner
annotations and candidate placements are separate authored facts.

A survey joins four kinds of data:

- **Position samples:** track id, coordinate reference, latitude/longitude,
  optional altitude with datum, timestamp, source and optional accuracy. Owner
  pins, GPX fixes and disclosed peer positions keep their distinct provenance.
- **Radio observations:** observer, direction and peer attribution when known,
  interface and exact receive/transmit profile when known, source session,
  sequence, source time, signal quality, and capture/transmit/refusal outcome.
- **Listening and test opportunities:** when a receiver was actually available
  for a profile, plus independently recorded test transmissions or a declared
  and verified test schedule. This supplies the denominator for delivery rates.
- **Analysis assumptions:** clock mapping and uncertainty, interpolation gap
  limit, freshness window, spatial aggregation, selected data sources and
  model parameters. Save these with the result so replay is reproducible.

Board monotonic time is meaningful only within its boot/session. Host receipt
time is not automatically RF event time. Clock joins carry offset/drift bounds;
events across resets or outside the permitted GPX gap remain unmatched. A
coarse disclosed position remains coarse throughout analysis and export.

Reception establishes a directed observation. A reverse observation adds the
other direction. A verified reverse test opportunity with no reception may
support a bounded failure statement; absent reverse logs mean unmeasured.
Likewise, packet absence during sleep, another profile's lease, capture-buffer
overflow, collector disconnect, or unknown clock alignment cannot become an RF
dead zone. The UI presents the cause when known and otherwise says unknown.

Predicted terrain visibility, estimated radio coverage, observed reception,
and confirmed delivery are separate layers. Every derived edge retains the
evidence and model that produced it. Candidate nodes have scenario-local ids
until explicitly associated with an installed device; proximity never merges
identities. Packet receipt never establishes transit authorization.

## Phases and done-conditions

### SP0. Import and replay one survey

Add caller-bounded GPX and observation-export readers with explicit byte,
record and track-point limits. Reuse an appropriate existing XML parser after
checking dependency fit; do not execute entities or fetch resources named in
an imported file. Define a documented CSV/JSON host interchange for third-party
captures, with unsupported fields preserved as unknown rather than fabricated.
An adapter for the reference planner's saved JSON is optional and independently
versioned; importing its output does not require installing Python RNS.

**Done when:** fixed fixtures cover a moving track, stationary observer, reset,
clock offset, long GPS gap, asymmetric observations and capture loss; repeated
imports produce the same records and diagnostics; invalid coordinates,
oversized inputs and nonfinite values are refused or quarantined explicitly.
Export and reimport preserve the original evidence and analysis settings.

### SP1. One useful Map view

Project survey tracks, samples and owner pins through the existing geographic
arrangement. Add layer selection, time range, signal metric and evidence-kind
filters. Selecting an installed node selects the same identity in Network.
Unplaced or unmatched observations stay inspectable. Save comparison scenarios
through the S6 ownership seam. The first view works offline on a projected
plane; optional cached basemaps cannot gate it.

**Done when:** a headed receipt opens a saved survey with network disabled,
selects a directed sample and explains its origin/uncertainty, shows a known
listening gap distinctly from a failed test, and compares two captured
placements. Reload restores owner settings and selection associations. Keyboard,
focus and accessible alternatives expose the same facts as the colored map.

### SP2. Terrain estimates and movable candidates

Import owner-selected cached elevation data with resolution, datum, extent,
source/license and checksum. Compute terrain profiles and visibility for
candidate sites with configured antenna and receiver heights and distance
limits. Missing tiles or unknown height/datum yield an unknown result. An
initial straight-line model must identify its omitted effects; additional
Fresnel, diffraction or propagation models get separate validation and versioning.

**Done when:** synthetic flat/ridge/missing-tile fixtures give expected
clear/blocked/unknown results; moving a candidate recomputes only derived
results; cancellation and grid limits bound work; the displayed result names
the model and inputs. A field comparison reports disagreements without
rewriting observations or claiming terrain visibility guarantees a radio link.

### SP3. Suggest placements under explicit objectives

Begin with owner-supplied feasible sites: access, available power, allowable
antenna height and installation cost are constraints the owner supplies.
Offer objectives for target sites reached, additional area, resilience, and
later population weighting. Let the owner choose the node budget and required
connection to existing sites. Rank candidates under the selected model and
state when a constraint cannot be met. Automatic grids are a later input mode.

**Done when:** small exhaustive cases bound the heuristic's quality; disconnected
high-score sites are refused; removing a chosen relay exposes lost connectivity;
ties are deterministic; saved inputs and model versions reproduce a ranking.
Report proposed reachability and marginal benefit, never certified service or
global optimality. Population datasets are optional, dated and user-selected.

### SP4. Close the field comparison

Use one declared site-change experiment with recorded board/image identity,
PHY settings, antenna setup, source clocks, transmitted test opportunities and
listening exposure. Retain both directions separately. Hold the configuration
fixed except the intended site change, or disclose the changed variables.

**Done when:** one saved artifact reconstructs before/after observations and
unknown intervals, describes predicted versus measured changes, and records
sample counts and collection overhead. A second host can replay the result
offline without access to the original board or owner secrets. This may supply
PD6 evidence when its own position/disclosure conditions are also met.

### SP5. Coverage over time

Consume actual executive listening intervals and leases once the LE lane
produces them. Extend scenarios to show which receive profiles lose coverage
during talk, sleep, quiet control operations, and peer disappearance. Model
assignment changes explicitly; a geographic edge alone does not imply a
simultaneous temporal path or sustainable airtime capacity.

**Done when:** replay of the two-listener experiment explains a covered lease,
a peer-loss interval and the survivor's restored floor; predictions are compared
with counted opportunities. Static imported surveys do not wait for this phase.

## Dependencies and implementation assignments

SP0 can begin with exported fixtures and manual positions before onboard GNSS,
BLE, WiFi, terrain downloads, PD4/PD5 disclosure, or the resident scheduler.
SP1 consumes the S6 placement/selection seam; it does not need S7 over-air
telemetry if positions were imported. SP2 follows the survey evidence model;
SP3 uses SP2; SP4 requires the chosen field collector; SP5 requires real LE
events. None of these gates needs a new whole-stack graphics architecture.

After the observation contract is settled, Luna can own bounded import/export,
fixtures, and pure projections. One desktop integration owner handles Map and
shared state/views. Terra owns any required firmware source events and control
boundary changes, under the cross-lane ownership agreement. One coordinator
owns physical flashing and test commands across both radio lanes.

## Open decisions

- Which existing capture format supplies the first real imported survey, and
  how its clock is anchored. Use synthetic fixtures before claiming real data.
- Which local terrain source/resolution is practical for the first region.
  Dataset choice affects storage and uncertainty; the provider stays replaceable.
- The first installation objective and allowed candidate sites. Defaults remain
  visible settings, with area/redundancy/target-site choices available as added.
- Retention and export precision for shared surveys. Importing a private GPX
  file does not authorize publishing it or linking identities across protocols.

## Findings

- **2026-09-06:** `apps/signalman-desktop/src/state.rs` already names Map, but
  there is no `src/map.rs`. `SurveyState` currently refers to device discovery;
  radio survey state must use a distinct type. `network.rs` supplies the existing
  graph-canvas projection path. These are consumers to extend, not implemented
  coverage planning.
- **2026-09-06:** `crates/postilion/src/management.rs::ManagementSnapshot`
  carries bounded announce/route/link facts and counters, not a timed RF survey.
  The observation lane must supply missing listening and capture facts.
- **2026-09-06:** PD6 specifies signal/position/time capture with a stationary
  spatial anchor. S6 specifies persona-owned placement and geographic projection.
  This plan keeps those owners while defining the analysis and scenario product.
- **2026-09-06:** `apps/signalman-desktop/Cargo.toml` pins Cambium to Mere
  `d82afa17e2cca86da843f07a2d718d2e69eb9f10` and Genet to
  `115d348deddc344d949754e63beaece47cf49f34`. A desktop receipt must state actual
  pins; local Mere work is not automatically part of this consumer.

## Progress

- **2026-09-10:** SP0 implementation adds a bounded GPX pull reader and a
  versioned `signalman-survey-capture` JSON interchange in
  `apps/signalman/src/survey.rs`. Original-byte SHA-256, collector receipt
  provenance, boot/session clock mappings with uncertainty, reset/gap/loss
  issues, unmeasured reverse evidence, and saved replay settings are distinct
  fields. The StoredCapture schema-1 adapter keeps its raw source clock and
  records unknown intervals; it does not invent a peer or RF timestamp. Eleven
  focused importer/replay fixtures pass, and library-only strict Clippy passes,
  with the lockfile and dependency cache offline. This receipt is synthetic
  software evidence; it does not claim a field capture or physical acceptance.
- **2026-09-06:** created the separate survey/placement plan at the owner's
  request; inspected the reference algorithm and existing Signalman/PD seams.
  All SP gates remain planned. No firmware, dataset download, new dependency,
  capture, or public release is part of this planning receipt.
