# Sennet and Tucket current-reference work

**September 29, 2026. Baseline:** Retinue `10a1711` on clean `main`.
**September 29 verdict:** bounded protocol repairs pass software gates. No serial
port was opened, firmware flashed or radio setting changed during that software
pass. The September 30 physical results below have their own evidence boundary.

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

Sennet alpha needs separate identified captures and bidirectional text/boundary/
restart receipts. Beta's outstanding gates and measured Tucket 1.17.1 scopes are
listed below. Scope policy, multiple repeaters and broader reference roles remain
separate work. Preserve settings and identities before physical changes. RNS
remains pinned to published 1.5.4.

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

## September 30 physical qualification

Baseline `e898d54` on clean main. Three connected boards were independently
identified before writes: V4 COM6 / MAC ending FB:28, V4 COM7 / FA:64, and T114
COM10. Existing owner selections from July 28 and August 20 identify both V4
carriers as revision 4.2. Firmware model labels are recorded separately. Full
16 MiB private V4 backups preceded foreign writes. Stock images were installed
through Linkboy's hash-checked three-part packages; a transfer receipt alone is
not a protocol exchange receipt. [physical](physical/) retains the observations.

| Comparison | Measured scope |
| --- | --- |
| MeshCore companion `v1.17.1-d929643`, API 13 | Signed adverts, flood/private text, encrypted ACK, learned reciprocal routes, three failed direct sends then fourth-attempt flood recovery; outbound widths 1, 2 and 3 |
| MeshCore repeater `v1.17.1-d929643` | Forced reciprocal one-hop text/ACK routes with prefixes `bc`, `bcb6`, `bcb6a6`; both endpoints must cross the stock repeater |
| Meshtastic `2.7.26.54e0d8d`, CLIENT, HELTEC_V4 | Sennet text accepted once in its client interface and rebroadcast with matching source, packet ID and exact text; two identical repeats suppressed in an eight-second observation window |
| Independent Retinue V4/T114 | Encrypted bidirectional text; durable IDs advanced by one across host process restarts |
| Text boundary | 233 bytes refused before opening a radio; current stock maximum-size acceptance remains pending |

The current Meshtastic release was queried through an untouched CLI 2.7.11
process. Only official binary firmware was extracted; no Meshtastic schema,
firmware or client implementation source was read. New hex fixtures replay
today’s raw RF/client/node-info observations, with their exact producer pin.
Historical July fixtures retain their original unknown producer. Public
node-info identifies `!f66afa64` as `Sennet Current Stock` / `SC26`; a relay's
name does not replace the original sender's identity.

[Final software gates](physical/gates/result.json) passed 151 protocol tests,
strict Clippy across both hardware examples, both embedded protocol checks,
formatting, registry verification and six captured-byte admission controls.
Those controls reject a wrong source, text or packet ID and truncated input;
they never open a serial port. The earlier five-case ad hoc self-check remains
historical. [manifest.json](physical/manifest.json) hashes public evidence,
working-tree source, executed examples and the unpublished recovery backups.

The first Tucket exchange exposed refusal of the official hexadecimal build
suffix. The repair accepts that suffix for a base-release pin while preserving
exact full-build pins and rejecting other releases, malformed suffixes and
empty pins. Earlier endpoint/repeater runs retain their executed binary hash;
the final tightened guards have a separate passing width-three endpoint run.
Sennet receipt admission now requires the exact source, packet ID and text,
preventing unrelated traffic from producing a successful transmit receipt.

Failures are retained. Two Sennet attempts received no rebroadcast while stock
region readback was UNSET. Keeping the CLI connected for five seconds after a
separate region setter persisted US; the succeeding readback precedes the
successful RF exchange. COM7 then disappeared from USB, and the maximum-size
attempt failed before opening the absent port. Reverse/current maximum-size
checks and original-image restoration await physical reconnection.

COM6's compressed full-image Linkboy restore timed out in `FlashDeflData` and
remains recovery-required in that transaction receipt. Separate expert recovery
wrote the exact original 16 MiB image uncompressed with esptool 5.3.1, verified
the full write, then independently compared the complete private settings tail
at `0x3f0000..0x400000`. Its original slot B / sequence 9 returned. T114 was
never reflashed and retains slot A / sequence 84, US915/modem and LongFast.
COM7's verified original backup remains ready for the same uncompressed
restoration. Private images, settings fragments and the stock CLI environment
are excluded from public receipts.

Remaining gates: COM7 restoration, beta reverse/current maximum-size RF,
separately pinned alpha, multiple stock repeaters, region scopes, PKI/directed
routing and broader reference roles. The ordinary build target remains
`C:/t/cargo-targets/retinue`; private recovery evidence remains under ignored
`validation/results/sennet-tucket-radios/backups`. The earlier policy-blocked
Cargo home is retained as described above. No worktree or isolated target exists.
