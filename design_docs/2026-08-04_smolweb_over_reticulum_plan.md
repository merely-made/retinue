# Smolweb over Reticulum — the three shared pieces

**Date:** 2026-08-04
**Status:** scoped with Mark. Not started.
**Depends on:** nothing in this repo that is not already built.

**Why this is short.** Thirteen small-web protocols run over a Reticulum link
with **no protocol changes at all**, because the compatibility test is only
"does it need a bidirectional byte stream", and `LinkStream` is one
(`crates/retinue/src/endpoint.rs:208,218`). The reasoning is in mere's
[carrier independence analysis](../../mere/design_docs/nematic_docs/technical_architecture/2026-08-04_protocol_carrier_independence.md);
the conclusion is that this is **not per-protocol work**. Three shared pieces,
written once, serve all of them.

Already proven: retinue's own `gemini_over_reticulum` example serves and
fetches a capsule over a real link, and `gemini_protocol::exchange` (published
2026-08-03) is transport-generic with a test that runs it over an in-memory
duplex, no TCP and no TLS. So the protocol side is done and published. What is
missing is everything *around* the stream.

---

## R-A. The addressing adapter

**The problem.** Every one of these protocols puts a hostname in its request
line, and Reticulum has no DNS and no host:port. A destination is an
`AddressHash`, optionally reached by a name resolved against announces.

**What exists.** `DestinationName::new(app_name, aspects)` and
`destination_hash(identity)` (`crates/retinue/src/destination.rs`), plus the
announce-matching resolution the gemini example already demonstrates:
recompute `name.destination_hash(identity)` for each announcer and match. That
is how Nomad Network addresses nodes, so it is the idiom to follow rather than
invent around.

**The decisions, and they are the whole of this step:**

1. **What a URL authority means.** Two forms need to coexist: a direct
   destination hash, and a name resolved by announce. Recommend both, with the
   name as the human form and the hash as the durable one.
2. **What gets stored.** A durable address must survive restart and re-resolve.
   Recommend storing the **destination hash** as the identity, carrying the
   announced name as an annotation rather than the reverse, since names are
   first-come and hashes are not.
3. **How the request line reads.** The protocol still writes an absolute URL
   (gemini requires it). Decide whether the authority written on the wire is
   the name or the hash. Recommend the form the user typed, because rewriting
   it changes what the server sees and some protocols echo it back.

**Done when:** a smolweb URL naming a Reticulum destination resolves to a link
by both forms, an address stored before a restart resolves after one, and a
name that no longer announces fails with a message that names the destination
rather than a DNS-shaped error.

**Not in scope:** a name registry or any notion of ownership. First-announce
wins, and that property is disclosed, not fixed.

---

## R-B. The posture vocabulary

**The problem, and it is a correctness one.** A client that reports the same
security posture regardless of what carried the bytes is lying about at least
one carrier. Concretely, and both directions are wrong today:

- **Gopher over a link is not "insecure".** The link is encrypted and the peer
  is proven by its destination key. Reporting "unauthenticated by design"
  because the scheme is `gopher://` understates the real security badly.
- **Gemini over a link is not "TOFU".** There is no certificate and no pin, so
  reporting a pin state reports a thing that does not exist.

This already produced a correction to mere's smolweb fidelity plan, whose WS2
mapped scheme to posture. **Posture is a property of the carrier**, with the
protocol contributing only what it adds on top.

**What a Reticulum link actually proves,** and the vocabulary should say
exactly this and no more:

- the peer holds the private key for a destination hash (the link handshake
  establishes it; `verify_data_proof` is the same primitive elsewhere);
- the channel is encrypted end to end;
- **not** that the peer is who a *name* said they were, unless the name was
  resolved by recomputing the hash from the announced identity, which is a
  separate and weaker claim because first-announce wins a name.

**Recommend** a distinct posture rather than reusing TOFU's word. TOFU means "I
pinned a key I could not verify, and I will notice if it changes". Reticulum
means "the address *is* the key, so there is nothing to pin". Those are
genuinely different and a user should be able to tell them apart. Name-resolved
addressing is the one that deserves a caveat, not the hash-direct form.

**Done when:** a fetch over a link reports a posture naming key-proven
identity, a name-resolved fetch reports the weaker name claim distinctly, and
neither borrows TLS vocabulary. The descriptor is produced beside the bytes, as
the fidelity plan's WS2 requires.

---

## R-C. Resources for bulk bodies

**The problem.** Reading a body to EOF over a link works and is what the gemini
example does. It is the wrong shape for a large page: no progress, no
compression, and no explicit completion.

**What exists.** Resources are implemented at the protocol level
(`crates/retinue/src/resource.rs`): windowed transfer, compression
(`compress`/`decompress`), part splitting, and proofs (`resource_hash`,
`proof`, `parse_proof`) — and `Endpoint` already surfaces "one fully received,
verified Resource". Nomad Network serves node pages this way, so this is also
the compatibility path for talking to real NomadNet nodes.

**The decision:** when a body rides a Resource rather than the stream. A
protocol's own framing usually says when a body ends, so this is a **carrier
optimisation, not a protocol change**, and it must stay invisible above the
carrier. Recommend a size threshold with the stream as the default, and never
let the choice change what the protocol layer sees.

**Done when:** a body above the threshold transfers as a Resource with real
progress and verified completion, the protocol layer is unchanged and unaware,
and an interrupted transfer fails loudly rather than yielding a truncated body
that looks complete.

---

## Ordering and cost

R-A first: nothing can be fetched until an address resolves, and it gates all
thirteen protocols. R-B next, because shipping a fetch that misreports its
security is worse than shipping no fetch. R-C last and optional; it is an
optimisation plus NomadNet compatibility, not a prerequisite.

The cost is genuinely small, and that is the point of the analysis this comes
from: three pieces, once, for thirteen protocols, on top of a link layer that
is already built and RF-proven.

## Not in scope

- **Micron and LXMF.** Reticulum-native formats, and this plan is about
  carrying *smolweb* protocols. They are analogues of gemtext and misfin
  rather than gaps, and they belong to the
  [Reticulum browsing plan](../../turnstone/design_docs/2026-08-03_reticulum_browsing_plan.md).
- **Where the composition lives.** Whether errand grows a Reticulum lane behind
  a feature or turnstone composes a separate bridge is open; errand is
  deliberately light and retinue is a large dependency, so the bridge is
  likelier right. Not urgent, and not decided here.
- **Guppy and fsp.** Datagram protocols whose own reliability duplicates the
  link's. Carrying them is possible and pointless; the honest response is not
  to offer that combination.
