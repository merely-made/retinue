# Outrider provenance

Outrider is an independent implementation of interoperability with LXMF, the
message format and delivery system of the Reticulum ecosystem. Its
auditability depends on keeping observation, implementation, and distribution
boundaries explicit. This record keeps pace with the code: every wire fact
the crate encodes names its source class here.

## Source boundary

Implementation facts come from three places only:

1. The public LXMF specification prose and diagrams, and the Reticulum
   manual's public documentation.
2. The public-domain Reticulum protocol specification, as already implemented
   and oracle-verified in the sibling `retinue` crate.
3. Direct black-box observation of bytes emitted or accepted by pinned stock
   LXMF clients and propagation nodes, run and observed, never read.

The excluded inputs are the same as the household's other clean-room crate
(`crates/sennet/PROVENANCE.md`): third-party protocol implementation source,
client applications, generated bindings, and implementation-derived API
references. The Python LXMF implementation and its client applications carry
post-2025 license clauses and are not consulted as implementation input under
any circumstances; they serve strictly as external oracles.

## Capture discipline

Each capture records the stock software and pinned version, the transport,
the direction, the input, the raw bytes, and the acceptance result. Captures
are committed as fixtures so CI replays them with no Python. Release notes
pin the stock baseline being matched.

## Record

No wire facts are encoded yet. The first entry lands with the capture oracle
(gate 1 of the founding doc).
