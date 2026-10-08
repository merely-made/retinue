# RNS 1.5.7 comparative review

**Date:** 2026-10-07. **Status:** findings; nothing adopted yet. The owner picks
what to adapt, and each adaptation is listed in the crate's `NOTICE`, per the
[Reticulum License adoption](2026-10-05_reticulum_license_adoption.md).

## Source and provenance

- **Upstream:** RNS 1.5.7, from `rns-1.5.7-py3-none-any.whl` (PyPI, SHA-256
  `0e4aaf8cc47f770da34572d2215b45fea900e104b009551600eb9741a41faddf`, the wheel
  qualified in the [1.5.7 repin](../testing/receipts/rns-1.5.7-repin/README.md)).
  It was unpacked into a session scratchpad outside the repository. GitHub had
  no 1.5.7 tag on this date.
- **Training condition:** the owner confirmed on 2026-10-07 that model training
  is off for the Claude account used for this review.
- **Method:** four read-only reviews (Transport, Link, Resource, and
  Identity/Packet/Destination/Channel/Buffer), each reading the RNS file next
  to the matching retinue modules. No RNS code was copied or translated; the
  findings describe behaviour and cite lines. The highest-impact findings were
  re-checked against retinue's code by the coordinating session; those are
  marked **verified**.
- **Not read:** RNode firmware, microReticulum, Prns, rns-rs, or any other
  third-party implementation. RNS `Interfaces/` files were read only for MTU
  autoconfiguration (`Interface.py` 94-95 and 245-275, plus grep hits).

**RNS lines read:**

| File | Ranges |
|---|---|
| `Transport.py` | 40-330, 685-1915, 1914-2880, 3165-3330, 3380-3605 (in chunks, with overlapping reads) |
| `Link.py` | 1-1505 |
| `Resource.py` | 1-1376 |
| `Identity.py` | 1-960 |
| `Packet.py` | 30-591 |
| `Destination.py` | 30-691 |
| `Channel.py` | 23-606 |
| `Buffer.py` | 23-372 |
| `Reticulum.py` | 93-160, 258-261, 308, 544-584, 884-886, 948, 1041-1100, 1149, 1238-1266, 1606-1612, 1818-1846, 2044-2085 (mostly grep hits) |
| `Cryptography/` | `Token.py` 30-114, `HKDF.py` 30-113, `PKCS7.py` 25-48, `Ed25519.py` 25-70, `Provider.py` 25-68, `X25519.py` 120-173 |

## A testing gap behind several findings

The four RNS Resource gates (`interop_resource_recv`, `interop_resource_send`,
`interop_send_large` and `interop_send_multiseg`) each drive `Incoming` and
`Outgoing` from their own example program. None of them goes through the
library's `ResourceSender` or `ResourceReceiver`, or through the `Endpoint` and
`Node` paths. The outrider large-message gate checks only the retinue side.
That is how R1 and R2 below got past a green repin. Every fix below should land
with a gate that runs the library path against stock RNS.

## Priority 0: interop bugs and safety (mostly small)

