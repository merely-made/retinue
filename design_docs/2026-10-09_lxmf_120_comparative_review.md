# LXMF 1.2.0 comparative review (outrider)

**Date:** 2026-10-09. **Status:** findings. Fixes land in batches. Each
adaptation goes in `crates/outrider/NOTICE`.

## Source and provenance

- **Upstream:** LXMF 1.2.0 from `lxmf-1.2.0-py3-none-any.whl` (PyPI, SHA-256
  `805f053e61082f585efc6d09beffd039fd921174c5e1547a2e690bc479d75b85`, the oracle
  pin). Unpacked in a session scratchpad outside the repository. RNS 1.5.7 was
  read for the context of the encryption paths.
- **Training condition:** confirmed off for this account (2026-10-07).
- **Method:** three read-only reviews.
  - Message format, fields, stamps and tickets.
  - Client-side delivery in `LXMRouter.py`.
  - The propagation node and peer sync.
  - No LXMF code was copied; findings cite lines. The top claims were re-checked
    against outrider's code and are marked **verified**.
- **LXMF lines read:**
  - In full: `LXMessage.py`, `LXStamper.py`, `LXMF.py`, `Handlers.py`, `LXMPeer.py`.
  - `LXMRouter.py`: 1-2997 across the three reviews.
  - `Utilities/lxmd.py`: 74-240, 395-510, 985-1060 (configuration defaults only).
- **RNS lines read:** `Destination.py` 596-654; `Identity.py` 69-84, 352-370,
  485-508, 852-905; `Cryptography/HKDF.py` 43-83; `vendor/umsgpack.py` 320-324,
  976-991.

## Testing gap

The outrider gates check only what outrider printed. None checks the stock side's
delivery state. That is how A1 and B3 got past a green repin. Fixes add gates
that require the stock state to be DELIVERED, and require that a second fetch
finds nothing.

## Priority A: interop breaks with stock LXMF today

| # | Finding | Effort |
|---|---|---|
| A1 | **Inbound messages are never proved.** Opportunistic registration leaves `ProofStrategy::None`, and direct link Data is never proved. A stock sender's small message therefore stays SENT, is retried every 10 s up to 5 times, and ends FAILED. Stock proves first (`LXMRouter.py` 1995-1996, 2805-2846). **verified** | M (retinue seam) |
| A2 | **No duplicate suppression.** Stock keeps a locally-delivered cache by message id and transient id for 180 days (`LXMRouter.py` 1013-1033, 1975-1980, 2565-2577). Outrider has none, and stock resends are re-encrypted, so A1's retries become duplicates. | S-M |
| A3 | **A nil display name is rejected, and a nameless announce drops its stamp cost.** Stock always emits `[name or nil, cost or nil, [features]]`, with cost in 1..254 (`LXMRouter.py` 321-323, 386-402, 1042-1058; `LXMF.py` 152-186). **verified** | S |
| A4 | **The propagation-node announce decoder is too strict.** Stock accepts 7 or more elements, 3 or more costs, and float limits; lxmd announces floats (`LXMF.py` 225-250; `lxmd.py` 165-184). | S |
| A5 | **The fetch session ignores the third `/get` (the delete acknowledgement).** Delivered messages come back, and with `max_per_fetch=1` stale entries starve the queue. The node must loop on requests, accept `[nil|ids, nil|ids, limit?]`, and treat the limit as a KB budget, integer or float (`LXMRouter.py` 1499-1560, 1637-1645). **verified** | M |
| A6 | **Client fetch:** one bad entry aborts the whole fetch, already-handled ids use up slots, there is no final `[nil, haves]` acknowledgement, and 0xf0/0xf1 are not typed (`LXMRouter.py` 1571-1651). | S-M |
| A7 | **Propagated messages ignore ratchets in both directions.** Stock encrypts to the recipient's ratchet and decrypts ratchets first (`LXMessage.py` 434-436; RNS `Destination.py` 606-651). Needs retinue `encrypt_for` / `decrypt_for` seams. | M |
| A8 | **Propagated messages carry no delivery stamp, and fetched messages are not stamp-checked** (`LXMRouter.py` 1822-1826, 2558-2576, 2673-2690). | S |
| A9 | **An unknown source is refused after the transfer was already proved,** so the message is lost silently. Stock delivers it flagged unverified (`LXMessage.py` 814-827). Return a verification status and keep refusing an invalid signature. | M |
| A10 | **Opportunistic receive refuses unratcheted packets.** Stock enforcement is opt-in (`LXMRouter.py` 103, 157, 370-371). | S |
| A11 | **Outbound receipts are dropped.** Return the single-packet receipt, await the proof on direct sends and submissions, and map a node's 0xf5 to `Rejected` (`LXMessage.py` 468-515, 604-628; `LXMRouter.py` 2321-2329, 2739-2749). | M |

## Priority B: node correctness, robustness and features

