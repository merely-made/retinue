# NomadNet resource interoperability receipts

**Latest finding:** the Retinue-to-rns-net 0.7.0 Resource-response stall was
resolved by float64 RTT activation encoding. See the final section. Earlier
matrices preserve the pre-fix results; stock NomadNet client defects and the old
server API limitations remain separate.

## Scope

This is a local-loopback receipt for Retinue's response-Resource sender. It covers the
published `gmlewis/go-nomadnet` v0.119.0 `view-mu` client (go-reticulum 0.100.0), built and
used as an opaque black-box, plus stock NomadNet 1.4.2 and stock RNS 1.5.3. No GPL/AGPL
implementation source was read.

## Change

`ResourceSender` now bzip2-compresses the resource body when and only when the encoded body
is smaller. The random-hash prefix remains outside the compressed body, matching Retinue's
existing resource recovery path. The advertisement sets `FLAG_COMPRESSED` only in that case;
incompressible payloads remain plain.

## Checks

On a Retinue loopback node, `view-mu -raw` retrieved each 131,072-byte compressible response
byte-for-byte after this change:

| page | SHA-256 |
| --- | --- |
| `large.mu` | `6d2791ca11bd20a70fd325ace9c33870dd8cbb09efe4dd9ccdad1478468f93fd` |
| `repetitive.mu` | `68d73b6658fc997659bbf18bb6d619dc756448b8eea739b30288f8e9f3c21174` |
| `varied.mu` | `fcb6499836d7c92decea09c2029a140ffac86a0395f66e0487cd2043f2a64cf1` |

Stock Python RNS 1.5.3 also retrieved the Retinue `repetitive.mu` and `varied.mu` bodies at
the same hashes. Focused Rust checks passed:

```text
cargo test --locked -p retinue sender_ -- --nocapture
cargo test --locked -p retinue loopback_adapter_serves_pages_and_refuses_unregistered_requests -- --nocapture
cargo check --locked -p retinue --no-default-features
```

The deterministic 131,072-byte `SHA256(counter)` body remained uncompressed
(`FLAG_COMPRESSED == 0`). Stock Python fetched it from both Retinue and NomadNet 1.4.2 at
`848a79e87da1f440d49039094ff05c5ddf28c5609b88129c5f3a9780ec145faf`; the Go client produced
zero bytes against both senders. This receipt therefore qualifies compression-compatible
Micron response resources only. It does not claim Go interoperability for plain multipart
resources.

Task-local command logs and the minimal black-box setup are retained at
`C:\t\micron-go-interop-20260912`.

## Rust peer qualification: nomadnet-rs 0.3.1

The additional Rust candidate was tested on 2026-09-13. Its published package is
MIT licensed, but its resolved `rns-core 0.1.9`, `rns-crypto 0.1.5` and
`rns-net 0.5.6` dependencies use the custom Reticulum License. Those dependencies
were treated as black boxes; only their license text was inspected. No runtime
dependency was added and no Retinue implementation changed for this qualification.

Native Windows compilation failed with 87 errors involving Unix/serial APIs.
Unmodified `nomadnet-serve` and separate public-API harnesses built on WSL Ubuntu
with Rust 1.97.1. The harness used actual announces, loopback TCP and bounded
timeouts. Retinue was pinned to `2c38da3b611777da8d7e3c74e636c6bc34054b96`.
The downloaded crate SHA-256 was
`f8afb390bb052508fed38ca1cb113ba783cf07d83ad92a3723d15ec11fcfe522`.

| Direction | Small, 14 bytes | Compressible, 131,072 bytes | Incompressible, 131,072 bytes |
| --- | --- | --- | --- |
| Rust browser API fetching its own raw PageCache server | No PageReceived before deadline | No PageReceived before deadline | No PageReceived before deadline |
| Retinue fetching Rust raw PageCache server | Exact match | Response receive timeout | Response receive timeout |
| Rust browser API fetching Retinue | No PageReceived before deadline | No PageReceived before deadline | No PageReceived before deadline |
| Retinue fetching stock nomadnet-serve | Exact match | Response receive timeout | Not tested |

