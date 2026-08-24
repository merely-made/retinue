# Re-pin receipt: RNS 1.4.2 → 1.5.0, LXMF 0.9.6 → 1.1.1

**Date:** 2026-08-23
**Pins moved:** `rns==1.4.2` → `rns==1.5.0`; `lxmf==0.9.6` → `lxmf==1.1.1`
**Verdict:** both landed. One outrider defect found and fixed; no upstream regression found.

---

## What RNS 1.5.0 is, and how that was established without a changelog

RNS 1.5.0 was published to PyPI on 2026-08-22. **No release notes exist anywhere public.** The
sdist ships no changelog, the wheel ships no changelog, and the 1.5.0 manual's reStructuredText
sources are byte-identical to 1.4.2's — the docs site was rebuilt with a bumped version string
and no content change. The GitHub repository is a declared public mirror (discussion #1069,
2025-12) frozen at 1.4.2, with no 1.5.0 tag or release. Upstream's own channel for notes is
`rngit` over Reticulum, which was not used here.

What follows was therefore reconstructed from evidence, inside the oracle discipline: package
metadata, file hashes, and **public constants and API signatures read at runtime**, which
`oracle/README.md` explicitly permits. No RNS or LXMF source was read.

**Package shape.** 126 files in both 1.4.2 and 1.5.0 — no module added or removed, identical
console entry points. 79 files byte-identical, 32 changed. The change is concentrated far from
the wire:

| file | 1.4.2 | 1.5.0 | delta |
| --- | ---: | ---: | ---: |
| `RNS/Transport.py` | 226639 | 247416 | +20777 |
| `RNS/Reticulum.py` | 98048 | 107582 | +9534 |
| `RNS/Utilities/rnstatus.py` | 41288 | 50435 | +9147 |
| `RNS/Interfaces/Interface.py` | 16029 | 18219 | +2190 |
| `RNS/Packet.py` | 25076 | 25647 | +571 |
| `RNS/Link.py` | 76015 | 76260 | +245 |
| `RNS/Identity.py` | 43523 | 43636 | +113 |
| `RNS/Destination.py` | 32203 | 32200 | −3 |
| `RNS/Resource.py` | 63568 | 63342 | −226 |

**Runtime surface diff** (both interpreters pinned to Python 3.14.2 after a 3.12/3.14 mismatch
was caught producing spurious signature diffs): +114 symbols, −7, 35 changed.

**It is an ingress-scheduling and congestion-control release.**

- Traffic classes: `TC_DATA=0`, `TC_ANNOUNCE=1`, `TC_PATH_REQUEST=2`, `TC_INGRESS_LIMITED=3`.
- A new `InboundQueues` type with bounded per-class depths — data 4096, announce 256,
  path-request 256, ingress-limited 128 — plus `USE_INBOUND_QUEUE=True`,
  `USE_OUTBOUND_QUEUE=False`, and new `inbound_job` / `outbound_job` pumps.
- The ingress path split: `Transport.preprocess_inbound(raw, interface, tc, ifac_handled)` now
  fronts `Transport.inbound(raw, interface=None, tc=None, ifac_handled=False)`.
- Path-request suppression tightened: `PATH_REQUEST_GATE_TIMEOUT` 120 → **45 s**, `max_pr_tags`
  32000 → **16000**, `path_request(..., ingress_limited=False)`,
  `outgoing_pr_frequency(preemptive=False)`, `discovery_pr_tags` removed.
- **Per-interface misbehaviour accounting:** new `Interface.protocol_violation(description)`,
  `Interface.ifac_violation(description)`, `Interface.packet_filter_hit()`, and
  `Identity.validate_announce(..., signal_blackholed=False)`.
- Byte accounting throughout (`received_announce(size=0, …)`, new `announce_rxb/txb`,
  `pr_rxb/txb`, speed and frequency counters — the `rnstatus` growth).
- `Interface.DEFAULT_IFAC_SIZE=16` named; `IC_BURST_MIN_SAMPLES=6` → `EC_BURST_MIN_SAMPLES=2`;
  `StreamDataMessage.HEADER_LEN=2` named; `Discovery` gained `OP_ADDR=240` with onion and
  invalid-IP filtering.

