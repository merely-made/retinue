# outrider

LXMF as a boundary crate in the retinue family: message codec, delivery state
machines, and a propagation client/server over
[Reticulum](https://reticulum.network/), riding on
[retinue](https://github.com/mark-ik/retinue)'s destinations, links, and
resources. An outrider rides ahead of the party to scout and carry word.

Not affiliated with or endorsed by the Reticulum or LXMF projects.

**Status: founded, no wire code yet.** The founding scope, provenance
discipline, and ordered gates are recorded in
[`design_docs/2026-07-25_outrider_lxmf_founding.md`](../../design_docs/2026-07-25_outrider_lxmf_founding.md).
The first gate is a black-box capture oracle against a pinned stock client;
nothing is implemented ahead of it.

## Scope (v1)

- The LXMF message codec, with unrecognized fields carried opaque and
  round-tripped intact.
- Direct delivery over a retinue link, including resource-backed large
  messages.
- Opportunistic single-packet delivery, once retinue's outbound ratchet
  encryption lands.
- Propagation client: submit to and fetch from propagation nodes.
- A bounded propagation server: accept, store, expire, deliver to owner, with
  stamp verification.

Out until demand is real: inter-node propagation sync parity, stamp
generation ecosystems, the wider fields ecosystem, paper messages, and any
conversation or contact semantics.

## Provenance

Outrider is implemented from the public LXMF specification prose, the
Reticulum manual, and black-box captures of pinned stock clients. The Python
LXMF implementation and its client applications are never read. See
[`PROVENANCE.md`](PROVENANCE.md), which keeps pace with the code.

## License

Mozilla Public License 2.0, like the rest of the retinue workspace; see the
[workspace README](../../README.md) for what that means in practice.