The small fixture hash was
`43f5fa9ef511939985588b51995258a37a545a50205c2875d42e1078302db811`;
the large and incompressible hashes are those listed above. There were two exact
matches in eleven cases. Failed cases produced no page output. The stock CLI
performs lossy UTF-8 conversion and `$SELF` substitution, so the opaque binary
fixture was tested only through a separate raw PageCache harness.

The first client harness blocked when dispatching inside the link callback.
The final harness dispatches from an application worker thread. It receives a
small RESPONSE callback but still emits no PageReceived in self-control. In the
MIT package's `src/browser.rs`, pending requests use a generated ID (lines
952-967), dispatch does not pass that ID to `send_request` (690-692, 969-971),
and response handling removes by the returned ID (715-730). This appears to
explain a request-correlation failure. It is not a diagnosis of the black-box
RNS implementation. The failed self-control prevents attributing the forward
failures to Retinue or qualifying this browser as a reference peer.

The candidate remains experimental. Requalification requires a passing client
self-control, then byte-exact small and multipart transfers in both directions.
Windows support needs a separate build receipt. These are loopback results,
not radio, GUI or Micron rendering acceptance.

The final matrix and per-case logs are retained at
`C:\t\nomadnet-rust-interop-20260913\run-20260913-122704\receipt.json`.
The parent directory contains the harness sources, Cargo locks and build logs.
All test child processes were stopped after the run.

### Native Fedora host comparison

The same eleven cases were repeated on `thinkpad-l14-f` through its configured
SSH agent on 2026-09-13. The host reported Linux `7.2.4-200.fc44.x86_64` and
glibc 2.43. The four existing Linux executables were copied without rebuilding;
their SHA-256 values were verified before execution. Only the runner's root and
binary paths changed. Every case reproduced the WSL outcome above: two exact
small-page matches and nine failures. The small self-control again received a
16-byte RESPONSE callback without a PageReceived event. Thus the observed
failures are not confined to WSL; this does not yet identify the multipart cause.

The native-host receipt is
`thinkpad-l14-f:/home/markik/nomadnet-peer-host-check-20260913/run-20260913-124901/receipt.json`,
also copied with its logs to `C:\t\nomadnet-rust-interop-20260913\run-20260913-124901`.
This is native Fedora execution of the same binaries, not a Fedora source-build
or cross-host transport receipt. All peer connections remained loopback.

### Client diagnosis and directional packet traces

A separate copy of the MIT `nomadnet-rs` browser was instrumented on 2026-09-13.
The unchanged baseline logged different pending and returned request IDs and
delivered no page. An environment-gated diagnostic then matched the sole pending
request regardless of ID. That produced a PageReceived event from both the raw
Rust server and Retinue, but its content was 16 bytes rather than 14: the prefix
`c4 0e` was a MessagePack binary wrapper. A second diagnostic decoded that value
before browser delivery. Together these changes recovered exact page bytes.

| Diagnostic client against | Page | WSL | Native ThinkPad Fedora |
| --- | --- | --- | --- |
| Unmodified Rust raw PageCache server | Small, 14 bytes | Exact | Exact |
| Unmodified Retinue probe | Small, 14 bytes | Exact | Exact |
| Unmodified Retinue probe | Compressible, 131,072 bytes | Exact | Exact |
| Unmodified Retinue probe | Incompressible, 131,072 bytes | Exact | Exact |

This confirms request-correlation and response-decoding defects in the tested
browser integration. It also establishes a narrower positive result: Retinue
multipart sending interoperates with this diagnostically corrected Rust client.
It does not qualify the published client unchanged. The single-pending fallback
is unsafe as a general repair: production correlation must handle concurrent
requests, timeouts and stale responses using the actual wire request identity.
The two diagnostics remain outside production code and runtime dependencies.

