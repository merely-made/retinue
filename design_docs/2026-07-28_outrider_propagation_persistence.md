# Outrider propagation persistence receipt

**Date:** 2026-07-28  
**Boundary:** caller-persisted `PropagationStore` snapshots  
**Stock baseline:** LXMF 0.9.6 / RNS 1.4.2

## Ownership

Outrider owns propagation-store semantics:

- stamp-gated admission before storage;
- transient-id duplicate suppression;
- bounded message, entry-count, and total-byte capacity;
- oldest-first capacity eviction;
- age expiry;
- delivery-destination owner scoping;
- owner-scoped acknowledgement.

The host owns the filesystem or database and decides when and how a snapshot
is flushed. This follows the existing Sennet packet-ID boundary instead of
putting host filesystem policy inside the protocol crate.

## Snapshot

`PropagationStore::encode_snapshot` emits a versioned MessagePack record
containing each encrypted propagation message and its receipt time. Derived
transient ids and byte counts are deliberately omitted.

`PropagationStore::restore` validates the complete record before returning a
store. It re-decodes every message, recomputes its transient id and byte count,
removes expired entries, suppresses duplicates, rejects messages over the
current per-message or total-byte bounds, and performs oldest-first eviction
under the current entry and byte limits. Unknown versions and malformed or
over-limit snapshots fail without exposing a partial store. The convenience
restore uses a 16 MiB input bound; larger hosts can set their own explicit
bound with `restore_bounded`.

The stored LXMF bodies remain identity-encrypted for their recipients. The
snapshot does not add plaintext message content.

## Restart receipts

The executable restart test writes a snapshot to a temporary host file,
drops the store, restores it, and crosses real Retinue links. It proves that:

- the first identity can fetch only its message;
- its later acknowledgement is persisted;
- after another restore, the second identity's message remains and is
  fetchable;
- the acknowledged first message does not return.

The stock-client oracle was also run twice against the same host snapshot.
The second process reported:

```text
STORE_RESTORED loaded=1 duplicates=0 rejected=0 expired=0 evicted=0
stock submitted to Outrider: PASS
stock fetched from Outrider: PASS
stock decoded title/body/id: PASS
OUTRIDER_PROPAGATION_SERVER: PASS
```

The headed logs and the resulting snapshot are retained outside Git:

- `C:\t\outrider-propagation-restart-r1.log`
- `C:\t\outrider-propagation-restart-r2.log`
- `C:\t\outrider-propagation-restart-20260728.snapshot`

## Limits of the claim

This is a durable-state boundary, not a bundled database. The example host
flushes and synchronizes one file; a product host may use atomic file
replacement, SQLite, or another transactional store. Inter-node propagation
sync and opportunistic delivery remain separate work.
