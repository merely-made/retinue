# Signalman S2 live station receipt

**Date:** 2026-08-20
**Status:** the live bench leg of S2 is complete. The lease-gated live path
carried one real over-the-air announce and its derived route into the shipped
Network projection. The headed interactive judgement (pan, zoom, drag, focus,
and selection in a real window) remains an owner action, like the
screen-reader pass, because the desktop deliberately has no synthetic-input
self-drive.

## What ran

- **Mere `1609cb90`** — `SitedStation::management_snapshot()`, the read-only,
  lease-checked getter the S2 receipt boundary was waiting on. It performs the
  same fail-closed authorization recheck as every other public station
  operation and returns owned snapshot data, never a handle onto the private
  station. The port's Retinue pin moved to `655336d`, the first revision
  carrying `postilion::management`.
- **Retinue `1971598`** — the desktop's live station actor
  (`apps/signalman-desktop/src/station.rs`): one worker thread owns a
  current-thread Tokio runtime and the `SitedStation`; it self-issues and
  renews the host grant from a wallet under the station data root, polls the
  snapshot getter, skips unchanged generations, and hands owned events to the
  UI thread. Projection happens on the UI side under the owner's stale policy
  through the one `apply_management_material` door. All eight desktop Mere
  pins moved together to `1609cb90` (63 commits over the prior pin,
  including Seiche on wgpu 30 and the sceno/scenomise arrangement renames);
  the full exact-pin desktop suite stayed green.
- **The gated live test** `apps/signalman-desktop/tests/live_station.rs` —
  inert unless `SIGNALMAN_STATION_PORT` and `SIGNALMAN_PEER_PORT` name
  attached boards. The station side runs the production `StationWorker` and
  `DesktopState` route exactly as the binary does. The peer side is a plain
  Postilion station with a short-lived random identity: real radio, real
  announce, no wallet claim.

## Bench facts

Windows host, two Heltec V4 boards running Retinue `tulle/heltec-v4` 0.0.1 on
the direct-PHY boot channel, `sync=2b`, `longfast=906875000`:

- station: `COM6`, `identity=loaded slot=B seq=9`;
- announcing peer: `COM7`, `identity=loaded slot=B seq=5`, in-test random
  station identity, announce interval 5 s.

Observed sequence, printed by the test run:

```text
connected: S2 bench station on COM6
snapshot gen ManagementGeneration { endpoint: 1, observations: 0, route_expirations: 0 }:
  0 routes, 0 links, 0 current announces, 0 history
snapshot gen ManagementGeneration { endpoint: 3, observations: 2, route_expirations: 0 }:
  1 routes, 0 links, 1 current announces, 2 history
live receipt: station on COM6, 2 nodes, relations
  ["signalman:heard-announce", "signalman:route-via"]
```

The projection reached two nodes — the station and the announced peer — with
one heard-announce and one route-via relation. Per S2's boundary, this is a
one-hop observation receipt; it makes no multi-hop, delivery, or link claim
(`links` stayed 0).

## Reliability observation

A serial session that ends without a clean channel stop leaves the V4 unable
to answer the next `Station::open`, which then reports "the radio did not
come online in time" (and a concurrently held port reports access denied).
The bench recovery is a run-mode reset: pulse RTS (EN) with DTR inactive.
Pulsing DTR and RTS together instead selects the ESP ROM download mode. This
session's failures came from a leftover S5 bench `signalman-desktop.exe`
holding the boards and from the boards' post-drop channel state; both cleared
with the reset. This is a reliability note beside the known repeated-CDC
T114 defect, not a closed defect.

## What stays open

- The headed interactive leg of S2: settle, pan, zoom, drag, focus, and
  selection in a real window, owner-driven.
- G5 hide/reopen and the S5 headed two-site audible voice receipts.
- Live-station UI activation policy: the bench activation is
  `SIGNALMAN_STATION_PORT` (with `SIGNALMAN_STATION_DATA` and
  `SIGNALMAN_STATION_NAME`), mirroring the other runtime-shaping variables.
  An owner-facing connect flow is product work the management plan has not
  gated yet.