A transparent loopback TCP relay recorded HDLC frame lengths, direction, packet
type and context using Retinue's packet definitions, without decrypting content
or reading the black-box RNS implementation. When Retinue requested either large
page from the unmodified Rust raw server, the trace contained the request but
neither a RESPONSE nor RESOURCE_ADV before timeout. The reverse failure thus
occurs before resource delivery, not during multipart reassembly. Its internal
cause remains unproven. In the successful Retinue-to-client direction, the
incompressible trace contained RESOURCE_ADV, 22 RESOURCE_REQ frames, 283 resource
data frames, three hashmap updates and a resource proof. The harness's rejection
of generic unsolicited resources did not block these response resources.

Artifacts under `C:\t\nomadnet-rust-interop-20260913`:

- `diagnostic-20260913-130534`: baseline, ID-only experiment and reverse traces.
- `diagnostic-20260913-130757`: complete four-case WSL decoded-client run.
- `diagnostic-20260913-130840`: complete four-case native Fedora run and traces,
  copied from the ThinkPad's `nomadnet-peer-host-check-20260913` directory.
- `browser-diagnostic.patch`, `client-experiment`, `nomadnet-rs-experiment` and
  `diagnose*.py`: isolated source changes and replay harnesses.

The final diagnostic client SHA-256 was
`daf5cc3b170ccc0889a04ef1563f0af505d125044492665ecdbf40688abd7e1a` on both hosts.
Both builds used the existing lockfile offline. The original peer source and
Retinue implementation were preserved; all test child processes were stopped.

### Cross-implementation evidence and server API comparison

