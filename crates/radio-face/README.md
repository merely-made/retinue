# radio-face

`radio-face` is the protocol-neutral, `no_std` UI core for Retinue's radio
firmware. It owns bounded display facts, PANEL×LEDGER rendering, input state,
LED intent, and the optional host-snapshot payload.

It deliberately does not own display pins, radio drivers, Retinue protocol
objects, or host transport. Board firmware supplies local facts. A host may
publish a lossy snapshot of facts it owns.

Generate the off-target visual receipts with:

```text
cargo run -p radio-face --example render_receipts
```

PNG output goes to `target/radio-face-receipts` unless a different directory is
passed as the first argument.
