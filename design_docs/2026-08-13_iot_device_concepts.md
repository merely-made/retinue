# IoT device concepts

**Date:** 2026-08-13
**Status:** Direction note from a brainstorm session. Creates no gates and no
lane; the [program sequencing doc](2026-08-12_program_sequencing_and_deadline_order.md)
still orders everything against the 2026-09-01 ARDC intake. This records which
device concepts the stack uniquely enables, Mark's weightings, and where each
concept would slot once work opens.

## The thesis

Commercial IoT outsources three things to a vendor cloud: device identity,
transport, and history. This stack makes all three things the owner holds.

- **Identity:** personae gives every device a cryptographic identity with a
  trust roster, family-shared via shared_root. No vendor account exists.
- **Transport:** Reticulum is bearer-agnostic and delay-tolerant. LoRa in the
  field, WiFi/TCP indoors, outrider propagation for store-and-forward. A
  sensor in a dead zone delivers when a courier node passes.
- **History:** muniment's append-only log is the natural home for telemetry,
  on hardware the owner keeps.
- **Head-end:** in Turnstone a device is literally a node in a spatial
  dataspace, with its muniment history and gazette name attached. Every
  competitor's dashboard is a list; ours is the graph the mesh actually is.
- **Distribution:** Linkboy/Signalman user-flash onto stock certified boards
  is already the FCC v1 posture. Devices ship the same way, so IoT adds no new
  regulatory surface.
- **Security posture:** FS4/FS5 custody and seizure semantics exist in
  software. Hobby IoT has no designed answer to "this node was stolen from the
  field"; we do.

The work-lanes doc already reserves the seam: DIST6 asks whether the V4 field
gateway should expose browser rendezvous to Turnstone, and civic deployment is
the phase-two consumer of the same machinery.

## Concepts

Ordered by Mark's weighting from the 2026-08-13 session, not by original
ranking.

### 1. Ephemeris e-ink family (elevated: "a lot of charm and uniqueness")

Cleromancy's analytic ephemeris core (vsop87 at millidegree parity with
DE440s; astro-rust for the Moon) on e-ink hardware. Three shapes, smallest
regulatory surface in the whole list:

- **Almanac.** Wall or desk e-ink panel: sunrise/sunset, moon phase, planet
  positions, offline forever, solar-friendly. Needs no radio at all.
- **Clock that surfaces readings.** The same face joins the household mesh as
  an ordinary Reticulum node and interleaves telemetry with the sky: tank
  level beside moonrise, greenhouse temperature beside civil twilight. E-ink's
  refresh model matches telemetry cadence (readings arrive in minutes, not
  frames). This is the domestic face of the telemetry family below.
- **Portable.** The T-Echo is already a supported board family (locked builds
  exist in the Prns lane) and carries e-ink, LoRa, and GPS on certified stock
  hardware. A pocket ephemeris-plus-mesh-pager personality is a
  boot-selectable channel on a board we already flash, which makes it the
  cheapest portable product in the list.

The compute is trivial at e-ink cadence; the almanac variant could ship as a
gift-shop object with zero mesh dependency, and the other two ride the
existing board families.

### 2. Store-and-forward voice drops (loved)

Audio letters carried by outrider propagation: record on one node, delivered
over minutes or hours as couriers and propagation nodes move the resource.
R4 resources and the large-propagation-response work are the carriage layer
and already have receipts.

Named honest gap: wavicle is the wrong codec here. LoRa bitrates want
something in the Codec2 class (700 bps to 3.2 kbps), which is new DSP work,
not a wavicle configuration. Over WiFi bearers the constraint relaxes.
Scoped same day in the
[lofi voice codec scoping note](2026-08-13_lofi_voice_codec_scoping.md):
ADPCM proves the pipeline on fat bearers, an owned LPC-10e implementation
from FIPS-137 is the recommended LoRa-tier codec (the `codec2` crate is
LGPL and barred from the workspace by deny.toml), and a Codec2-class coder
is a later optional rung.

### 3. Trail and wildlife sensors (loved)

Camera or PIR node in the field; over LoRa travels only the classification
("deer, 06:14"), never the image. First version does inference at the field
gateway (a V4 or small host running vates), which makes it a gateway feature
rather than a firmware program. burn-on-microcontroller is a real lift and
stays a later question; the gateway version needs nothing new from firmware
beyond the sensor personality of the telemetry family.

