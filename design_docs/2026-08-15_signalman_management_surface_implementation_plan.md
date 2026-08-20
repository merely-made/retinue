# Signalman management surface implementation plan

**Date:** 2026-08-15
**Status:** implementation in progress. S0 and S1 are verified. S2's model,
retained graph, layout actor, and headless face are verified; headed and live
receipts remain open at the named pin boundary. S3's five-section shell,
owner settings, unavailable states, and headless navigation receipts are
verified.
This opens named gates for the direction in
[Signalman as a management surface](2026-08-15_signalman_management_surface_direction.md).
The standing Peer, Air, Assurance, and Distribution authorities remain intact.
These lanes divide this product work; they do not authorize more than two active
streams or let presentation code take radio or flashing authority.

## Verdict

Build one narrow vertical proof first:

```text
retinue live facts
  -> postilion management snapshot
  -> Signalman device-data mere
  -> force layout + Network view
  -> headed selection and accessibility receipt
```

Do not build a Network-shaped cache in the desktop and move it later. The first
visible scene must already be derived from stable radio identities in the
device-data mere. That proof settles the dependency graph, source pins, update
model, physics ownership, selection, and desktop composition before Messages,
Map, or Browse multiply the same mistakes.

The primary desktop sections become **Devices**, **Network**, **Messages**,
**Map**, and **Browse**. Devices retains the existing Linkboy owner flow. The
other sections do not gain flashing authority.

## Corrections to the direction note's implementation premises

The product ruling stands. Five source claims need narrower implementation
language.

1. **The current listener lease is not live state yet.** `radio-hand` still
   persists a boot `Channel` and runs `Channel::start/serve/stop`; LE1 and LE2
   have not replaced that boundary. Network may show the current boot channel
   as observed configuration. It must not draw a speaking lease until the Air
   lane exposes a real lease fact.
2. **Outrider propagation peering is open.** A decoded propagation announce can
   establish the role `propagation node`. It cannot establish a peering edge.
3. **Postilion does not expose the claimed network snapshot.** It retains only
   first-seen delivery peers. Retinue's path table and link registry are
   private; announce ingress, hops, transport, refresh time, and peer-attributed
   live links are not one public read model.
4. **`gaz` has neither a location facet nor persistence.** It is a contact
   model. Owner-authored device placement belongs in a persona-scoped Signalman
   record beside the device grant. Contacts may refer to a placed device, but
   they do not own its site.
5. **The geographic solver already exists.** `sceno::Arrangement::Geographic`
   and `scenomise::solve` have a fixture proof. Atlas work is the first product
   adapter and canvas consumption, not a new solver. Conversely, Pelt is a
   document host/surface rather than an embeddable Cambium pane, so Browse is a
   composition and pin gate, not the cheapest section.

`NODE_SHEET` is currently a private `mere-canvas` stylesheet constant. It
controls node appearance, not identity. Signalman uses stable graph identities
and the existing Cambium graph-canvas focus/selection contract in the first
proof. Visual declarations move to a shared contract only when Signalman is a
real second consumer, with the old private declaration removed in the same
change.

## Authority and data ownership

| Authority | Owns | Must not own |
| --- | --- | --- |
| Retinue `Endpoint` | routes, interfaces, links, announce ingress facts and routing counters | product roles, labels, contact identity, UI state |
| Postilion | bounded host snapshot over one station; clocking and source generations | graph nodes, Signalman roles, persistence policy |
| Signalman | device vocabulary, source reconciliation, message status wording, management actions | Retinue routing internals or Linkboy plan construction |
| Device-data mere | stable device/destination/site graph, typed radio relations, observed/stale history, selection identity | radio I/O, delivery claims, flashing policy |
| Signalman desktop | sections, viewport, focus, user settings, presentation of facts and refusals | inferred routes, inferred hardware identity, hidden actions |
| Mere Signalman port | persona/device grant, sealed station state, owner placement records | radio routing and Linkboy execution |
| Linkboy | package trust, immutable plans, execution, recovery, receipts | graph or messaging policy |
| Genet/Nematic/Pelt | document parsing, sessions, layout, rendering and content-surface mechanics | Reticulum addressing or security posture |

