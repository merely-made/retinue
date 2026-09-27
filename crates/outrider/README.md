# outrider

LXMF as a boundary crate in the retinue family: message codec, delivery state
machines, and a propagation client/server over
[Reticulum](https://reticulum.network/), riding on
[retinue](https://github.com/merely-made/retinue)'s destinations, links, and
resources. An outrider rides ahead of the party to scout and carry word.

Not affiliated with or endorsed by the Reticulum or LXMF projects.

**Status: codec, opportunistic/direct delivery, and captured propagation lane implemented.**
The [LXMF 1.1.1 / RNS 1.5.4 receipt](../../testing/receipts/rns-1.5.4-repin/README.md)
qualifies seven local direct, opportunistic and propagation gates, including stock
clients and Outrider's bounded server. Committed byte fixtures retain their original
producer versions. Older physical receipts cover direct and opportunistic delivery
over V4-to-T114 direct-PHY RF and Resource-backed propagation over that protected
pair; they were not rerun as part of the 1.5.4 qualification. The
founding scope, provenance discipline, and ordered gates are recorded in
[`design_docs/2026-07-25_outrider_lxmf_founding.md`](../../design_docs/2026-07-25_outrider_lxmf_founding.md).

## Scope (v1)

- The LXMF message codec, with unrecognized fields carried opaque and
  round-tripped intact.
- Direct delivery over a retinue link, including resource-backed large
  messages and carrier-specific Resource transfer policy.
- Opportunistic single-packet delivery over Retinue's current/retained
  ratchets. The packet destination elided by stock LXMF is restored only at
  this boundary; internal messages remain ordinary signed LXMF objects.
  Retinue applies each selected interface's complete-frame cap before issuing
  Outrider a queue receipt.
- Propagation client: submit to and fetch from propagation nodes.
- A bounded propagation server: accept, store, expire, deliver to owner, with
  stamp verification.

The bounded server is currently in-memory and defaults to one message of at
most 240 encrypted bytes per fetch. Callers can raise those bounds; large
responses then use a request-bound Resource, proven with a 4 KiB message
against stock clients in both directions. The store emits versioned snapshots
for a host to persist in its chosen file or database; restore re-derives
transient ids and byte counts and reapplies current capacity, expiry,
duplicate, and owner-scoping rules. Inter-node propagation sync parity, ticket
ecosystems, the wider fields ecosystem, paper messages, and
conversation/contact semantics remain open.

## Provenance

Outrider is implemented from the public LXMF specification prose, the
Reticulum manual, and black-box captures of pinned stock clients. The Python
LXMF implementation and its client applications are not implementation inputs. See
[`PROVENANCE.md`](PROVENANCE.md), which keeps pace with the code.

## License

Mozilla Public License 2.0, like the rest of the retinue workspace; see the
[workspace README](../../README.md) for what that means in practice.