### 4. Field beacons, and knot as the universal container (loved, extended)

A solar node serving local content: trailhead conditions, farm stand hours,
community notice board. The session extended this well beyond the original
smolweb framing:

- **Button-to-read for any phone.** Press a physical button; the beacon wakes
  a WiFi access point with a captive portal for a bounded window and serves
  its content to whatever phone connects. No app, no account, no mesh client.
  This targets the ESP32 family (V4); the nRF boards have no WiFi.
- **What format comes out.** Smolweb formats and micron are already highly
  renderable, but the phone's browser doesn't speak them. The answer is the
  knot: the beacon's content is authored as a djot knot in which gemtext,
  gopher, and feed blocks embed in their idiomatic spec-faithful form
  (per the [polyglot knot design](../../genet/design_docs/nematic_docs/implementation_strategy/2026-05-08_polyglot_knot_design.md)),
  and the beacon serves a **projection** of it: plain HTML for Safari/Chrome,
  raw `text/vnd.knot` for a Mere-aware client that wants the real container.
  "Read gemtext even in Safari" is exactly the HTML projection of a knot
  whose gemtext block stays gemtext in the source.
- **Gap CLOSED same day:** `EngineDocument::to_html` landed 2026-08-13 in
  `genet/components/inker/src/document/render/html.rs` (body-fragment
  emitter beside `to_markdown`/`to_gemini`/`to_knot`/`to_gophermap`;
  escaped, semantic intent kept as classes, link predicates carried as
  `data-predicate`; 5 tests, inker 94 green). Whether the beacon renders
  on-device or stores a pre-projected HTML form beside the knot is an
  implementation choice; storing both is simpler for a microcontroller.
- **Boundary with the knot publishing protocol.** The
  [knot publishing plan](../../mere/design_docs/mere_docs/implementation_strategy/2026-08-07_knot_publishing_protocol_plan.md)
  Phase A is authenticated, private, ticketed reading between Mere instances,
  and it deliberately scoped out anonymous public hosting. The beacon is not
  Phase B and must not be confused with it: it is anonymous local-radius
  serving of explicitly authored public content over plain HTTP on the
  device's own access point. Different trust model, different transport, no
  admission stack. The shared piece is the knot format itself.

### 5. Telemetry devices, in and out of the house (loved: "a lot of takes possible")

The broad family, and the quiet flagship for Merely's first market (rural
Kentucky homestead: soil moisture, water tank, gate, greenhouse, livestock
waterer). Takes enumerated in session:

- **Bearer per zone.** LoRa for the field, WiFi/TCP indoors (which lifts the
  bandwidth ceiling entirely and is the anti-Tuya story: flash it yourself,
  it answers only to your roster), BLE for wearable or proximity range under
  the [wall-node management plan](2026-08-30_wall_node_management_plan.md).
  Reticulum's bearer-agnosticism means one identity and one
  telemetry grammar across all three.
- **Telemetry is small.** A few bytes per reading sits comfortably inside
  LoRa duty cycle and the bounded-state discipline AIR3 just proved on the
  T114 (18,168-byte heap peak).
- **A sensor personality** is one more boot-selected channel in the existing
  one-image model; it rides DIST6 and the civic-deployment measurement
  machinery rather than opening a new program.
- **Head-end:** Turnstone renders the household as the graph it is; the
  e-ink clock (concept 1) is the ambient face of the same data.
- **Stock-hardware answer for probes:** RAK WisBlock. Certified modular
  sensor hardware on the RAK4631, the same nRF52840 family as the T114, so
  the board work substantially transfers and Merely still manufactures
  nothing.

## Sequencing

Nothing here opens before the ARDC application ships. Natural first artifacts
afterward, in rough order of leverage:

1. **Sensor-personality design doc** consuming DIST6 and civic measurement
   machinery (unlocks concepts 5, 3, and the clock face of 1).
2. **T-Echo ephemeris personality note** (concept 1 portable; cheapest
   product, boards in hand).
3. ~~`to_html` emitter~~ **landed 2026-08-13** (inker `render/html.rs`);
   concept 4's remaining work is the beacon itself.
4. **Codec work** for voice drops, per the
   [scoping note](2026-08-13_lofi_voice_codec_scoping.md) (written same day;
   Rung 0 ADPCM can start any time).

Concepts 1 through 5 all strengthen the ARDC application as written intent
without creating engineering gates.
