# FS2: which carrier a signed command travels in

**Date:** 2026-08-10. **Lane:** Assurance (ASSURE3, ASSURE4).
**Status:** decided and implemented. FS2 closed in software; FS3 remains open.

ASSURE4 asks one question: does FS2 use the interoperable RNS signed-artifact
carrier, or a smaller Retinue envelope? ASSURE3 exists to answer it with
evidence rather than taste, so the evidence comes first.

## The evidence (ASSURE3)

RNS 1.4.2's signed artifacts are now reproduced byte for byte in
[`retinue::artifact`](../crates/retinue/src/artifact.rs).

The vectors were captured by driving `rnid`, the executable RNS ships, as an
operator would: `capture_signed_artifact.py` writes an identity file, signs
with `--sign` and `--sign-message`, and reads back the `.rsg` and `.rsm` bytes.
Nothing imports RNS. Six cases are committed in
`crates/retinue/tests/fixtures/rns_signed_artifact.json`: detached, embedded,
and embedded-with-metadata, each for two identities. Ed25519 is deterministic,
so byte equality is reachable, and anything weaker would be a passing test that
proved only that we can call SHA-256.

One of the two identities is the one Prns uses in its own signed-artifact
tests. That makes the same capture an independent check of Prns's published
constants, which a donor's self-tests cannot supply. RNS emits exactly the
string Prns asserts. That is recorded as a test, because corroboration is worth
having and worth noticing if it ever stops being true.

Reading the envelope layout from Prns is what made this cheap. The vectors are
not from Prns, so they remain independent oracle evidence rather than
donor-conformance evidence. The distinction is the harvest brief's, and it is
the reason the capture script exists at all.

## The decision

**FS2's normative wire form is a compact fixed-offset Retinue envelope. The
signed artifact is not on the field node's command path.**

```text
command  = envelope || ed25519_signature(64)
envelope = version(1) || class(1) || key_id(16) || target(16)
           || counter(8, big-endian) || opcode(1) || payload_len(2) || payload
signed   = "retinue-command-v1" || envelope
```

Three reasons, in the order they mattered.

**One bounded parser on the metal.** FS1's ruling is that the on-metal decode
path returns errors and counts them, because a loud assert on an RF parse path
is a remote reset button. A fixed-offset envelope is a length check and seven
slices. The artifact carrier is a MessagePack reader with nesting, container
counts, and string decoding, all of it reachable by a stranger before any
signature has been checked. Retinue now has that reader and it is bounded
(`retinue::msgpack`), but the cheapest attack surface is the one that is not
there. The artifact parser stays on the host tier, where FS1 permits loud
assertions.

**The carrier re-sends what the node must not learn.** An RSG envelope carries
the signer's 64-byte public key and 16-byte identity hash on every artifact. A
field node already holds the operator's public key in its allowlist, and rule 2
of the security posture is that authorization flows one direction. A command
whose key arrives inside the command is a shape that invites verifying against
the key the message brought. Naming the key id and looking it up locally makes
the wrong implementation harder to write than the right one.

**Size, but last.** The compact envelope is 45 bytes of header against the
artifact's 224-byte floor for a detached signature over nothing. That matters
on LoRa, and it keeps a whole command inside one packet so authorization never
depends on reassembly. It is listed third because it would not have been
sufficient on its own.

### What the artifact is for instead

It stays, tested, on the host tier, for what H7 actually described: signed
service descriptions, invitations, distribution records, and carrying an
immutable firmware manifest over Reticulum. Those are host-tier artifacts read
by hosts, and interoperability with stock `rnid` is a real property there. An
operator with RNS installed can produce and check one with no Retinue tooling
at all.

## What FS2 implements

[`retinue::command`](../crates/retinue/src/command.rs), `no_std`, allocation
free, bounded allowlist. Against FS2's stated validation conditions:

- **A replayed command is rejected across a reboot.** Freshness is a
  per-operator monotonic counter, not a clock, because a field node has no RTC
  and no trustworthy time at reboot. `Verifier::ledger` exposes the counters and
  `Verifier::restore` takes them back, and the test reboots a verifier through
  that seam and replays.
- **A command over a foreign bearer verifies identically to serial.** There is
  one entry point and it takes bytes. No overload accepts a link, a session, or
  a peer, so transport-independence is structural rather than promised.
- **Possession of a live session without the key authorizes nothing.** Same
  fact from the other side, and the fuzz target asserts it: no command
  attributed to an unallowlisted key is ever accepted.

Two design points that are easy to get wrong and are therefore written down.

**The counter window has two edges.** Acceptance requires strictly greater than
the last accepted counter, and no more than `COUNTER_WINDOW` (4096) beyond it.
The lower edge is the replay rule. The upper edge is the one that is easy to
omit: without it, one intercepted command carrying a counter near `u64::MAX`
locks that operator out of that node permanently, which is a denial of service
available to anyone who can listen. 4096 is generous because an operator's
counter is theirs and not this node's, so a node that was unreachable while the
fleet was commanded must still be able to catch up.

**The signature covers a domain prefix that is never sent.** Retinue signs
announces and link proofs with the same key material. Prefixing
`retinue-command-v1` means a command signature is only ever a command
signature. There is a test that signs a bare envelope, the way another signing
context would, and watches it be refused.

Checks run cheapest first: shape, version, allowlist, addressing, counter
window, then the signature. An unsigned flood costs lookups rather than curve
operations. This does not make verification free. An attacker who knows a live
key id and a plausible counter still forces one Ed25519 verification per
attempt. That is a rate-limiting problem, it belongs to the Air lane's
interface admission machinery (H1/AIR2), and no signature scheme solves it
here. Recorded rather than papered over.

## What this does not close

- **FS3, the durable counter.** The ledger seam exists and is tested through a
  simulated reboot. Until FS3 writes it to flash, a real reboot resets the
  window. FS3 was correctly sequenced after FS2: the wear-leveled slot log now
  has a settled grammar to bind.
- **Opcode semantics.** The envelope carries an opcode and a payload and
  assigns meaning to neither. The consuming firmware owns that, and it should
  be settled alongside FS3 rather than invented per caller.
- **Allowlist lifecycle.** De-listing a seized relay and rotating operator keys
  are themselves signed commands, so the bootstrap and lockout stories are
  still open: who signs the command that revokes the only key. `authorize` is
  idempotent and never reopens a replay window, and `restore` only ever moves a
  counter forward, so the primitives do not make the eventual answer harder.
- **On-metal evidence.** Everything here is host-side. No board has verified a
  command over RF. That is a receipt the Air lane's bench owns.

## Changed here

- `crates/retinue/src/msgpack.rs`: the MessagePack subset RNS puts on the wire,
  canonical on encode and bounded on decode.
- `crates/retinue/src/artifact.rs`: RSG/RSM create and validate.
- `crates/retinue/src/command.rs`: the FS2 envelope and verifier.
- `crates/retinue/oracle/capture_signed_artifact.py` and
  `tests/fixtures/rns_signed_artifact.json`: the ASSURE3 vectors.
- `crates/retinue/tests/signed_artifact.rs`: byte-exact replay of all six.
- `crates/retinue/tests/command_corpus.rs`: asserts the committed fuzz seeds
  still reach the verifier, so the corpus cannot rot into silent non-coverage.
- `fuzz/fuzz_targets/retinue_command_accept.rs` plus its seed, and the
  `retinue-command-accept-fuzz` suite and CI step.
