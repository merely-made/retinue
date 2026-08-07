# Stamp cost on boards: the measurements, the midstate, and who verifies

Date: 2026-08-07. Status: library and firmware landed; on-board receipts for the
sliced check and the mint pending the T114's return from demo prep.

## The measurements

From the v45 T114 receipts (64 MHz Cortex-M4, 48 KB heap), all via the `lxmf`
probes against captured stock artifacts:

- Decode a stock LXMF 0.9.6 message and confirm its id: 183 us, 120 B heap.
- Score the captured propagation stamp, 1,000 rounds: 1,868 ms, zero heap.
- Message-cost stamps are 3,000 rounds, so about 5.6 s by extrapolation.
- The whole lane (codec, stamps, baked fixtures) cost 6.4 KB of flash.

Derived figures used below: about 1.9 ms per round, and roughly 65 us per
SHA-256 compression, which puts one mint trial at one compression.

## The property we inherit

LXMF scores a stamp as leading zero bits of `SHA256(workblock || stamp)`, where
the workblock is derived from the message's transient id by rounds of
HKDF-SHA256. Deriving the workblock IS the expensive part, and the verifier
must derive it too, so verification costs roughly what the sender's setup
costs. That is unusual among proof-of-work schemes, and wire compatibility
means we inherit it: no cheaper verifier exists without leaving the protocol.
On host CPUs the check is milliseconds and invisible. On a 64 MHz M4 it is
seconds, which inverts the scheme's purpose at the small end: the checker pays
real work per novel message, triggered by bytes that are cheap to send.

## Two folds through fixed-size state

The workblock never needs to exist, because its only use is to be fed into a
hash. Both halves of stamp work fold through a fixed-size hash state:

- **Checking** streams each round into the hasher and drops it. Zero heap,
  proven on the board. The materialised forms (256 KB at propagation cost,
  768 KB at message cost) were never possible against a 48 KB heap.
- **Minting** exploits alignment: rounds are 256 bytes, SHA-256 absorbs in
  64-byte blocks, so the hasher that absorbed the workblock holds everything
  scoring needs in about a hundred bytes of midstate. `stamp::Derivation`
  keeps that midstate; each trial is a clone plus one compression. Expected
  trials are `2^target`, so trial time passes the 1.9 s derivation only near
  target 15. Below that, minting a stamp costs about what checking one costs.

Equivalence tests hold both folds to the materialised implementations exactly:
same score, and the same stamp from the same seed.

## Executor discipline

The first on-board check ran as a blocking loop inside embassy and held the
executor silent for its full 1.9 s. The watchdog fires at 8 s, so message-cost
checks at 5.6 s leave a margin too thin to keep. `Derivation` is the fix's
shape: the caller advances a budget of rounds, yields, repeats. The probes now
yield every 8 rounds (about 15 ms held per slice) and every 256 mint trials
(about 17 ms). The radio shares the probe's executor slot and still pauses for
the probe's span; a live verification lane would own a low-priority task,
which the same object serves without change.

## Who verifies, and where

- **Over RF, the air is the scarce resource.** Triggering one check costs an
  attacker a full message of airtime. Duty-cycled regions self-limit the
  attacker; and in any region, a flood that saturates the channel has already
  denied service before verifier CPU becomes the binding constraint. Stamp
  enforcement was never the board's real shield; the bearer is.
- **Board inboxes require no stamps.** The receive API already makes stamp
  cost optional. When a board does want checking, it is deferred, budgeted,
  and sheddable: a bounded queue that drops newest under pressure, never a
  promise to verify at line rate.
- **Enforcement at rate belongs to hosts.** Internet-facing propagation nodes
  see unbounded arrival rates and have millisecond checks; that is where
  requiring and verifying stamps earns its keep. The T114 was already ruled
  out as a propagation store by its 48 KB heap, before CPU entered into it.
- **For a board endpoint, minting is the load-bearing half.** A peer that
  announces a stamp cost is unreachable to a sender that cannot mint. With
  the midstate, a board mints a propagation-cost stamp in roughly the
  derivation time plus a second of trials at target 14, in zero heap. That
  turns "boards cannot participate in stamped delivery" into a latency
  figure.

## Open hardware questions

Measure, do not assume: the nRF52840 carries CryptoCell 310 SHA-256, but many
small HMAC operations are its worst case, so the gain over the ~1.9 ms
software round is unproven. The V4's ESP32-S3 runs 240 MHz with a SHA engine
and is likely severalfold cheaper per round; giving it the same `lxmf` probes
would name the family's natural stamp-weigher with a number.

## Receipts pending

- v46 on the T114: the sliced check's `took=` beside v45's blocking 1,868 ms,
  and the first `lxmf mint ok` line (value, nonce, derive and total times).
  The board is currently flashed as a stock RNode 1.86 for demo-prep
  Bluetooth pairing and returns via linkboy.
- V4 probe timings, if the stamp-weigher question becomes load-bearing.
