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

The excluded implementation inputs are the same as the household's other clean-room crate
(`crates/sennet/PROVENANCE.md`): third-party protocol implementation source,
client applications, generated bindings, and implementation-derived API
references. The Python LXMF package and its client applications serve
strictly as external oracles. They are under the Reticulum License, which is
reproduced verbatim in [`oracle/RETICULUM_LICENSE`](oracle/RETICULUM_LICENSE)
in acknowledgment; its terms are honored for the oracle use, and its added
clauses do not attach to outrider's own code, which is MPL-2.0 by the
clean-room boundary this document records.

## Capture discipline

Each capture records the stock software and pinned version, the transport,
the direction, the input, the raw bytes, and the acceptance result. Captures
are committed as fixtures so CI replays them with no Python. Release notes
pin the stock baseline being matched.

## Record

### 2026-07-27: message object

PyPI LXMF 0.9.6 with RNS 1.4.2 packed one fixed message through its public API:

- timestamp `1753603200.5`
- title bytes `TITLE`
- content bytes `BODY`
- fields `{7: binary(meta)}`

The full signed object, source public key, derived message id, and inputs live
in `tests/fixtures/lxmf_0_9_6_message.json`. The fixture proves the actual
payload order is timestamp, title, content, fields, resolving an ambiguity in
the public prose. Tests rebuild the complete signed bytes and message id
without Python.

The codec now encodes only facts backed by that capture and the public format
description: fixed 16-byte addresses, 64-byte signature, four-item MessagePack
payload, optional fifth stamp, SHA-256 message id, and exact signature
preimage. Arbitrary fields stay as MessagePack values at the boundary.

Process note: during broad web source discovery, an agent opened the official
`LXMessage.py` before reading this repository's stricter local boundary. No
source was copied or translated into the crate. Every wire fact retained in
the implementation is independently reproduced by the fixed black-box
capture above. Future work returns to the capture-only boundary.

### 2026-07-27: direct delivery

Pinned LXMF 0.9.6 / RNS 1.4.2 stock clients exchanged fixed messages with
Outrider over live Retinue TCP links in both directions. Separate captures
cover one-packet messages and 4 KiB Resource-backed messages. Both sides
agreed on title, body, message id, and selected transport. Cost-8 delivery
announces caused stamps to be generated and enforced in both directions.

`tests/fixtures/lxmf_0_9_6_direct.json` retains the fixed announce and message
receipt used by CI.

### 2026-07-27: propagation submit, fetch, and server

Pinned stock clients supplied the propagation facts retained in the crate:

- the seven-item node announce and its cost triple;
- submit container `[transfer_time, [binary entries]]`;
- submitted entry `destination(16) || identity-encrypted message || stamp(32)`;
- transient id `SHA256(destination || encrypted message)`;
- propagation-stamp workblock derived from the transient id with 1,000
  black-box-observed expansion rounds;
- the fetch path hash, initial `[nil, nil]` offer request, selection
  `[wanted ids, handled ids, 1000]`, offered transient-id list, and fetched
  unstamped encrypted entries.

The workblock derivation was recovered by invoking the installed stock
package with fixed inputs and instrumenting callable arguments at runtime.
No implementation body was read. The fixed submit, announce, stamp, and fetch
requests live in `tests/fixtures/lxmf_0_9_6_propagation.json`; CI decrypts the
entry, verifies its signature, recomputes its ids and stamp value, and parses
the request grammar without Python.

Live receipts then proved Outrider submitting to and fetching from stock, and
a stock client submitting to and fetching from Outrider's bounded server.

Process note: later broad web searches surfaced snippets from the official
`LXMRouter.py` search result, and a symbol search over the installed oracle
package surfaced function names and line locations. Those outputs were not
used as implementation inputs. Every retained announce, stamp, submit, and
fetch fact above was independently reproduced through fixed black-box bytes
or callable behavior before it entered the crate.

### 2026-07-28: direct delivery over Tulle RF

The direct-delivery state machine was repeated over the product radio path,
using the Heltec V4 on COM6 and T114 on COM10 at 906.875 MHz, BW 250 kHz,
SF8, CR 4/5, and 17 dBm. One continuous headed session passed cost-8 stamped,
authenticated delivery in both directions for both transport forms:

- 18-byte content in one Retinue Data packet;
- 4,096-byte content in a Retinue Resource.

Receivers checked source identity, message id, title, content, transport
mode, and stamp validity. The receipt and the carrier timing discovered while
running it are recorded in
`design_docs/2026-07-28_outrider_direct_phy_delivery.md`.

### 2026-07-28: large propagation fetch responses

A black-box stock capture established the response-Resource boundary that the
public prose leaves ambiguous:

- advertisement flag `0x10` marks a response Resource, alongside encrypted
  flag `0x01`;
- advertisement field `q` is the 16-byte request packet id;
- the Resource content is the complete `[request_id, response_value]`
  MessagePack response envelope, so `q` and the envelope bind the same request.

The captured advertisement is replayed by Retinue's Resource codec test.
Live receipts then crossed the boundary in both directions with a fixed
4,096-byte message: Outrider fetched and authenticated a stock node's Resource
response, and a stock client submitted to Outrider, fetched Outrider's
Resource response, and decoded the original title, content, and message id.
The headed receipt is recorded in
`design_docs/2026-07-28_outrider_large_propagation_response.md`.

### 2026-07-28: opportunistic delivery and Retinue R9

Pinned LXMF 0.9.6 / RNS 1.4.2 emitted a fixed opportunistic message through
the public `LXMessage(..., desired_method=OPPORTUNISTIC)` and
`LXMRouter.handle_outbound` APIs. Retinue decrypted the ratcheted single
packet and exposed 144 plaintext bytes. Unlike direct delivery, those bytes
omit the 16-byte LXMF destination:

`source(16) || signature(64) || MessagePack payload`

The Reticulum packet header supplies the destination. Prepending it reproduces
the ordinary signed LXMF object, including the same message id and signature
preimage. The capture lives in
`tests/fixtures/lxmf_0_9_6_opportunistic.json` and rebuilds byte-exactly in
tests.

Live un-stamped delivery then passed in both directions over TCP: Outrider
verified stock's source signature and retained ratchet id, and stock's
delivery callback decoded Outrider's title/content and agreed on the message
id. A separate executable test enforces a cost-8 stamp and sends through a
Retinue transport node. The stock cost-8 opportunistic sender did not emit a
packet during the bounded oracle run, so that exact live combination remains
open rather than being inferred from the direct-delivery receipt.
