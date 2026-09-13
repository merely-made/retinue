# NomadNet resource interoperability receipts

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