The device-data mere begins as
`apps/signalman-desktop/src/device_mere.rs`, using an exact-pinned Mere family:
Chartulary for its logged graph and Seiche for force layout. A second host
consumer is required before extraction into a package. Graphshell/Turnstone is
that candidate. When it arrives, extract the module and delete the app-local
implementation in the same gate.

## Source model

### Retinue facts

Add read-only diagnostic values without exposing the mutable tables:

- `AnnounceFact`: destination, announced identity, opaque app data, ingress
  interface, hops, transport, and observation sequence;
- `RouteFact`: destination, interface, optional transport, hops, and age at one
  captured instant;
- `LinkFact`: link id, interface, link kind, direction, and only the remote
  identity or destination the protocol actually proved;
- existing routing, queue, and announce-admission counters.

If a link has no attributable peer, its fact says `unknown`. It may appear in a
diagnostic list but does not become an edge to a guessed node. Retinue must
retain the missing direction/peer facts when a link is registered or later
IDENTIFY proves them; the view cannot reconstruct them from a COM port or link
id.

### Postilion snapshot

Add `crates/postilion/src/management.rs` as a module, not a crate. Its bounded
`ManagementSnapshot` contains:

- station identity and radio configuration;
- current routes and attributable links;
- a configurable bounded announce-observation history, refreshed on repeated
  announces rather than first-seen only;
- typed delivery and propagation announces where Outrider can decode them,
  plus unknown announces preserved as unknown;
- routing/queue counters and one monotonically increasing generation.

Snapshot construction takes an injected or caller-supplied time in tests. No
serialized fact contains `Instant`, and one capture uses one clock sample.

Postilion does not derive `endpoint`, `transport relay`, `propagation node`,
`peer`, or `known but stale`. Signalman derives those roles from the snapshot
and an owner-configurable stale threshold.

### Device-data mere

The app-owned graph uses stable IDs derived from source identities, never list
position or display name. Radio relations use an open Signalman vocabulary,
including:

- `signalman:heard-announce`;
- `signalman:route-via`;
- `signalman:live-link`;
- `signalman:propagation-peering`, only after real peering state exists;
- `signalman:placed-at`.

Every imported fact carries source generation, observed time, and provenance.
Applying the same snapshot twice produces no log edits. Disappearance first
makes an observation stale according to policy; it does not silently delete
owner-authored placement, contacts, history, or device identity.

The graph is the section source. Network, Map, Messages, and Browse may project
different views of it, but they do not maintain parallel node registries.

## Gates

### S0. Resolve the private application dependency graph

**Writes:** `apps/signalman-desktop/Cargo.toml`, its lockfile, and only the
Mere manifests required to make the private port consumable from Git.

Select one exact Mere revision for `mere-signalman`, Chartulary, Codicil, Gaz,
Sceno/Scenomise, and Seiche. Keep the existing exact Genet family until a gate
actually needs a newer one. If `mere-signalman` takes Retinue by Git, patch
that source back to this checkout in the desktop workspace so Cargo sees one
Retinue/Postilion family rather than local and Git twins.

**Done when:** a clean task-local Cargo home resolves with `--locked`; `cargo
tree -d` shows one source for every type crossing Signalman's Retinue, Mere,
Genet, Netrender, petgraph, and public AccessKit seams; the committed lock
contains exact SHAs; and no machine-local `.cargo/config.toml` is required.

**Receipt boundary:** the Windows host graph meets that source-identity gate.
Genet still resolves target-adapter-internal `accesskit_consumer` 0.35, 0.36,
and 0.38 across its broader metadata, so this is not a claim that every crate
whose name begins with `accesskit` has one version on every target. The pinned
`mere-signalman` manifest also understates its effective Rust floor as 1.92;
its `p2panda-core` dependency requires 1.96. The desktop declares 1.96, while
repairing the port's own metadata waits for a new Mere revision and pin bump.