| # | Finding | Effort |
|---|---|---|
| B1 | **Size admission comes after the transfer.** Set `set_max_resource_size` before `receive` on the node and on direct inbound. Tie the announced limit to the store's limit (`LXMRouter.py` 2046-2053, 2289-2292). | S |
| B2 | **The store keeps no stamp or stamp value,** which peering needs. Add a v2 snapshot that reads v1 (`LXMRouter.py` 2581-2585; `LXMPeer.py` 345-363, 451-460). | S-M |
| B3 | **No memory of processed transient ids** (180 days, `LXMRouter.py` 1013-1033, 2565-2568). | S |
| B4 | **Submission policy:** validate per entry and keep the valid ones, use cost minus flexibility as the floor, send 0xf5 on invalid stamps, throttle the sender for 180 s, and allow one entry per non-peer transfer (`LXMRouter.py` 2303-2329, 2451-2470, 2515-2523). | M |
| B5 | **Store efficiency:** replace the linear scans with an id map and a per-destination index. Use weighted eviction (size × age, prioritized destinations) instead of FIFO. Base restore on the limits, not a fixed 4096 (`LXMRouter.py` 1064-1075, 1191-1226). | S-M |
| B6 | **The node announce timebase is static.** Build it with `now` on each announce (`LXMRouter.py` 193, 332-344, 2083). | S |
| B7 | **Error responses** 0xf0/0xf1 and an optional allow list (`LXMRouter.py` 465-484, 1480-1492). | S |
| B8 | **Tickets:** 16-byte ticket stamps (`truncated_hash(ticket‖message_id)`), field 0x0C issuance, inclusion and validation, a snapshot-able ticket book (`LXMessage.py` 42-53, 278-308; `LXMRouter.py` 1081-1142, 1309-1373, 1832-1841, 1917-1933). | M |
| B9 | **Paper and URI messages** (`lxm://`, 2210-byte limit) (`LXMessage.py` 102-106, 451-463, 705-723; `LXMRouter.py` 2611-2627). | S |
| B10 | **Field registry constants** (`LXMF.py` 8-143). | S |

## Priority C: efficiency and smaller items

- **C1. Stamping cost.** A stamped send derives the 3000-round workblock twice; reuse the found stamp. Check the opportunistic size before minting (a stamp adds 34 bytes). `stamp::find` re-hashes the workblock per trial; use the midstate like `find_streamed`. On the host, mint in parallel and validate node batches off the executor.
- **C2. Codec.** `codec.rs` copies large messages 4-5 times; move it onto the `portable` encoder. Decide one rule for the id of a non-canonical stamped payload (`LXMessage.py` 762-769).
- **C3. Leniency.** Accept string-typed title and content (as stock's unchecked decode does), integer `transfer_time`, and legacy UTF-8 delivery app data.
- **C4. Stamp outcome.** Report the stamp outcome (value, or ticket) on received messages.

## Larger parity work (later batches)

- **L1. Outbound delivery state machine:** a sans-IO `Outbox` with stock states, attempts and timing, announce-triggered retries, and opportunistic-to-direct fallback (`LXMRouter.py` 32-38, 1754-1864, 2754-2997; `Handlers.py` 15-54).
- **L2. Link reuse and backchannel:** cached direct links with a 600 s idle teardown, and a backchannel IDENTIFY (`LXMRouter.py` 959-973, 2064-2067, 2770-2783, 2855-2868).
- **L3. Propagation-node peering and sync.** Peering keys (cost 18), `/offer`, sync scheduling and backoff, and inbound sync dispatch. Also autopeering within 4 hops, peer persistence, and the control destination (`LXMPeer.py` all; `LXMRouter.py` 777-875, 2073-2403). B2, B3 and B5 are its prerequisites.

## Upstream defects not to port

- `LXMRouter.py:2367` uses `and` where `or` is meant.
- `LXMRouter.py:2410` has an always-true type check.
- `LXMPeer.py` 253 and 308 use an undefined `destination_hash`.
- `lxmd.py` 170-177 overrides the transfer limit.

## Checked and equivalent

All of the following match upstream:

- **Message:**
  - hash and preimage;
  - signature;
  - stamp as the fifth element, excluded from hash and signature;
  - f64 timestamp and narrowest bin encoding.
- **Stamps:**
  - workblock (3000 rounds for messages, 1000 for propagation; HKDF-256 with the msgpack round salt);
  - stamp validity, up to the 2^-256 boundary;
  - midstate scoring.
- **Propagation:**
  - transient id, entry layout and submit container;
  - fetch request shapes;
  - `/get` deletion order and scoping;
  - 30-day expiry;
  - identification required;
  - KB = 1000 bytes.
- **Packets and links:**
  - opportunistic framing and the 383-byte limit;
  - the choice between a direct packet and a Resource (link MDU);
  - compression declared off.
- **Fields and policy:**
  - the audio field;
  - inbound stamp enforcement under `enforce_stamps`;
  - source derivation;
  - path-request pacing.