**No header, flag, or size constant changed anywhere.**

The misbehaviour accounting deserves attention beyond this re-pin. Until the 2026-08-06 fix,
retinue emitted roughly half its IFAC frames with the wire flag clear and 1.4.2 tolerated it
silently. Under 1.5.0 that class of deviation feeds a counter wired to blackholing, so any
residual protocol deviation stops being invisible and becomes attributable by peers.

## RNS 1.5.0: evidence

- **The live gates pass, and the suite flakes at a rate that predates this re-pin.** A clean run
  is twelve of twelve — open and IFAC-authenticated announce, path resolution, links in both
  roles, request/response, endpoint streaming, Resources in both directions including the 2.5 MB
  segmented cases, and transport routing. Repeated runs are not reliably clean, on either pin.
  See "The suite flakes, on both pins" below before quoting a gate count.
- **Wire unmoved.** The fixture corpus was captured twice under 1.4.2 to establish which files
  are deterministic (20 of 31; the 11 announce and token fixtures vary run-to-run at a *fixed*
  version and were excluded, so randomness could not masquerade as a wire change), then once
  under 1.5.0. **Eighteen of eighteen deterministic wire files are byte-identical**, including
  `ifac_packet.bin`, `tcp_frame_announce.bin`, `tcp_stream.bin`, `link_proof.bin`,
  `ratchet_packet.json` and `path_request.json`. The two files that differ are
  `ifac_packet.json` and `manifest.json`, and in both the sole difference is the recorded
  `rns_version` string. The IFAC wire has now held across 1.3.8 → 1.4.2 → 1.5.0.
- **494 host-crate tests pass** across retinue, outrider, tulle, selvage, sennet, tucket and postilion.

Note: `cargo test --workspace` fails on a pre-existing `critical-section` feature-unification
clash between the firmware and host crates. It predates this work and is unrelated; the host
crates must be named explicitly.

## LXMF 1.1.1: a real wire change, and an outrider defect

LXMF 1.1.1 changed **zero** existing constants or signatures. It added 42 symbols and removed 2
(one callback split into `delivery_resource_transfer_began` and
`propagation_resource_transfer_began`). Five new message fields — `FIELD_REPLY_TO=48`,
`FIELD_REPLY_QUOTE=49`, `FIELD_REACTION=64`, `FIELD_COMMENT=65`, `FIELD_CONTINUATION=66` — all
land in previously empty registry space; re-running `oracle/capture_fields.py` at the new pin
took the registry from 20 to 25 fields with nothing removed, nothing renumbered, and audio still
field 7.

**But the delivery announce grew a third element**, and outrider refused it.

```text
LXMF 0.9.6:  92 c40e "Stock Receiver" 08          # 2-element
LXMF 1.1.1:  93 c40e "Stock Receiver" 08 9100     # 3-element, trailing [0]
```

`announce.rs` required `parts.len() == 2` and returned `InvalidShape` for anything else, so
**every stock LXMF 1.1.1 delivery announce was rejected**. Outrider never learned any sender's
keys, and `interop_direct_receive` and `interop_opportunistic_receive` both failed. Outrider's
own two-element announce was still accepted by stock, so the break was receive-side only.

**What the third element means, established black-box.** LXMF's *public* accessor
`compression_support_from_app_data` answers it directly, and `SF_COMPRESSION = 0`: the array is a
supported-features list, and membership of `0` declares compression support.

| features | `compression_support_from_app_data` |
| --- | --- |
| `[0]` | **True** |
| `[]` | False |
| `[1]` | False |
| `[0,1]` | **True** |
| `[2,3]` | False |
| absent (2-element) | **True** |
| `nil` | **True** |

Two consequences. First, stock parses a **four**-element announce without complaint, so upstream
is itself forward-tolerant and matching that tolerance costs nothing. Second, and worse: the
default is permissive in the dangerous direction. **Outrider implements no compression, yet its
two-element announce had always been read by stock as compression-capable** — a misdeclaration
that predates 1.1.1 and that 1.1.1 merely exposed.