| # | Finding | Effort | Source |
|---|---|---|---|
| 1 | **The library's resource proof is a DATA packet, so RNS never accepts it.** `finish`/`reprove` use `framed_packet` (always DATA; `resource_transfer.rs` 398-431) instead of the existing `resource_proof_packet` (`link.rs` 555). An RNS sender then never completes and cancels. `Node` also routes RNS's PROOF-type proof to link setup only (`node.rs` 1788, 1847-1868). RNS: `Resource.py` 765-777, `Link.py` 1157-1164. **verified** | S | Resource R1 |
| 2 | **Received bz2 is decompressed without a bound.** `recover` calls `decompress` (`resource.rs` 681), even though `decompress_bounded` exists. Any link peer can send a decompression bomb. RNS caps at 64 MiB and cancels (`Resource.py` 123-125, 698-705). **verified** | S | Resource R3 |
| 3 | **Link-data proofs use the wrong key in both directions.** A retinue initiator proves link packets with its long-term identity (`endpoint.rs` 4814-4842), and a retinue responder validates only against an IDENTIFY'd identity. RNS initiators prove with the link's ephemeral Ed25519 key, carried in request bytes 32..64 (`Link.py` 186-189, 274-285, 326-331, 378-408). Reliable streams against RNS retransmit endlessly; the live gate passes only through a Channel window quirk. Retinue-side behaviour **verified**. | S-M | Link F1 |
| 4 | **A forged LINKCLOSE kills a reliable stream.** The driver closes on `context == CTX_LINKCLOSE` without decrypting (`endpoint.rs` 4924-4927). RNS closes only if the payload decrypts to the link id (`Link.py` 674-683). **verified** | S | Link F3 |
| 5 | **The Channel send window can run more than 48 sequences past a lost envelope.** An RNS receiver drops anything that far ahead but still proves it, so the data is silently lost. Bound the window by sequence span as well as by count. RNS: `Channel.py` 357-369, `Link.py` 1147-1154. retinue: `channel.rs` 322-350. | S | Channel F1 |
| 6 | **Buffer ignores the Channel message type.** Any non-stream message is parsed as stream data and can inject bytes, set EOF, or tear the link down (`channel.rs` 417-445, 775-830). RNS dispatches by `MSGTYPE` (`Channel.py` 118-128, `Buffer.py` 151-165). | S | Channel F2 |
| 7 | **The address book never evicts, and route learning and relaying are gated on admission to it.** `forget` has no callers. Once full (32 on `Node`, 8 on `Instance`), new destinations are neither learned nor relayed. RNS culls by age (`Transport.py` 957-978, 1169-1176). **verified** | M | Transport 1 |
| 8 | **Path responses are rebroadcast.** RNS learns from them but never relays them (`Transport.py` 2303, 2351). `Node::relay_announce` and the Endpoint relay ignore the context, so one path request floods the mesh. **verified** (Node). | S | Transport 6 |
| 9 | **The IFAC flag is neither enforced nor stripped.** Frames with bit 7 are decoded on non-IFAC interfaces, and `Packet::encode` sets bit 7 again when forwarding (`packet.rs` 275-277). RNS drops these either way (`Transport.py` 1765-1784). | S | Channel F4 |
| 10 | **Endpoint learns its own echoed announce:** a route to itself, a `PeerAnnounce` for itself, and a re-relay. Node already filters this (N5). RNS: `Transport.py` 2177, 2211. | S | Transport 7 |
| 11 | **A multi-segment (over 1 MiB) RNS resource is truncated to its first segment** and returned as if complete (`resource.rs` 532-561, `endpoint.rs` 632-750). Interim: refuse `l > 1` with an RCL. Full: segment accumulation and sending, adapted from `Resource.py` 274-339 and 702-835. | S, then M | Resource R2 |

## Priority 1: interop features and robustness

