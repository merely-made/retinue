# Re-pin receipt: RNS 1.5.0 to 1.5.2, LXMF held at 1.1.1

**Date:** 2026-08-29 local / 2026-08-30 UTC
**Pins:** `rns==1.5.2`; `lxmf==1.1.1`
**Baseline Retinue revision:** `af4b858099ead8d5034236cbc7017362ccfcdc24`
**Verdict:** the current pin is compatible within the measured local-TCP, wire,
Resource, route-freshness, LXMF, and pinned-Prns scopes. The re-pin needs oracle and
documentation changes, including two route-probe timing corrections. It needs no Retinue
or Outrider protocol-code change.

## Evidence boundary

RNS and LXMF remain black-box reference implementations. This work installed published
wheels, invoked public APIs and command-line surfaces, and inspected runtime output, wire
bytes, package metadata, and official release material. It did not read or unpack Python
implementation source. The Reticulum License therefore remains confined to the oracle
dependency and is not an implementation input to Retinue's MPL-2.0 code.

The exact RNS 1.5.2 wheel used here is 631,363 bytes with SHA-256
`56da2ec26f103a074ad546edee2b2eeb949b24f1f74399f294e08027e238a471`.
The value agrees between [PyPI's 1.5.2 metadata](https://pypi.org/project/rns/1.5.2/)
and the asset attached to the [official 1.5.2 release](https://github.com/markqvist/Reticulum/releases/tag/1.5.2).
The oracle virtualenv reported `RNS 1.5.2` and `LXMF 1.1.1` before the live gates.

## Upstream delta

This re-pin crosses two releases. The [RNS 1.5.1 release](https://github.com/markqvist/Reticulum/releases/tag/1.5.1)
introduced adaptive ingress and egress control, HDLC and IFAC processing changes, earlier
invalid-frame rejection, and transport implementation name/version data in interface
discovery. The [RNS 1.5.2 release](https://github.com/markqvist/Reticulum/releases/tag/1.5.2)
is a maintenance release over that scheduler work. Its notes identify a Resource regression
introduced in 1.5.1, an I2P keepalive bug, dataplane parameter tuning, and an updated `rngit`
configuration example.

The current [ingress-queue manual](https://markqvist.github.io/Reticulum/manual/interfaces.html#tuning-ingress-queues)
documents strict class priority and defaults of 1024 data, 128 announce, 128 path-request,
and 8 ingress-limited packets. The RNS 1.5.0 documentation used 4096, 256, 256, and 128.
That is an operational change. Existing Retinue gates exercise ordinary traffic under
bounded local load; they do not saturate those queues. This receipt therefore establishes
compatibility at tested load, not queue-pressure parity.

## Stock-RNS and Resource gates

A wheel-only RNS 1.5.2 environment ran the complete stock-RNS oracle suite once. All 12
listed gates passed, including IFAC, both link roles, request/response, endpoint streaming,
Resources, 120 KB transfer, 2.5 MB multi-segment transfer, and transport behavior. One
suite pass is useful integration evidence, but the live-gate flake rate is not bounded, so
it does not carry a deterministic claim by itself.

The 1.5.2 notes specifically name a Resource regression in 1.5.1, so the four
Resource-sensitive gates were then interleaved for three rounds:

| gate | result |
| --- | ---: |
| stock sends Resource to Retinue | 3/3 |
| Retinue sends Resource to stock | 3/3 |
| Retinue sends 120 KB Resource | 3/3 |
| Retinue sends 2.5 MB multi-segment Resource | 3/3 |

All 12 targeted executions passed. Every 2.5 MB direction completed three segments and
proof verification. The only diagnostics were expected TCP close/reconnect messages during
teardown.

## Fixture comparison

The earlier 1.5.0 re-pin reported 18/18 deterministic fixtures byte-identical. That broad
form is not repeated here because several current drivers contain fresh identities,
ephemeral link material, ciphertext, request tags, or physical-RNode input.

The deterministic `ifac_packet.bin` capture was byte-identical between fresh 1.5.0 and
1.5.2 environments. After recursively normalising `rns_version` and `rnid_version`, the
following JSON captures differed only in that version metadata: `buffer_wire.json`,
`channel_link.json`, `channel_wire.json`, `ifac_packet.json`, `link_identify.json`,
`link_session.json`, `manifest.json`, and `rns_signed_artifact.json`.

The entropy-bearing semantic checks retained their observed shape. The ratchet case kept a
115-byte packet, 96-byte token, destination and ratchet identifiers, flags, and the same
HKDF/HMAC/AES facts. The path request kept a 51-byte frame, flags `0x08`, packet type 0,
destination type 2, context `0x00`, and the same 16-byte target; its fresh request tag
changed. RNode serial captures were not rerun because they require attached hardware.
These observations support the live compatibility conclusion, but they are not a
corpus-wide byte-identity receipt.

## Announce timebase and route freshness

The persistent announce-timebase probe reproduced all three current decisions under RNS
1.5.2. P1 rejected equal-timebase replacement. P2 accepted the `2^39` high-water announce
and rejected a following value of `2`. P3 accepted `1` followed by `2^39`. The ignored
receipt is
`validation/results/announce-timebase-20260830T024551Z/result.json`, SHA-256
`252c29b40dc4b972f1615935e434a8a1802cccf6f2db6ec641f4bae851b5fef0`.

The first RNS 1.5.2 route run stopped before issuing any path request because it observed
two seeded path-response candidates in pre-request captures. That run carries no route
decision. Inspection found a probe defect: the old settle delay elapsed while terminal
transports had no downstream TCP peer, so queued egress could not drain. One candidate also
crossed into a different hop-relation arm that the old same-arm check did not inspect.

The corrected probe now connects a passive downstream client to each terminal, retains its
bytes, waits for an observed quiet window, and checks every arm for every seeded candidate.
It also publishes receiver announce events atomically and waits until all stage-one
incumbents have been accepted before shutdown. The final connected-drain run captured two
old ordinary announces, 371 bytes total, before the three terminals became quiet. That is
setup evidence explaining the first failure, not changed RNS admission behavior.

The corrected full profile passed 72/72 cells. Every cell had a public-signature-valid
forwarded Type-2 frame and a calibrated hop relation, with zero conflicting-frame cells.
Real public `request_path` calls produced the path-response arm. The ignored receipt is
`validation/results/route-freshness-full-20260830T030952Z/result.json`, SHA-256
`14601b688fe72e1763e8d022915c468d1f9b164715bc47aee726050b905aaf39`.

The packet-loop isolation profile also passed 6/6 ordinary, equal-timebase,
exact-same-blob rows. Its original loop window held all six incumbent packet hashes. The pre-candidate window held one
reload-generated hash and none of those six. Every row remained a valid no-admission result
with no route transition. Its ignored receipt is
`validation/results/route-freshness-same-blob-diagnostic-20260830T030802Z/result.json`,
SHA-256 `d660ea18f6ce38d0029672d85829a257d25f53dd30ae2df9cec63fe2f6972550`.

After the failure-path review, the first five-cell smoke rerun marked one row invalid
because its ordinary candidate never produced a forwarded frame. Its destination table
still showed the expected no-admission result, but the missing delivery proof makes the row
unmeasured. A clean rerun passed 5/5. This is recorded as live-probe delivery variance, not
as a route decision or as a bounded flake rate. The failed and passing ignored receipts are
`route-freshness-smoke-20260830T032159Z` (SHA-256
`366d5fb3f631fecf1aa04297535ea44ea7189d98a6d6bdabb459361baaa9d844`) and
`route-freshness-smoke-20260830T032340Z` (SHA-256
`05c78e1ebb44e38caac3c0d5ded0924b5b6b9330f2e76d08fff63b56e24ae465`).

## LXMF and Outrider

LXMF remains pinned at 1.1.1. Seven current Outrider gates passed under RNS 1.5.2 and LXMF
1.1.1: direct receive, direct send, opportunistic receive, opportunistic send, propagation
receive, propagation server, and propagation through the stock peer. The propagation driver
comments now name the pinned stock LXMF instead of retaining the obsolete 0.9.6 label. This
is a documentation correction; no Outrider codec or state-machine change was needed.

## Pinned-Prns H8 matrix

H8 was rerun against a clean detached Prns worktree at exact revision
`72b6b30d27cac910ce20d370e1dc711fe9b95955`. `prnsd 0.3.4` was built from inside that
worktree so its 256 MiB Windows stack configuration applied. The resulting daemon SHA-256
was `a3b33143895a1dd979680114e11ee5aa19e2174eda839bb9a9fbe81e416a3495`.

Three clean matrix runs each passed all five cases: RNS to Retinue, Prns to Retinue, Prns
to RNS, RNS hop behavior, and Prns hop behavior. The ignored receipts are:

| run | cases | `matrix.json` SHA-256 |
| --- | ---: | --- |
| `peer-rns152-20260829-run1` | 5/5 | `e14683af0ff222807f0891e690c05a9bc1d2b5d497590e59c55cee0fe524b14f` |
| `peer-rns152-20260829-run2` | 5/5 | `b9c590252a898faff871e017222e0d9fe3bea59c44ea7f618f0f375bbdf462` |
| `peer-rns152-20260829-run3` | 5/5 | `5592bca0302d98dbfddaebf9f36de3526f5bf8cec8e86b16e04ef2091beb2f79` |

That is 15/15 case executions. H8 remains closed for its declared local-TCP scope. It does
not close RF forwarding or newer-Prns adoption.

## Repository gates and remaining boundary

The focused Rust gates passed 201 default Retinue library tests, 168
`--no-default-features` Retinue library tests, and 49 Outrider tests. The validation
inventory passed with 20 manifests, 76 assets, and 14 suites. The broader
`cargo test -p retinue -j1` command still has the pre-existing
`announce_ingress_burst_is_bounded_attributed_and_does_not_silence_a_neighbor` failure at
`crates/retinue/tests/endpoint_ingress.rs:344`; the library gates are green and this re-pin
does not change that test's Rust path.

Done for this re-pin means the exact wheel is pinned, current public release facts are
recorded, Resource and ordinary live interop are green, Outrider is green, P1/P2/P3 and P8
are revalidated, H8 is re-receipted, and the fixture claim is narrowed to what was actually
reproduced. The remaining limits are explicit: queue saturation, I2P keepalive behavior,
interface-discovery metadata, public-network operation, physical RNode captures, RF
forwarding, and natural elapsed route expiry were not measured here.