**The fix**, in `crates/outrider/src/announce.rs`:

- **Decode** accepts `parts.len() >= 2` and ignores anything past the stamp cost. The peer's
  feature list is deliberately not retained, because outrider never compresses and so has no
  decision to make with it.
- **Encode** emits three elements with an **empty** feature list, `[name, cost, []]` — the only
  shape that truthfully declares no compression. Emitting stock's `[0]` would claim a capability
  outrider lacks; emitting the old two-element form claims it implicitly.

Six tests replace the two that asserted byte-exact re-encode of the 0.9.6 form: legacy
two-element still decodes, both captured 1.1.1 forms decode, a four-element future announce is
accepted, the empty-feature-list emission is pinned byte-exact, and our own announce round-trips.

**All six outrider gates pass** at the new pin, in both directions and end to end.

## The suite flakes, on both pins

**The live gate suites carry pre-existing intermittency of roughly 9% per gate. This is not a
regression, and it is not new — it had simply never been measured.** A single suite run is a
poor instrument, and every historical "twelve of twelve" in this repository, including the one
originally written into this receipt, is a snapshot of a flaky suite rather than a stable result.

Measured by alternating the **full** `run_live.py` suite between the pins, three rounds each:

| round | old pin (1.4.2 / 0.9.6) | new pin (1.5.0 / 1.1.1) |
| --- | --- | --- |
| 1 | FAIL `interop_reqresp` | PASS (12 gates) |
| 2 | PASS (12 gates) | FAIL `interop_resource_recv` |
| 3 | FAIL `interop_ifac` | FAIL `interop_reqresp` |

**One clean suite in three on each pin — identical.** The failing gate varies run to run, and
`interop_ifac` failed on the **old** pin, which nothing in this work touches. If P(clean suite)
is about 1/3 over twelve gates, per-gate failure is around 9%; at that rate a suite run is closer
to a coin flip than a verdict.

Every affected gate passes reliably in isolation: `interop_reqresp` 5/5, `interop_opportunistic_receive`
32/32 and roughly 53 consecutive. Three distinct gates were seen to flake —
`interop_opportunistic_receive`, `interop_reqresp`, `interop_propagation_receive` — plus
`interop_ifac` and `interop_resource_recv` in the alternating runs.

**Per-gate rates vary widely, and the 9% above is an average inferred from suite-level results,
not a uniform figure.** Measured directly: `interop_reqresp` fails **4 of 30 standalone runs**
(13%), while `interop_opportunistic_receive` went 32/32 interleaved and roughly 53 consecutive.
A five-run sample proves nothing at these rates and should not be quoted as evidence.

**One mechanism is identified, in `interop_reqresp`, and it is a teardown race in the example
rather than a protocol or version problem.** Paired pass/fail logs show it directly: in passing
runs RNS logs receipt of the direction-2 response *after* retinue's socket has already closed; in
failing runs it reports `None` while retinue's own log already says `ANSWERED_REQUEST` and
`d2=true`. The library is not at fault -- `TcpInterface::send_raw` does `write_all` then `flush`,
so the bytes reach the kernel before `send` returns -- but `examples/reqresp_interop.rs` exited
the instant both done-conditions were met, dropping the interface underneath a peer that had not
read yet. A 250 ms grace was added before return, the bookend to the 250 ms wait the example
already took after `accept` for the documented RNS connect race.

