# Signalman as a management surface

**Date:** 2026-08-15
**Status:** Direction, ruled with Mark. Signalman grows from Linkboy's
graphical face into a radio management surface: its own device-data mere,
a geographic scene, a logical network scene, a messaging surface, and a
content pane. No gates are opened here; sequencing is at the end.

**Implementation:** [staged plan and ownership lanes](2026-08-15_signalman_management_surface_implementation_plan.md).

## The ruling

**Signalman-desktop integrates its own device-data mere.** The same pattern
as woodshed's stage and isometry's overmap: a mere is the unit an
application integrates, and the radio dataspace is signalman's. Turnstone
links into that mere later as a guest, through graphshell's remote-lens
seam, and the DIST6 boundary survives intact: signalman is the radio
authority, turnstone is a consumer, and neither the routing nor the
flashing authority moves. The graphshell remote-lens plan already lists
"the radio companion app" as a consumer with existing pull; this is that
consumer taking shape.

Why now: the transit link receipt wants an operator at a second site, and a
bare cargo example is a poor operator. Signalman makes the second site
humane. It does not gate the receipt, which runs on examples per its spec.

## Surface: Network (the logical scene)

A force-directed schematic of what the radio actually knows, in the orrery
register. Everything it renders already exists as state:

- retinue: the path table, announce history with hop counts, active links;
- outrider: propagation-node peering;
- the listener-executive: the current lease, which is precisely the thing
  an operator cannot see today.

Roles fall out of state rather than declaration: endpoint, transport relay,
propagation node, peer, known-but-stale. Edges are typed the same way:
heard-announce with hops, live link, path-via-transport, peering.

The snapshot model lives in **postilion, as a module first** per the
module/crate discipline: it is host-tier data shared beneath signalman, and
a second consumer (turnstone's guest view) is what would justify a crate.
Node identity and selection come from NODE_SHEET; canvas navigation follows
the standing defaults. The scene renders real state with invariants
asserted, never a redrawn cache: a placebo network view is worse than none.

## Surface: Map (the geographic scene)

Positions come from three honest sources, in order of arrival:

1. **Owner-pinned locations.** A repeater on a ridge is a fact the owner
   types in once; gaz records carry it as a facet.
2. **LXMF telemetry.** FIELD_TELEMETRY is field 2 in the captured registry,
   but its interior format is Sideband's and publicly undocumented, so it
   needs a capture against the pinned oracle before parsing, exactly as the
   audio field did.
3. **On-board GPS.** The T-Echo has it; it is the natural first moving dot.

The **Atlas arrangement is mere-side work**: reserved as P5 in the
projection table, zero stack hits so far, and signalman becomes the first
consumer that satisfies its consumer-pull gate. V1 stays offline-first on
principle, because a radio tool that needs a tile server is lying about its
context: plain projected plane, range rings, an optional imported basemap
later. The civic deployment plan names an "atlas" in phase two; this is its
owner-scoped cousin, and the vocabulary is deliberately aligned.

## Surface: Messages

Outrider's boundary doc assigns conversation, contact, and storage
semantics to the consumer. Signalman becomes that consumer:

- contacts through personae and gaz;
- history through the codicil log;
- direct and propagation delivery, with real status per the no-placebo
  rule;
- **voice drops through pipit**, field 7, AM_CUSTOM: signalman is the first
  shipping consumer of the codec, and a memo recorded at site A playing at
  site B through the relay is the human shape of the transit test.

## Surface: Content (the pelt pane)

Mark's addition, and the cheapest of the four to compose: signalman-desktop
already sits on genet, and both pelt (`ports/pelt`) and nematic now live in
that same workspace. A pelt pane rendering nematic's engines gives signalman
a reader for content served over the mesh.

Two lanes, one existing and one new:

- **Smolweb.** Nematic's sixteen engines cover gemtext, gopher, nex,
  spartan and the rest today. Transport is the smolweb-over-Reticulum plan
  (scoped, not started, R-A gates all fetching), but the bytes are already
  proven: the `gemini_over_reticulum` example serves and fetches a capsule
  over a real retinue link with no IP anywhere.
- **Micron.** No micron engine exists in nematic. It is a new engine,
  implemented from public prose and captures per the household discipline,
  since NomadNet is GPL and stays unread. It belongs in nematic rather than
  app-local, per propagate-capability-up-the-stack, and it pays twice:
  knots gain micron blocks through the polyglot fence expansion, and the
  HTML projection carries micron content to stock browsers, which extends
  the field-beacon story.

## Costs, named plainly

- **Pin lockstep.** signalman-desktop pins an exact genet rev; mere tracks
  the same family. Integrating mere means the two pins move together, which
  is the known patch-table trap. This is the largest mechanical cost.
- **Atlas arrangement** is new mere work, owned there, pulled by signalman.
- **Telemetry capture** is oracle work before any position parsing.
- **Micron engine** is genet-side nematic work, owned there.
- UI copy stays plain: the panes are Map, Network, Messages, and a browse
  pane; orrery and Atlas remain code-register words.

## Sequencing

Nothing here gates the transit receipt. A workable order once work opens:

1. Network scene on postilion's snapshot module, since it needs nothing
   from outside the retinue family and pays off at the bench immediately.
2. Messages with text first, then voice drops.
3. The device-data mere integration proper, which items 1 and 2 can
   precede as panes and then move into.
4. Map, behind the telemetry capture and the Atlas arrangement.
5. Content pane, behind smolweb-over-Reticulum R-A for fetching; a micron
   engine can land in nematic independently at any time.