**Stop:** do not begin scene code while two type-bearing families remain.

### S1. Produce the honest management snapshot

**Writes:** `crates/retinue/src/endpoint.rs`,
`crates/postilion/src/{lib,management}.rs`, and focused tests.

Expose the read-only Retinue facts, retain bounded repeated announce
observations in Postilion, and classify only formats that decode. Add a fixture
with a local station, direct delivery peer, one transported route, one unknown
announce, and one unattributed link.

**Done when:** snapshot order is deterministic; every route/link refers to an
existing interface; repeated announces refresh one identity and append bounded
history; unknown data stays unknown; expired routes do not appear current; and
snapshotting changes no endpoint state.

### S2. Land the Network projection proof

**Writes:** `apps/signalman/src/management.rs`,
`apps/signalman-desktop/src/{device_mere,network}.rs`, desktop state/view/theme
integration, and focused tests.

Reconcile S1 facts into one logged Chartulary graph. Feed its node keys and
typed edge pairs to a Seiche simulation running off the UI thread. Project its
layout snapshots through Cambium's graph canvas, using the same positions for
paint, native hit targets, labels, keyboard focus, and selection. Roles and
edge labels are pure derivations from graph facts.

The accessible companion view is a list over the same node and relation set,
not an independently assembled fallback. Stale nodes remain legible and are
not presented as live.

At the selected pins, Cambium's graph canvas accepts an external viewport but
does not emit pan or zoom actions. S2 therefore proves pan and zoom through
named controls over that same viewport. Pointer/wheel pan and zoom require a
targeted Cambium change and deliberate Genet pin bump. Mere's selected
`SitedStation` wrapper also keeps the Postilion station private and exposes no
management snapshot getter. Model, fixture, and headed face receipts can land;
the live sealed-station bench receipt stays open until Mere adds a read-only,
lease-checked getter and the desktop bumps to that exact revision.

**Done when:**

- applying one source snapshot twice produces no graph edits or physics reset;
- every visible edge has two present endpoints and a typed source relation;
- selection survives snapshot refresh and layout movement by stable ID;
- a headless scenario can select every node and inspect every relation;
- a headed fixture settles, pans, zooms, drags, focuses, and selects correctly;
- one live bench receipt shows at least one real announce and route without
  upgrading that receipt into a multi-hop or delivery claim.

This is the first release-worthy slice. Stop here for review before opening
Messages.

**Implementation receipt, 2026-08-15:** the Signalman projection preserves
unknown observations, derives roles only from decoded/current facts, uses stable
source identities, and converts fact ages using a caller-supplied capture time.
The desktop reconciles that material through one attributed Chartulary batch,
skips empty batches, retains disappeared identities as stale, and advances the
physics epoch only for topology changes. One bounded Seiche actor runs on its
own thread, keeps only the latest snapshot, drops stale epochs, handles
pin/unpin, and stops ticking after settling or a finite tick budget. Cambium's
canvas and its accessible companion rows consume one `DeviceProjection`; the
headless host selects and drags every keyed target and exercises the named
pan/zoom controls. The exact-pin desktop suite passes 20 integration tests plus
seven library tests.

This is verified through rung 2, **Headless face**. A real Genet/Netrender
window and physical station were not run. The selected Mere wrapper still
lacks the lease-checked management getter required to feed a sealed running
station into this graph, so the headed live fixture and bench receipt remain
open rather than being replaced by sample data.

### S3. Turn the desktop into a management shell

**Writes:** `apps/signalman-desktop/src/{state,views,theme,main,network}.rs` and
focused desktop tests. `network.rs` is in scope because force strength and
damping are runtime actor inputs rather than view-only preferences.

Add the five primary sections. Devices embeds the existing six-step installer
flow unchanged. Network hosts S2. Messages, Map, and Browse begin as explicit
unavailable states naming their unmet gate; they do not render sample data.

User settings own stale age, history bound, force strength, layout damping,
label density, and whether last-known observations remain visible. Defaults
are ordinary starting values, not policy constants hidden in the graph.

