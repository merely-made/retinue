# Outrider large propagation response receipt

**Date:** 2026-07-28  
**Stock baseline:** LXMF 0.9.6 / RNS 1.4.2  
**Carrier:** live Retinue TCP links  
**Payload:** fixed 4,096-byte content

## Captured boundary

A stock propagation node returned its large fetch result as a Resource. The
captured advertisement established three facts:

- flags are encrypted `0x01` plus response `0x10`;
- `q` is the 16-byte request packet id;
- the Resource content is the full `[request_id, response_value]` MessagePack
  response envelope.

Retinue now parses and emits that request binding, verifies that `q` agrees
with the envelope, and automatically chooses a direct response or Resource
from the packed response size.

## Headed receipts

Stock node to Outrider:

```text
OFFERED 1
FETCHED 1
CONTENT_LEN 4096
PRODUCTION_FETCH true
OUTRIDER_LARGE_FETCH_FROM_STOCK: PASS
```

Outrider server to stock client:

```text
SERVER_STORED inserted=1 rejected=0 entries=1 bytes=4320
SERVER_SERVED offered=1 served=1 acknowledged=0
stock submitted to Outrider: PASS
stock fetched from Outrider: PASS
stock decoded title/body/id: PASS
OUTRIDER_PROPAGATION_SERVER: PASS
```

The server proof also exposed a lifecycle condition: the accepted Resource
session must remain owned until the peer projects the completed response.
`serve_fetch` therefore borrows the accepted session; its caller decides when
the session can close and when endpoint shutdown begins.

## Limits of the claim

This closes the large one-node fetch-response carrier boundary. It does not
add durable storage, inter-node propagation sync, or opportunistic delivery.
The default in-memory store remains conservatively bounded at 240 encrypted
bytes per message; applications opt into larger limits explicitly.
