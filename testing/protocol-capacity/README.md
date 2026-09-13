# Protocol capacity probe

This unpublished validation crate compiles Retinue, Sennet and Tucket together
with `no_std + alloc`. It is not a protocol switching runtime.

The fixture retains one Retinue Node (8 peers, 4 actions, 1 link, 4 routes,
2 freshness blobs per peer, 255-byte ingress, 32-byte local announce data,
160-byte link payload, 1,024-byte outbound resource and 8 inbound parts), one
Sennet channel/packet allocator (8 directory rows, 32 dedup identities), and one
Tucket Node (8 contacts, 32 dedup hashes, 2 pending texts of at most 171 bytes).
Capacities are explicit fixture selections, not imposed firmware settings.

It fills directories, churns duplicate history, encodes/decodes Sennet texts,
retains Tucket routes and attempted texts, establishes a Retinue link, receives
a 1 KiB resource and leaves an outbound resource pending. Synthetic peer objects
are included in the allocation peak. Fixture identities and repeated keys must
never be used in deployed firmware.

```powershell
cargo run -p protocol-capacity-probe --features host --locked --offline
cargo +esp rustc -p protocol-capacity-probe --lib --release --locked --offline --target xtensa-esp32s3-none-elf '-Zbuild-std=core,alloc' -- --emit=llvm-ir
cargo +esp build -p tulle-heltec-v4-phy --example protocol_capacity --release --locked --offline --target xtensa-esp32s3-none-elf '-Zbuild-std=core,alloc'
```

The host executable reports **requested allocation bytes** through a System
allocator wrapper. It excludes allocator metadata, size-class rounding and
stack. Its peak is one exercised workload, not a worst-case theorem or an
Xtensa measurement. All workload allocations must be released on drop.

The target LLVM symbol `PROTOCOL_LAYOUT_V1` lists pointer size, Retinue Node,
Actions and InterruptionReport, Sennet Channel, PacketIdState, ManagedFlood,
NodeDirectory, Tucket Node, PendingText, the complete Residents structure, and
PendingTexts, in that order. These sizes include inline state, not heap storage.

The V4 example is **build-only**, with a 64 KiB LLFF heap, 64 KiB radio/runtime
BSS reservation and 16 KiB queue/state BSS reservation. Its main links the
workload but validation does not flash or execute it. Reservations demonstrate
linker space for a candidate budget; actual runtime, allocator fragmentation,
target heap/stack high-water and adverse traffic remain for board integration.
The shipping direct-PHY firmware remains a separate image.

Use the protocol-owned checked input paths and retain queues within their
declared bounds. General low-level allocation helpers and caller-owned vectors
do not acquire a global budget merely by linking this crate. Compression is off
in the target graph; incoming decompression needs a separate expansion limit.

The adjacent `receipt.json` records the exercised source hashes, host results,
target layouts, linked sections and scope. It is working-tree evidence, not a
clean-revision release receipt.