**Done when:** pointer and keyboard navigation reach every section and return
to the prior Devices stage without losing an active installer state; close
disposition still protects a running flash; the AccessKit tree names all five
sections; and the existing owner-flow tests remain green.

**Implementation receipt, 2026-08-15:** the desktop exposes Devices, Network,
Messages, Map, and Browse as stable named sections. Devices preserves the
installer state object; the other three unfinished faces state their actual
gate and carry no sample records or placeholder actions. Stale age, announce
history depth, force strength, damping, the pinned canvas's shown/hidden label
density, and last-known visibility are explicit owner state with visible
starting values. History depth is labelled as a next-connection setting because
Postilion has no runtime setter. Stale age supplies Signalman's projection
policy seam; the exact Mere pin still lacks the station snapshot lease that
would invoke it live. Physics changes advance a presentation epoch and
reconfigure the bounded Seiche actor. Last-known hiding filters the one shared
projection and its incident relations while Chartulary retains the facts.

The exact-pin locked/offline desktop suite passes seven library tests, five
accessibility tests, five management-shell tests, four Network-face tests, and
thirteen owner-flow tests. These are headless receipts. They do not close S2's
headed-window or physical-station boundary.

### S4. Messages, text and truthful status

**Writes:** Signalman's management/message modules, desktop `messages.rs`, and
the smallest required Gaz/Codicil adapters at the selected Mere revision.

Use Gaz for key-rooted contacts and Codicil for an append-only conversation
log. Persist outgoing intent before transmission, then append observed status
transitions. Rename Postilion's present `Sent::Delivered` presentation: its own
contract says it means handed to the radio, not end-to-end delivery. Direct,
resource, and propagation paths each expose only the strongest receipt they
actually have.

Unknown authenticated senders remain addressable without silently becoming a
saved contact. Contact creation is an explicit owner action. Conversation,
contact, and retention policy remain Signalman's.

**Done when:** restart replay reproduces one conversation exactly; duplicates
do not create a second message; an unknown sender is distinct from a Gaz
contact; offline/queued, handed-to-radio, accepted-by-propagation-node, fetched,
and failed states cannot be confused; and a two-process fixture exchanges text
with the visible status matching the transport receipt.

**Implementation receipt, 2026-08-19:** Signalman now owns stable authored
message identities, a self-checking versioned text envelope, append-only events,
chronological replay, deduplication, and fact-only status transitions. Postilion
preserves the authenticated LXMF id, sender key, and Data/Resource mode on
receive. Its former `Sent::Delivered` is `Sent::HandedToRadio` and carries the
actual Outrider receipt id and mode.

The desktop adapter stores `Codicil<MessageEvent>` and the persona-scoped Gaz
book through Muniment. The shipping desktop opens redb under the owner's local
application-data directory; headless state uses the memory backend explicitly.
A candidate log is validated and saved before it replaces the visible material.
Authenticated unknown senders remain addressable by key and destination, and
enter Gaz only through the named save-contact action. The Messages face renders
the current receipt wording and persists an offline outgoing intent before it
can be handed to a transport worker.

The two-process fixture uses separate sender and receiver processes over a real
Retinue TCP interface. It records the outgoing event before carriage, carries one signed
Signalman envelope through Outrider, converts both ends through Postilion's
shipping adapters, and proves matching application and LXMF transport ids. A
redb close/reopen test reproduces the exact conversation; duplicate events do
not grow the Codicil log. The clean exact-source desktop graph passes locked,
offline metadata and its full headless suite. The Retinue, Outrider, Postilion,
and Signalman family suite also passes locked and offline.

The desktop binary still has no provisioned station identity at startup, so its
ordinary UI can queue only after a host attaches that authority. The fixture
closes S4's process boundary; it is not a headed serial-radio receipt. Likewise,
the existing Outrider propagation submit receipt is not promoted to
accepted-by-node without an application-level storage acknowledgement.

### S5. Voice drops

**Writes:** Signalman message/audio modules and Outrider only where a missing
public carriage seam is proven.

