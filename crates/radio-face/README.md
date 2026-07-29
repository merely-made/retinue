# radio-face

`radio-face` is the protocol-neutral, `no_std` UI core for Retinue's radio
firmware. It owns bounded display facts, PANEL×LEDGER rendering, input state,
LED intent, and the optional host-snapshot payload.

It deliberately does not own display pins, radio drivers, Retinue protocol
objects, or host transport. Board firmware supplies local facts. A host may
publish a lossy snapshot of facts it owns.

The snapshot wire format is versioned, allocation-free, capped at 160 bytes,
and valid for at most 300 seconds. Firmware expires it relative to receipt time
so disconnected host state becomes unavailable rather than persistent truth.

Generate the off-target visual receipts with:

```text
cargo run -p radio-face --example render_receipts
```

PNG output goes to `target/radio-face-receipts` unless a different directory is
passed as the first argument.
