# Sennet and Tucket current-reference software work

**September 29, 2026. Baseline:** Retinue `10a1711` on clean `main`.
**Verdict:** bounded protocol repairs pass software gates. Fresh stock-radio
qualification remains open. No serial port was opened, firmware flashed, radio
setting changed, or deployment credential provisioned in this work.

The [wire plan](../../../design_docs/2026-09-27_wire_compatibility_plan.md)
owns the scope and remaining acceptance. [reference.json](reference.json)
records four SHA-256-identified MIT MeshCore comparison inputs at official
companion/repeater 1.17.1 revision
`d92964352441e53b93e8667b802e04f6e072b39e`. These are source/document comparisons,
not an executed independent peer. Sennet's targets are Meshtastic
`2.7.26.54e0d8d` beta and, separately, `2.8.0.47db0e3` alpha. Sennet inputs
remain public prose and black-box observations; no Meshtastic implementation,
schema or client source was used. Historical captures retain unknown producer
versions and do not acquire a current pin from this work.

## Repairs

- Tucket appends, compares and consumes the entire requested one-, two- or
  three-byte public-key prefix. Intermediate source-route hops forward without
  delivering the payload locally. Wrong-hop input does not poison later dedup.
  `Node::set_flood_hash_size` configures newly originated floods; payload identity
  hashes remain one byte with existing collision refusal. The public forwarding
  helper now accepts `&Identity` rather than one byte.
- TRACE dedup hashes the reference's one-byte path metadata. A project-selected
  input is checked against independent .NET and Python SHA-256 calculations,
  including the distinct digest of the old extra-zero-byte bug.
- The unscoped Tucket Node refuses unsupported payload versions and transport
  scopes before contact/dedup mutation. The codec remains able to preserve those
  fields. This establishes refusal, not region-scope support.
- Sennet's broadcast flood engine refuses directed traffic before changing its
  seen table. Its retained text leaf accepts broadcasts or its own full NodeID,
  validates application bytes before dedup insertion, and observes headers without
  constructing discarded relay output. Channel-encrypted directed reception does
  not establish PKI, directed routing or delivery-ACK parity.
- Sennet checks generic port-varint width before allocating outbound envelopes.
  Text retains a 232-byte body; a five-byte selector allows 228 bytes within the
  same 237-byte transport payload.
- MeshCore device identification distinguishes serial API version from firmware
  release and excludes the BLE PIN. The harness's `--probe` queries identity
  without changing settings; exchanges refuse a mismatched release before
  configuration. Default expected release is 1.17.1, configurable through
  `MESHCORE_EXPECTED_VERSION`. The existing exchange still needs settings
  preservation/restoration before physical use.
- Sennet config capture can attach caller-supplied firmware identification and
  a receipt hash. Unknown producers remain unknown. A 15-second/64-KiB limit
  bounds intake; empty or limit-stopped attempts preserve output and fail.

## Results and failed attempts

[Initial gates](gates/result.json) preserve the successful protocol tests,
embedded checks, formatting and registry checks, plus three dependency-cache
failures. The missing locked `serial2`/`aes` dependencies were acquired and those
gates rerun in [dependency repair](dependency-repair/result.json).
[Capture validation](capture-validation/result.json) records simulated CLI and
registry checks. Each result identifies source bytes, command, exit status,
elapsed time and its log digest; these are working-tree observations, not
clean-commit release certificates.

| Gate | Result |
| --- | --- |
| Sennet/Tucket host tests | 149 passed: 72 Sennet, 77 Tucket |
| Hardware-feature/all-target check | Passed |
| Strict Clippy, including hardware examples/tests | Passed |
| `thumbv7em-none-eabihf` protocol libraries, no default features | Passed |
| Downstream radio-hand library/integration tests | 235 passed |
| Simulated Sennet capture CLI | Five cases passed; no serial port opened or capture file written |
| Formatting and validation inventory | Passed |

The new regressions include complete wider-hop matching, count/byte limits,
malformed public Packet fields, two modeled repeaters, future-version/scope
refusal without dedup poisoning, all port-varint widths, foreign destination
refusal and malformed-application recovery. A modeled repeater is our code,
not a stock MeshCore peer. Replayed historical RF fixtures retain their original
evidence boundary. Whole firmware-image, allocator-peak, on-air scheduling and
fault acceptance are not established by library target checks.

## Reproduction and remaining gates

Run `run.py --output <unused evidence directory>` with the ordinary approved
`C:/t/cargo-targets/retinue`. The default runner uses locked/offline dependencies;
`--allow-downloads` permits acquiring missing locked packages. `--gates` reruns
only named failures while preserving earlier output. Existing evidence is never
overwritten.

Fresh Sennet beta and alpha stock peers need separate identified captures and
bidirectional text/boundary/restart receipts. Tucket needs identified 1.17.1
companions/repeaters for advert, flood, learned route, text, ACK, retry, fallback,
multi-byte and multi-repeater acceptance. Scope policy and broader reference
roles remain separate implementation/acceptance work. Preserve settings and
identities before physical changes. RNS remains pinned to published 1.5.4.

The shared default package-cache lock was held by an unrelated Mere fetch. The
existing marker-identified `C:/t/cargo-homes/retinue-wire-compat` was reused for
this gate, seeded with 205 checksum-verified cached registry archives and
read-only access to immutable cached git objects. Missing gate dependencies were
downloaded into that home. The stable ordinary Retinue target is retained;
there is no separate target or worktree. Cleanup is recorded in [cleanup.log](cleanup.log).
Automatic approval review rejected the checked deletion command with only
"blocked by policy" as its reason. A separate live-owner recheck found no matching
gate process. The temporary home remains owned by this completed software
qualification; deletion was not bypassed.
