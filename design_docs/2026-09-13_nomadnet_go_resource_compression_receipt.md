# NomadNet Go resource compression receipt

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