**That fix did not measurably reduce the failure rate**: 4 in 30 before, 4 in 60 after, which is
indistinguishable at these sample sizes (Fisher's exact p is about 0.44). What changed is the
*mode*. The teardown signature stopped appearing in captured failures; the residue is two other
modes the grace cannot touch -- `d2=false`, where retinue breaks out of its receive loop before
RNS's request arrives at all, and a collapse of the whole exchange in which direction 1 fails
too. Both remain unexplained.

**Two further hypotheses were tested and failed.** Raising RNS's log level to 7 made a failure
vanish (6/6), suggesting a timing race; and forcing a rebuild before each run, to reproduce the
edit-then-test loop the first failures appeared in, did **not** reproduce them (5/5). A sequence
effect was also ruled out: `interop_reqresp` flakes at the same rate run alone as inside the
suite, so the suite's roughly-one-failure-per-run is arithmetic over twelve gates, not
interference between them.

**A methodological warning, recorded because it nearly cost a wrong conclusion here.** An earlier
2×2 across `{RNS 1.4.2, 1.5.0} × {LXMF 0.9.6, 1.1.1}` was run as four sequential *blocks* and
appeared to separate cleanly — both 1.5.0 cells at 8/10, both 1.4.2 cells at 10/10 — which reads
as a 20% regression in RNS 1.5.0. It is an artifact: cell identity was confounded with position in
a long batch. Re-run **interleaved**, four cells per round with the leading cell rotated, the same
comparison gives 8/8 in every cell — 32/32, no version effect at all. Against a background of
independent per-run flakiness, blocked experiments manufacture differences. Interleave.

## Fixture corpus

Per the naming decision taken with this re-pin: **the current pin's vectors carry no version in
the filename; superseded captures keep theirs.** The corpus accumulates rather than being
overwritten, and every file records its versions internally.

| file | status |
| --- | --- |
| `lxmf_fields.json` | **re-captured** at LXMF 1.1.1 / RNS 1.5.0 by `capture_fields.py` |
| `lxmf_direct.json` | announce vectors **re-captured** at the new pin; message vectors carried forward |
| `lxmf_message.json` | carried forward |
| `lxmf_opportunistic.json` | carried forward |
| `lxmf_propagation.json` | carried forward |
| `lxmf_0_9_6_*.json` | retained unchanged as the superseded captures |

`capture_fields.py` now writes the unversioned name and records `LXMF.__version__` /
`RNS.__version__` read at runtime, rather than hardcoded strings that drift.

**Honest limits on the carried-forward files.** Only `capture_fields.py` produces a fixture
programmatically; the other four were hand-assembled from capture output, so they cannot be
mechanically re-captured. Each was checked for version sensitivity:

- `lxmf_direct.json` **was** sensitive — it pins `sender_announce_app_data` and
  `receiver_announce_app_data`, which moved from two- to three-element. Both were genuinely
  re-captured by driving stock LXMF's public API at the new pin. Its message vectors are carried
  forward and the file says so.
- `lxmf_propagation.json` is **not** sensitive: its pinned propagation-node announce was
  re-checked against 1.1.1's `pn_announce_data_is_valid`, `pn_name_from_app_data` and
  `pn_stamp_cost_from_app_data`, returning `True` / `Stock Propagation Oracle` / `13`, identical
  to 0.9.6.
- `lxmf_message.json` and `lxmf_opportunistic.json` carry no announce data.

The carried-forward vectors replay green against 1.1.1 but **are not re-observations at this
pin**, and each file's `provenance` block states that. A full hand re-capture of those three
remains open work.

## Open

- **A full re-capture** of the three carried-forward fixtures at the current pin.
- **The live-suite flake**, largely unexplained and present on both pins. `interop_reqresp` is
  measured at 13% standalone and two of its three failure modes have no identified cause;
  `interop_resource_recv` and `interop_ifac` were seen failing during the alternating runs and
  have no baseline of their own yet. This wants a dedicated lane with sample sizes in the
  hundreds -- 30-run blocks cannot separate a 13% rate from a 7% one. Until then, a passing
  suite run is weak evidence and a failing one is ambiguous.
- **What `LXMF.PN_META_VERSION = 0` gates.** Unused here; the propagation-node announce it
  accompanies parses identically across both LXMF versions.
- **Whether stock compresses toward a peer that declares support.**
  `LXMessage.determine_compression_support` exists and is per-recipient, but an isolated probe
  never reached the router's peer table (returned `None`, identical packed sizes), so this was
  not settled. It is moot for outrider, which now declares no support — but it matters if
  outrider ever implements compression.