| # | Finding | Effort | Source |
|---|---|---|---|
| 12 | **No keepalive, stale detection or watchdog.** RNS tears down idle links to retinue (about 15-26 s on a LAN). A vanished peer hangs retinue forever. Adapt `Link.py` 80-106, 722-802, 1130-1135. Needs 13. | M | Link F2 |
| 13 | **RTT is never measured.** The Endpoint always sends 0.05 s, and Node never sends an RTT packet, so an RNS responder stays in HANDSHAKE and drops the link after about 6 minutes (`Link.py` 419-438, 516-538, 732-742). | S-M | Link F7 |
| 14 | **Link MTU is not clamped in transit or at the terminus.** RNS peers linking through a retinue relay can agree on 8192 bytes, and every frame over 500 is then dropped. Clamp to the ingress and egress frame limits (`Transport.py` 2058-2088, 2538-2566). Node's responder should also offer min(own, requested) (`node.rs` 1827-1843). | S-M | Transport 4, Link F4/F9 |
| 15 | **Channel retransmission never gives up.** No retry cap, no backoff, and RTT samples taken from retransmits. Adapt RNS's 1.5^tries backoff, the 5-try limit with teardown, and Karn's rule (`Channel.py` 455-483). | M | Channel F3 |
| 16 | **Transit bridges are never validated.** No proof deadline. `Node` evicts live links for forged requests, and the `Endpoint` map is uncapped (1 h TTL). Adapt RNS's validated state and deadline (`Transport.py` 872-955, 2060-2160, 2641-2671). | M | Transport 3, Link F13 |
| 17 | **Path requests.** Node never answers them, not even for its own destination. The Endpoint answers on every interface with no tag dedup, hop check or rate floor. Adapt `Transport.py` 1828-1886, 3454-3456. | S-M | Transport 2 |
| 18 | **Inbound links are unbounded in count and lifetime.** Per-link tasks, 64 KiB buffers, and unbounded channels. Add caps and bounded queues, plus 12's timeouts. | M | Link F8, Channel F15 |
| 19 | **No proofs for single packets (PROVE_ALL/APP) and no delivery receipts;** proofs also can't cross a retinue relay, because there is no reverse table. Adapt `Transport.py` 2104-2110, 2595-2605, 2733-2744 and `Packet.py` 391-539. | M | Channel F7, Transport 5/13 |
| 20 | **Single-packet encryption.** `send_single` refuses destinations without a ratchet, although `encrypt_to_identity` exists. A ratcheted registration always refuses identity-key packets; in RNS, enforcement is opt-in (`Destination.py` 502-513, 596-654). | S | Channel F5/F6 |
| 21 | **Resource cancel and reject.** retinue never sends RCL or ICL, accepts them unauthenticated, has no accept hook and no `max_response_size`. Adapt `Resource.py` 155-165, 1090-1136 and `Link.py` 1035-1126. | S-M | Resource R8, Link F11 |
| 22 | **Map-hash collisions break transfers**; the receiver keys parts by their 4-byte hash, and its lookups are O(n²). Use index-based storage, HMU placement by `segment*74`, and a collision re-roll on send (`Resource.py` 440-512, 877-901). | M | Resource R4 |
| 23 | **Requests too large for one packet** (sent as a Resource with the `u` flag) are unsupported in both directions (`Link.py` 473-510, 885-895, 1036-1043). | M | Resource R10, Link F5 |
| 24 | **Resource metadata (flag 0x20) is ignored.** Metadata responses fail (`Resource.py` 261-272, 707-749). | S-M | Resource R9 |
| 25 | **The link MDU is 367 instead of RNS's 431 at MTU 500,** because of the `WRITE_CHUNK` clamp in `endpoint.rs` 62-80. Requests of 368-431 bytes fail; responses become Resources. | S | Link F6 |
| 26 | **Routes and bridges survive interface detach.** Sends go to a dead interface id until the route TTL expires (`Transport.py` 865-978). | S | Transport 10 |
| 27 | **Endpoint link setup uses a fixed 15 s timeout, and there is no path rediscovery after a failed setup.** Reuse Node's `link_request_timeout` and request the path on timeout (`Transport.py` 697-725, 2062). | S | Transport 12, Link F10 |
| 28 | **Endpoint has no general duplicate filter**, and it processes type-2 packets addressed to another transport (`Transport.py` 1630-1680). Keep N10's no-Channel exemption. | S-M | Transport 8 |
| 29 | **No resource proof recovery via CACHE_REQUEST.** Node sends its proof once (`Resource.py` 652-671, 773). This depends on 1. | S-M | Resource R12 |
| 30 | **Ratchet lifecycle.** No rotation at announce, and the endpoint works on a cloned store. Own epochs expire by creation age rather than by count. The snapshot is unauthenticated (`Destination.py` 85-90, 206-288, 437-475). | M | Channel F8 |
| 31 | **The address book** overwrites a destination's key without the mismatch check RNS does, and is not persisted (`Identity.py` 100-349, 569-577). | S, then M | Channel F14 |

## Priority 2: efficiency and throughput

