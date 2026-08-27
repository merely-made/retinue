# Unsafe policy receipt

The board RF ingest route is `Packet::decode` followed by
`retinue::node::Node::ingest`. It contains no first-party `unsafe` token. The
only approved unsafe surface is the board-runtime machinery listed in
`unsafe-policy.toml`:

| File | Tokens | Reachability | Reason |
| --- | ---: | --- | --- |
| `firmware/t114-phy/src/heap.rs` | 9 | normal startup / every allocation | installs the fixed allocator once before allocation; the TrackingHeap high-water wrapper forwards GlobalAlloc alloc/dealloc to the wrapped LlffHeap |
| `firmware/t114-phy/src/crash.rs` | 9 | fault handling / reset inspection | preserves the crash record in its reserved linker section |
| `firmware/t114-phy/src/main.rs` | 1 | normal startup | invokes the checked allocator handoff |
| `firmware/heltec-v4-phy/src/wake_input.rs` | 2 | low-power RX | accesses only the GPIO14 PAC bits owned by the wake adapter |

All other first-party crate roots forbid unsafe code. The two firmware crates
deny unsafe operations inside unsafe functions, and every exception has a
review date and exact lexical count. The audit deliberately scans tracked
first-party source only: generated build output and third-party vendor code are
not silently treated as Retinue's own unsafe surface.

Run it with:

```sh
python3 validation/security/unsafe_audit.py
```

The fuzz harness is the separate scheduled receipt:

```sh
python3 validation/run_fuzz.py --seconds 900
```

It copies the immutable seed files into a temporary writable corpus before
calling `cargo fuzz run retinue-node-ingest`. A passing local smoke or policy
audit is not a sustained fuzz receipt; that needs `cargo-fuzz` and its recorded
duration on a clean exact commit.