Start with a PCM fixture and the already-shipped Pipit clip plus Outrider field
7 / `AM_CUSTOM` carriage. Add microphone capture and playback only after the
file-backed path is byte- and duration-receipted. Voice uses the same message
log and status model as text.

**Done when:** a clip recorded or injected at site A is encoded once, carried
through a propagation node, fetched at site B, decoded, and audibly played;
the receipt records codec, sample rate, duration, encoded bytes, transfer
mode, and decoded duration; cancellation leaves no message marked delivered.

**File-backed receipt, 2026-08-19:** the first S5 rung is implemented. A
bounded PCM16LE file is encoded exactly once into a checked Pipit clip, carried
only in LXMF audio field 7 under `AM_CUSTOM`, submitted to a real Outrider
propagation store, fetched by a second Retinue endpoint, authenticated, and
decoded. The retained `VoiceReceipt` names codec, sample rate, encoded and
decoded duration, encoded bytes, and the node-observed Data/Resource transfer
mode. Text and voice use the same replayable Signalman event log, while the
untagged text representation preserves S4's stored JSON shape. Cancellation is
terminal, so a later fetch fact cannot turn a cancelled message into a
delivery.

Outrider's field carriage was already sufficient. The only missing public
fact was which mode the propagation node used for its fetch responses, now
retained in `ServedFetch` rather than inferred from size. This rung is a
three-endpoint headless software receipt. Microphone capture, output-device
playback, the desktop action, and a headed two-site audible receipt remain the
rest of S5. Postilion's present direct-message event also omits LXMF fields;
live direct voice ingress must preserve that payload without copying the clip
into Signalman's metadata body.

**Host-audio and direct-ingress receipt, 2026-08-19:** the next S5 software
rung is implemented. The desktop enumerates stable CPAL input and output device
IDs, leaves both as owner-visible choices, and offers Pipit LPC-10, half-rate
LPC-10, or IMA ADPCM with a 10, 30, or 60 second capture bound. One ordinary
worker owns the host streams. Its real-time callback writes only into a bounded
buffer; completion downmixes and anti-alias downsamples host PCM to 8 kHz mono,
then Signalman encodes the captured PCM exactly once and appends the same
`OutgoingQueued` event used by file-backed voice. Selected decoded clips are
resampled for the selected output and yield an output-device, sample-rate,
channel-count, and decoded-duration receipt.

Postilion now retains the complete authenticated `LxmfPayload` in
`Event::Message` and exposes `Station::send_payload`; `send_bytes` remains the
text convenience wrapper. A real direct Retinue exchange proves field 7
survives Postilion and decodes through Signalman's ordinary `incoming_event`.
Desktop tests inject typed capture/playback events and therefore do not claim
that CI opened or heard a physical device. The remaining S5 receipt is a
headed two-site run that attaches the desktop to its sealed station authority,
records through a real microphone at site A, carries the queued clip through a
propagation node, fetches it at site B, and audibly plays it through the selected
output. At that receipt, the exact Mere port still had to expose that live
station lease; an injected `message_local` in a harness was not substituted
for it.

**Sealed-station runtime receipt, 2026-08-19:** Mere revision
`47966742923c48c2e33b74762458a9a6cd12484d` now exposes address,
management-snapshot, and complete-payload operations through the sealed
station head, with the retained authority rechecked at each operation. The
desktop pins that revision as one Mere family and restores the head only when
`SIGNALMAN_STATION_DATA_ROOT`, `SIGNALMAN_STATION_RECORD`,
`SIGNALMAN_STATION_PORT`, and `SIGNALMAN_STATION_NAME` form a complete explicit
attachment. Send patience remains owner-configurable through
`SIGNALMAN_STATION_SEND_PATIENCE_SECONDS`.

One worker thread owns the Tokio runtime, sealed identity, serial station,
message sends, authenticated ingress, and generation-deduplicated management
captures. The UI receives typed Signalman events, persists each carriage or
failure fact, and feeds live snapshots into the existing device-data mere. A
missing, partial, locked, expired, or stopped attachment remains visibly
disconnected. This is a concrete software consumption of the sealed authority,
not a headed or physical receipt. The microphone, propagation/fetch, second
site, and audible-output run remains the final S5 gate.