The [FreeTAKTeam LXMF-rs v0.10.1 report](https://github.com/FreeTAKTeam/LXMF-rs/releases/download/v0.10.1/v0.10.1-independent.json)
pins its external rns-rs peer to `6c6d79b83516feff271d15c97d39dd1de7798afe`
(`rns-net 0.7.0`, `rns-core 0.1.16`), rather than NomadNet's `0.5.6` dependency
set. Its compressed response case checks a full 1 MiB body and matching request
identity. These remain upstream measurements; we did not rerun its full suite.

The report's public-API control adapter explicitly registers
`register_request_handler_response` and returns `RequestResponse::Resource`.
NomadNet uses `register_request_handler` returning bytes. A compile probe confirmed
that the explicit response method/type is absent from the old dependency API.
Only the MIT NomadNet code and the EPL-2.0 test adapter's public-API setup were
examined; rns-rs implementation code remained black-box. No upstream code was
imported into Retinue.

Minimal external servers compared these choices on native ThinkPad Fedora:

| Server setup | Retinue small page | Retinue compressible 128 KiB | Retinue incompressible 128 KiB |
| --- | --- | --- | --- |
| Old dependencies, plain byte handler | Exact | Timeout | Timeout |
| Pinned 0.7.0 dependencies, plain byte handler | Exact | Timeout | Timeout |
| Pinned 0.7.0 dependencies, explicit Resource handler | Timeout | Timeout | Timeout |

All handlers logged that they returned the expected byte count. Failed Resource
cases emitted no RESOURCE_ADV in the loopback trace. Upgrading alone therefore
does not resolve the original failure, and explicit Resource selection alone
does not resolve the Retinue-facing case.

Crucially, the same newer Resource server passed all three pages byte-for-byte
with the earlier diagnostically corrected Rust client. This is a positive
external-peer control, not a stock NomadNet client receipt. Two isolated Retinue
request variations, current wall-clock timestamp and then an explicit 4 MiB
response-size field, still timed out for all three pages. Neither experiment
was promoted into production.

The upstream report separately records failure to activate an LXMF-rs-initiated
link on rns-rs, while Python RNS 1.5.2 activates the same link. That is a specific
lead for the remaining discrepancy: link initiation/activation must be tested
separately from which peer sends the application request. It is not yet proven
to be the cause of Retinue's failure. The next narrow gate is a link-activation
comparison against stock Python, with initiator direction recorded explicitly.

Artifacts under `C:\t\nomadnet-rust-interop-20260913`:

- `upstream-v0.10.1-independent.json` and `upstream-control-adapter.rs`: reference
  report and external adapter inspected for the API comparison.
- `version-old`, `version-probe`: minimal public-API servers and locked manifests.
  Final Linux builds passed with `--locked --offline`; no rns-rs patch was made.
- `diagnostic-20260913-132928`: nine-case server API/version comparison.
- `diagnostic-20260913-133303`: three-case corrected Rust client positive control.
- `diagnostic-20260913-133439` and `diagnostic-20260913-133551`: timestamp and
  timestamp-plus-cap experiments, respectively. All four directories were copied
  from the ThinkPad after execution.

Executed server SHA-256 values: old
`978659ee075fa8f49e717951293e6462121c67a978aa6a264e301ec1cff8827f`;
new `e5d0dcdab69ef5fedc1e14f89020f998ffc4fc62c9228f576b24c83e234ce886`.
All task-owned peer processes were stopped. No hardware or public-network claim
is made by these loopback tests.

### Confirmed activation mismatch and Retinue fix

The isolated activation comparison changed only the RTT packet's MessagePack
encoding from float32 (`0xca`) to float64 (`0xcb`), preserving the public `f32`
API and its numeric value. The newer rns-rs server's public callback showed no
link activation for the original client, although its page handler ran. With
float64, LINK_ACTIVE preceded the handler and all Resource responses completed.
This confirms the activation boundary for the tested Retinue/rns-net 0.7.0 pair;
it does not diagnose every upstream link failure.

| Native Fedora comparison | Small | Compressible 128 KiB | Incompressible 128 KiB |
| --- | --- | --- | --- |
| Original Retinue → rns-net 0.7.0 explicit Resource server | Timeout, no LINK_ACTIVE | Prior timeout | Prior timeout |
| Float64 RTT Retinue → same server | Exact, LINK_ACTIVE | Exact, LINK_ACTIVE | Exact, LINK_ACTIVE |
| Float64 RTT Retinue → stock Python RNS 1.5.3 page server | Exact | Exact | Exact |

The fix is applied in `Link::rtt_packet`. Float32 is valid MessagePack, but the
tested external Rust peer requires float64 to activate this link. The regression
test decrypts an RTT packet established from the existing Python proof fixture
and checks the independently specified nine-byte float64 encoding of 0.125.
Request timestamps, response-size fields and production peer dependencies are
unchanged. The old NomadNet server still needs a Resource-capable response API;
its client correlation/decoding defects are not repaired by this RTT change.

Native logs and byte receipts are retained under
`C:\t\nomadnet-rust-interop-20260913\diagnostic-20260913-134934` (activation
comparison) and `diagnostic-20260913-135037` (Python reference), copied from the
ThinkPad. The probe was built from Retinue `2c38da3` with only the RTT encoder
change; its SHA-256 is
`d69ced48869a6a68f917f8dd60a5ba446cc0514fed173a81cca299a9a5f99fc8`.
The callback-instrumented external server SHA-256 is
`74a238457cb7a05c0bab37d10efc8fb97d16005121e996a7cf4c27c183b4e0a8`.

The current production source passed all nine focused tests: three in
`link_session` and six in `endpoint_resource`. They ran via the isolated
`main-validation/Cargo.toml`, which points at the actual repository library and
test files, with copies of the existing fixtures. The root workspace's offline
resolution required an uncached, unrelated Signalman/Mere dependency; no workspace
dependency or lockfile was changed to work around that. Logs are in
`main-validation.log`. All task-owned peer processes were stopped after testing.
