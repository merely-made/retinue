# Prns peer matrix receipt

**Date:** 2026-08-11 local / 2026-08-12 UTC  
**Status:** H8 Peer lane complete for the local TCP receipt

This receipt records the live three-corner interoperability matrix required by
the Peer lane. It does not import Prns into Retinue, modify Prns, or claim RF
behavior.

## Peer boundary

The peer was a clean detached Prns worktree at
`72b6b30d27cac910ce20d370e1dc711fe9b95955`, built as `prnsd 0.3.4`. The
executed daemon SHA-256 was
`5ef0cfbcc20bb0cdac6d523e2a0f9485b252dca2640b4dc32125770afc12a953`.
Retinue stayed a separate process and stock RNS was `1.4.2` from the existing
black-box oracle environment.

The driver is
[`peer_matrix.py`](../crates/retinue/oracle/peer_matrix.py), SHA-256
`7433a46fd3380e29907d00f6b5e30c34209223603c9fd4b48a81465b0c88b499` at
execution. It rejects a dirty peer tree and records the peer revision, daemon
digest, commands, temporary ports, source status, and raw stream digests in
the result manifest.

## Matrix result

`validation/results/peer-20260812T035508Z/matrix.json` passed. Its SHA-256 is
`e46981aee1917714aa7a44cea154fd3b559950a4a223b122e43786733a1142e3`.
That directory is intentionally ignored: the captured bytes are local evidence
and include transient identities and ports.

| Case | Result | Receipt |
| --- | --- | --- |
| Retinue ↔ stock RNS | Pass | Both announce validations passed; 188 bytes Retinue→RNS and 386 bytes RNS→Retinue were captured. |
| Retinue ↔ pinned Prns | Pass | Prns learned Retinue's destination through its path table; `prnsd nnpages announce` produced an announce Retinue validated. Captures: 188 bytes Retinue→Prns, 202 bytes Prns→Retinue. |
| pinned Prns ↔ stock RNS | Pass | Stock RNS validated Prns's `nomadnetwork.node` announcement; Prns learned the stock destination through its path table. Captures: 382 bytes RNS→Prns, 202 bytes Prns→RNS. |
| stock RNS transport O-10 | Pass | Two Retinue leaves observed one another as type-2 transport announces at `hops=1`. |
| Prns transport O-10 | Pass | The same two-leaf capture observed type-2 transport announces at `hops=1`. |

## O-10 disposition

There is no local TCP discrepancy: a source announce at wire hop 0 appeared
after one transport forward as a type-2 packet with wire hop 1 in both stock
RNS and Prns. The receipt does not promote that to a physical/on-air result;
the radio lanes remain responsible for an RF forwarding receipt.

## Scope left closed and open

H8's software receipt is closed. The explicit lane boundary remains:

- Prns-derived ports need only donor-conformance evidence in their affected
  seams. The untouched detached process remains an independent regression peer.
- This receipt does not close Air, Assurance, Distribution, RF range/loss, or
  installer custody gates.