### S6. Owner placement and Atlas

**Writes:** `repos/mere/ports/signalman/**`, Signalman's `map.rs` adapter, and
the smallest Scenograph/graph-canvas changes proven necessary.

Add a persona-scoped `SitePlacement` record beside the device grant, sealed by
the existing Signalman port storage. It is keyed by stable device identity and
contains coordinates, provenance (`owner`, `telemetry`, `gps`), observation
time, optional accuracy, and retention policy. It is never written to board
settings.

Project placed nodes to `sceno::Arrangement::Geographic` and solve them with
the existing Scenomise path. Wire that scene into the same graph-canvas
selection identity as Network. V1 is an offline projected plane with range
rings. Imported basemap data is a later, optional user-selected layer.

**Done when:** an owner pin survives restart, changing a pin changes only the
placement fact, unplaced nodes stay in an explicit list, Network and Map select
the same identity, the geographic score round-trips, and a headed offline
receipt shows pins and range rings with network disabled.

### S7. Telemetry and moving positions

**Writes:** `crates/outrider/oracle/`, the captured fixture, the narrowly
decoded Outrider type, and Signalman's placement adapter.

Capture `FIELD_TELEMETRY` from the pinned stock oracle before parsing it. Keep
unknown telemetry fields lossless. A live GPS observation is ephemeral by
default; retaining it is an explicit owner setting. T-Echo or another
GPS-equipped board is a physical gate, not implied by the host fixture.

**Done when:** the oracle capture and public prose agree on the decoded subset;
malformed or unknown telemetry cannot move a node; owner pins outrank live
telemetry unless the owner changes the source preference; and a physical GPS
receipt names the exact board, firmware, field bytes, coordinate, and age.

### S8. Browse, render before fetch

**Writes:** first a disposable composition probe, then the proven Signalman
surface seam; Genet paths only after the probe passes.

The opening proof injects captured gemtext bytes. It does not fetch. Lower the
bytes through Nematic and embed one document session beside the Cambium shell.
Prefer an existing reusable `DocumentSession`/Pelt tile-surface contract over
copying Pelt viewer code. Prove scene composition, focus routing, scrolling,
links, and accessibility before widening the engine set.

Fetching waits for smolweb R-A addressing and R-B posture; R-C is needed only
when body size proves it. The UI must distinguish key-proven, name-resolved,
and unverified sources. One real Retinue fetch then replaces the injected bytes
without changing the document-session boundary.

Micron is an independent Genet lane: public prose plus black-box captures,
never NomadNet source. It lands in Nematic, then expands Knot's polyglot fence
and the HTML projection with round-trip fixtures. It does not gate the first
gemtext Browse receipt.

**Done when:** a clean pin bump yields one Genet/Netrender family; injected
gemtext renders and navigates in the real Signalman window; a Retinue-fetched
capsule produces the same document facts; posture is visible; and no content
byte path bypasses Signalman's address/security decision.

### S9. Graphshell guest proof and extraction

**Writes:** a Signalman Graphshell endpoint package and, only now, an extracted
device-mere package; Turnstone changes stay in its own repo/lane.

Serve the already-proven Network projection first, then Map. Graphshell sends
bounded score/scene facts and constrained intents. It receives no radio handle,
Linkboy planner, private station identity, or direct graph mutation authority.

**Done when:** Turnstone renders the same stable node/relation identities over
loopback and an authenticated remote carrier; selection intent is accepted or
refused by Signalman; reconnect resumes from a generation; the app-local
`device_mere` implementation is gone; and both desktop and endpoint tests use
the extracted package.

## Ownership lanes and path fences

These lanes describe authority. They are not a recommendation to run all of
them concurrently.