| # | Finding | Effort | Source |
|---|---|---|---|
| 32 | **The Resource window is fixed** (74 on Endpoint, 4 on Node). Adapt RNS's adaptive window: 4, growing to 10, then 75 on fast links, capped at 4 on very slow ones, with per-link carry-over (`Resource.py` 58-100, 862-935, `Link.py` 1257-1266). | M | Resource R5 |
| 33 | **Resource retransmit timing.** Requests repeat every 500 ms even while parts arrive. Re-adverts are unlimited, duplicate adverts trigger duplicate serving, and there is one 30 s deadline for the whole transfer. Adapt the watchdog (`Resource.py` 577-683). | M | Resource R6 |
| 34 | **Each hashmap update costs an extra idle round trip** (about 30 per 1 MiB segment). Send the exhausted flag along with the last known parts (`Resource.py` 942-976). | S | Resource R7 |
| 35 | **Channel window growth.** Retinue grows by 1 per 10 proofs and cuts by 4 per retransmit pass; RNS grows by 1 per proof with tier promotion (`Channel.py` 236-245, 417-477). | S | Channel F10 |
| 36 | **The announce-freshness ledger** does O(rows × blobs) work per accepted announce (about 65k steps at Endpoint defaults) under a global lock. Use a `BTreeMap` index and periodic expiry. Independent fix. | M | Transport 14 |
| 37 | **Per-packet waste:** `full_hash` re-encodes the whole packet. `Node::on_proof` runs `prove()` twice. Announces are verified before the cheap hash binding and before dedup. Ratchet trial decryption re-derives public keys and clones the store per packet. The token re-keys AES/HMAC per call. Reliable runs an O(1024) `retain` per proof. All independent fixes. | S each | Channel F9/F11/F13, Transport 15, Link F15 |
| 38 | **Resource copies:** about 4 full copies at peak, plus per-part clones on send. Fold into 22's index-based storage. | M | Resource R13 |
| 39 | **Announce rebroadcast:** no jitter by default, no neighbour-rebroadcast suppression, no airtime cap. The Endpoint can't relay on the ingress interface, so a single-radio repeater never relays (`Transport.py` 765-829, 1522-1585, 2180-2203, 2338). | S, then M | Transport 9 |

## Owner decisions and minor items

- **The same-blob tombstone versus route repair (Transport 11).** retinue keeps
  a blob for 7 days and rejects it as a replay. A transport answering
  `request_path` from cache sends that same blob, so a route behind RNS
  transport can't be restored until the destination re-announces. P8 shaped the
  current rule, so this is the owner's call. One option: admit a same-blob
  candidate when no route exists and a path request is outstanding.
- **Ratchet enforcement default (Channel F5).** Keep strict (anti-downgrade) and
  document it, or follow RNS's opt-in.
- **Minor items:**
  - Empty data fields are accepted (`Packet.py` 275), and the `packet.rs`
    header doc has the header order wrong.
  - Request and proof lengths and modes are not validated (`Link.py` 187,
    225-227, 396-405).
  - Link packets are not tied to the link's interface.
  - Replayed resource REQs are re-served.
  - Resource part size and HMU batching differ below MTU 500.
  - Codec gaps: msgpack `0xcd`/`0xce`, the split flag, and shortest-form ints.
  - The hop ceiling is off by one.
  - PLAIN/GROUP announces are not rejected.
  - App names with dots are accepted.
  - No GROUP destinations.
  - No send-side Buffer compression.
  - A later IDENTIFY overwrites a resource session's peer.

## Checked and equivalent

The reviews found these to match RNS:

- identity, destination, name and ratchet-id hashing;
- announce layout and signed message;
- Token format and HKDF;
- packet flags, header-2 order and hash masking;
- the MDU constants;
- IFAC derivation and masking (retinue compares in constant time);
- Channel envelopes and Buffer frames;
- link id, trailer, proof message, key derivation, RTT encoding, LINKCLOSE
  payload, keepalive bytes and IDENTIFY;
- request and response shapes;
- resource hashes, advertisement fields, REQ/HMU formats, compression choice
  and context dedup exemptions;
- the freshness decision (except the gravity and unresponsive carve-outs);
- type-2 insertion and stripping;
- the path-request wire format and 20 s floor;
- Node's link-request deadline (N3/N8).

The deliberate divergences were respected: the own-echo filtering (N10, no
Channel exemption), the 30-minute route TTL, and the two held RNS defects.

## Suggested order

1. **Priority 0, items 1-10, and item 11's interim refusal.** Each is small. Add
   a library-path oracle gate for resources (send and receive through
   `Endpoint`, including the RNS sender's completion) and a mixed RNS/retinue
   reliable-stream gate that checks proofs.
2. **Link liveness:** items 12, 13, 14 and 25, which together make retinue a
   well-behaved link peer.
3. **Transit hygiene:** items 16, 17, 19, 26, 27 and 28.
4. **Resource throughput:** items 22, 32, 33 and 34, then 11's full
   segmentation and 23.
5. **Efficiency:** item 37 opportunistically; item 36 when transit load matters.

Findings marked "adapt" become `NOTICE` entries when they land; independent
fixes do not.