| Lane | Owns | Primary path fence | Opens at |
| --- | --- | --- | --- |
| **Facts** | Retinue diagnostic facts and Postilion snapshot | `crates/retinue/src/endpoint.rs`, `crates/postilion/**` | S1 |
| **Material** | Signalman roles, device-data mere, history reconciliation, force layout | `apps/signalman/src/management*`, `apps/signalman-desktop/src/{device_mere,network}.rs` | S2 |
| **Face** | management navigation, pane composition, input and accessibility | `apps/signalman-desktop/src/{state,views,theme,main}.rs` | S3 |
| **Correspondence** | contacts, text, voice, message receipts | Signalman message modules, narrowly `crates/outrider/**` | S4 |
| **Placement** | sealed site records, geographic adapter, telemetry capture | `repos/mere/ports/signalman/**`, Signalman `map.rs`, oracle capture | S6 |
| **Reading** | embedded document session, smolweb carriage adapter, Micron engine | Signalman `browse.rs`; `repos/genet/components/nematic/**` and proven host seam | S8 |
| **Guest** | Graphshell profile and Turnstone consumption | new Signalman endpoint package; Graphshell protocol consumers | S9 |

Facts conflicts with active Air edits to `Endpoint` or Postilion. Face conflicts
with Distribution whenever it touches desktop state/views or runs a physical
installer. Reading conflicts with any Genet root-manifest, Cargo.lock, Cambium
host, layout, or Netrender pin migration even when the Nematic source file is
otherwise disjoint. Placement must not edit the current dirty Graphshell native
host paths in Mere.

Run at most two streams:

1. one source/transport stream from Facts, Correspondence, or Placement; and
2. one host stream from Material, Face, Reading, or Guest.

Never share a root manifest or lockfile between them. Use `git commit --only`
with the lane's path fence and inspect `git show --name-only` after every commit.

## Verification ladder

Each gate reports the highest rung actually run.

1. **Model:** deterministic unit/fixture tests and serialization round trips.
2. **Headless face:** the real Cambium tree, semantic keyboard actions, and
   AccessKit projection.
3. **Headed desktop:** real Genet/Netrender window, pixels, focus, pointer,
   scrolling, resize, suspend/resume, and close disposition.
4. **Live host:** real serial station and observed Retinue facts.
5. **On air:** named boards, firmware, profiles, packet/receipt bytes, and an
   independent peer observation.
6. **Two site:** distinct operators or machines, propagation/fetch, replay,
   restart, and failure recovery.

Useful baseline commands:

```powershell
cargo test -p retinue -p postilion -p signalman --locked --offline
cargo test --manifest-path apps/signalman-desktop/Cargo.toml --locked --offline
cargo tree --manifest-path apps/signalman-desktop/Cargo.toml -d
cargo test --manifest-path C:\Users\mark_\Code\repos\mere\ports\signalman\Cargo.toml --offline
git diff --check
```

Physical and headed receipts add exact commands, SHAs, device identities, and
artifacts rather than replacing these software checks.

## Order and stop rules

Serial product order is:

```text
S0 -> S1 -> S2 -> review -> S3 -> S4 -> S5
                         \-> S6 -> S7
                         \-> S8
S2 + S6 --------------------> S9
```

After S2, Messages, Placement, and render-before-fetch Browse are independent
at the model tier, but the Face and root dependency files remain single-writer.

Stop when any of these occurs:

- a pane needs a second source of node identity or selection;
- a view would have to infer a route, peer, lease, location, delivery, or
  hardware identity;
- the same source revision produces graph churn or resets settled layout;
- a source pin resolves two type-bearing families;
- Map work starts inventing a replacement geographic solver;
- Browse work copies Pelt's windowed viewer instead of consuming/extracting a
  reusable document-session boundary;
- a physical installer run overlaps an Air bench session;
- Turnstone needs direct Signalman graph or radio mutation authority.

## Deliberate deferrals

- speaking-lease display until LE2 exposes a real fact;
- propagation-peering edges until Outrider implements peering;
- online tile services;
- live voice calls;
- fleet campaigns and remote flashing;
- public packaging of the private Genet/Mere composition;
- Micron fetching before its engine and the R-A/R-B carriage gates both pass;
- any claim that a host fixture proves RF, headed UI, or two-site operation.
