# Permissive radio protocol compatibility survey

**Date:** 2026-08-25. **Status:** research and architecture record. This is a
revision-pinned survey, not a new delivery lane and not a gate receipt.

**Related authority:** [mesh household](2026-07-20_mesh_household_tulle_tucket_sennet.md),
[listener executive and protocol leases](2026-08-10_listener_executive_and_protocol_leases.md),
[Prns harvest brief](2026-08-09_prns_harvest_brief.md), and
[current RNS/LXMF re-pin receipt](2026-08-23_rns_150_lxmf_111_repin_receipt.md).
Those documents retain authority over landed architecture and gate status.

**Coverage:** named projects, active public implementations found through the
Reticulum and LoRa ecosystems, and the most relevant client libraries at the
pins linked below. This is broad enough to choose boundaries and next proofs;
it is not a claim that every unindexed fork or future repository has been read.
Every later candidate must repeat the licence and provenance gate.

## Finding

Retinue should not grow a universal mesh router. It already has the right lower
boundary: Tulle owns radio mechanics, the resident executive owns scanning and
leases, and each protocol adapter owns one wire format and bounded protocol
state. Five distinct integration shapes belong above that boundary:

1. **Exact-wire peer:** implement the foreign protocol and join its network.
2. **Application client:** use Retinue/Outrider as the network stack while
   matching an existing application's behavior and conventions.
3. **Radio personality:** capture and transmit a distinct LoRa PHY and wire
   format under a bounded `ProtocolAdapter` lease.
4. **Bearer:** carry opaque Reticulum frames through a mesh that the device
   genuinely participates in.
5. **Semantic bridge:** terminate both protocols at a trusted gateway and map
   selected messages between them.

Those shapes are not interchangeable. A bearer preserves the inner Reticulum
packet but pays nested-routing and fragmentation costs. A semantic bridge can
reach ordinary users on both networks, but it terminates cryptographic context
and must not present one network's identity or end-to-end security as the
other's. A radio personality provides coexistence on one device, subject to
scan physics; it does not make the networks mutually routable.

The immediate conclusions are:

- Keep Prns as Retinue's sole implementation donor and clean independent peer.
  Expand the executable peer matrix with other implementations, but do not use
  source-derived ports as implementation inputs.
- Re-pin Tucket against current MeshCore firmware before adding surface area.
  Tucket already has the strongest fact in this survey: real bidirectional,
  encrypted, acknowledged traffic through an official one-hop repeater.
- Evaluate UMSH next as a new exact-wire adapter. Its host/radio split is the
  closest external match to Retinue's executive and delegated-key design.
- Treat MeshChat, MeshChatX, and Crosstalk as application acceptance clients
  for Outrider and Signalman, not as Reticulum implementation donors.
- Keep LoRaWAN as a separate concentrator/uplink service. It is infrastructure,
  not a peer mesh.
- Reserve semantic bridges for explicit, configured gateways. Do not put
  content translation or cross-network identity synthesis into Tulle,
  Retinue's Transport, or the listener executive.

## Licence and provenance gate

The repository licence was checked before implementation reading. MIT,
Apache-2.0, BSD, ISC, MPL-2.0, EPL-2.0, and CC0 trees were readable under the
project rule. GPL and AGPL trees were stopped at licence identification. Code
under another licence or without a licence was also excluded. A permissive
root licence did not authorize a differently licensed subtree.

The Python RNS and LXMF references remained black-box. Their source was not an
implementation input. Public protocol prose, public API behavior, and observed
input/output bytes remain admissible. This is stricter than a repository's
declared licence and deliberately so.

Licence and donor status are separate questions:

| Evidence class | May be read? | May shape Retinue code? | May close independent interop? |
| --- | --- | --- | --- |
| `clean-donor` | yes | yes, with elected inbound licence and provenance ledger | only while an untouched executable remains independent |
| `peer-output` | executable behavior only for implementation work | output bytes and externally visible behavior only | yes, when the peer is untouched and independently run |
| `source-derived-peer` | yes under the stated licence gate | no; keep its implementation outside the clean-room context | yes, but label the shared ancestry |
| `official-doc` | public protocol prose only | yes | not by itself |
| `observed-wire` | yes | yes | yes when the setup and direction are recorded |
| `blocked-source` | licence name and boundary only | no | released binaries may be black-box peers if separately lawful |

This classification prevents a later MIT label from laundering source lineage.
Several readable ports explicitly say they mapped, ported, or generated code
from the restricted Python reference. They are useful peers and feature
inventories, but they are not donors.

## Reticulum implementations

Retinue is already at RNS 1.5.0. The current local H8 receipt runs the pinned
Prns 0.3.4 peer at `72b6b30d27cac910ce20d370e1dc711fe9b95955`
against Retinue and stock RNS 1.5.0, with seven full runs and 35/35 matrix
checks passing. Upstream Prns has since moved, so its newer revision is a new
pin rather than an automatic upgrade.

| Implementation | Survey pin and licence | Useful surface | Retinue posture |
| --- | --- | --- | --- |
| [Prns 0.3.7](https://github.com/KenAKAFrosty/Prns/tree/7b40d7dff1c7b95cee87c38a713719f086f8b4a7) | `7b40d7d`, MIT OR Apache-2.0 | Ground-up Rust; `no_std` fixed-capacity core; host-owned entropy, persistence and interfaces; Tokio and Embassy runtimes; TCP, serial/KISS/RNode, WebSocket, I2P, LoRa, USB and Bluetooth work | **Clean donor and independent peer.** Keep the receipted 0.3.4 pin until 0.3.7 has its own donor diff and peer receipt. Its stated RNS target remains 1.4.2, so exercise every claimed seam against Retinue's 1.5.0 baseline. |
| [Quad4 Reticulum-Go 1.0.2](https://github.com/Quad4-Software/Reticulum-Go/tree/2a114d0099bfc6e7daa0784c2da0e129efea6885) | `2a114d0`, Apache-2.0 | Host stack with transport, links, resources, channels and buffers; bounded ingress workers | **Executable peer only.** Its cross-reference tests clone the restricted reference. RNode, KISS, AX.25-KISS and Weave are absent. |
| [thatSFguy reticulum-go 0.1.1](https://github.com/thatSFguy/reticulum-go/tree/770c570d908e826da06cb6f21944411128f30c47) | `770c570`, MIT | Go RNS plus partial LXMF; TCP/HDLC, links, announces, proofs, opportunistic/direct messages and propagation submission | **Peer only, provenance unresolved.** Transit routing and multi-segment Resource remain incomplete. Its selected specification repository is CC-BY-4.0, outside this survey's readable set. |
| [Beechat Reticulum-rs 0.1.0](https://github.com/BeechatNetworkSystemsLtd/Reticulum-rs/tree/151e3b6c77a8c7d33fafa3971a084ae02510ef39) | `151e3b6`, MIT | Rust core with TCP/UDP/HDLC, paths, links and channels | **Source-derived peer.** It identifies itself as a Rust port. Several advertised daemon interfaces are not implemented, and the core still requires allocation. |
| [microReticulum 0.5.0](https://github.com/attermann/microReticulum/tree/40fa628809d57140180c1c833559ab96fec992c1) | `40fa628`, Apache-2.0 | C++ embedded stack; configurable allocation/storage, destinations, transport, links and Resource; useful failure notes from old interop | **Source-derived peer.** It identifies itself as a port and its receipts target RNS 1.2.9. Ratchets, Channel and Buffer are absent. Its global transport state is also the wrong ownership model for Retinue's resident executive. |
| [FreeTAKTeam LXMF-rs 0.10.0](https://github.com/FreeTAKTeam/LXMF-rs/tree/5436ee715f94f81e18abb0808cfca52fcd7cc9bc) | `5436ee7`, EPL-2.0; surveyed HEAD `9f4dd91` | Broad Rust RNS/LXMF workspace with host, embedded, daemon, RPC and FFI surfaces; claims RNS 1.5.0 and direct, opportunistic, paper and propagation coverage | **Source-derived peer.** It publishes a generated mapping to pinned Python source and treats Python as normative. Useful as a released executable and operational inventory, not as donor code. Hardware and public-network operation remain separate claims from parity tables. |
| [aerik reticulum-js 0.1.0](https://github.com/aerik/reticulum-js/tree/872d781b1a33c1f4d718a9c05dd0d224f2d790ca) | `872d781`, MIT | Browser/Node identities, destinations, announces, links, Resource, Channel and several interfaces | **Source-derived peer.** The project calls itself a JavaScript port and includes a Python-derived implementation note. LXMF status is internally inconsistent. |
| [liamcottle rns.js](https://github.com/liamcottle/rns.js/tree/d4602ca6b6091685040a101828183336bc2b1f53) | `d4602ca`, MIT | Browser-oriented TCP/WebSocket client with packets, announces, links and basic LXMF | **Application peer, not donor.** Useful for browser-facing consumer tests. Ratchets and broad Resource/Channel behavior are incomplete or absent. |
| torlando-tech [Reticulum-KT](https://github.com/torlando-tech/reticulum-kt/tree/2a3d2c1e0792a3fe44ef7789ced8460791e54d86), [Reticulum-Swift](https://github.com/torlando-tech/reticulum-swift/tree/e23dc0ce403e0abf72f7cd25b7dee6b7ef4bbe5a), [LXMF-KT](https://github.com/torlando-tech/LXMF-kt/tree/faa86fb44ab9db2b92efa7a194b9a2899235f65c) and [LXMF-Swift](https://github.com/torlando-tech/LXMF-swift/tree/60186710b9891d35dfb54e636935ddb506b581e6) | linked 2026 pins, MPL-2.0 | Android/iOS, BLE/RNode, mobile lifecycle, direct/opportunistic/propagated LXMF | **Source-derived mobile peers.** Their parity material relies on Python bridges and the LXMF port says so directly. Valuable for lifecycle and FFI acceptance, not implementation. |
| J-Krush [ReticulumKit](https://github.com/J-Krush/ReticulumKit/tree/d05342cd67050f624621d4a9fc23f5aceeac25c1) and [LXMFKit](https://github.com/J-Krush/LXMFKit/tree/25f3b5ceb382c7fa34f048f3e5362b39d9887650) | linked v0.1.0 pins, MIT | Swift actor-based alpha libraries with claimed direct, opportunistic and propagated LXMF | **Peer only, provenance unresolved.** The clean-room claim conflicts with Python-module citations and an incorrect statement about the reference licence. Require clarification before matrix admission. |
| [svanichkin go-reticulum](https://github.com/svanichkin/go-reticulum/tree/06621cc972ebd25e3ee1fb023d50dd34d38ce538) | `06621cc`, MIT | Broad Go RNS/LXMF/daemon family | **Source-derived peer.** The project describes LLM-assisted reading and porting of the Python reference. |
| [holiman Reticulum-Go](https://github.com/holiman/Reticulum-Go/tree/0413d92f2a71ec5ef4cdfea2114d14300ac6243b) | `0413d92`, MIT | Early Go implementation | **Low-value peer candidate.** It is visibly incomplete and lacks persuasive independent interop evidence. |
| [reticulum-zig](https://github.com/ion232/reticulum-zig/tree/cdf7da3aeb0398a6c96456add72388ed8445c1a2) | `cdf7da3`, MIT OR Apache-2.0 | Early embedded Zig work | **Low-priority peer candidate.** Its own embedded status says transport is not ready. Do not add it to the matrix until it can complete a bounded exchange. |
| [RTReticulum](https://github.com/0xSeren/RTReticulum/tree/dab4362cf3577e464e98e85b71abc5cb26185224) | `dab4362`, declared MIT | RTOS-oriented C++ identities, links and transport, based on microReticulum | **Quarantined peer.** Direct derivation is stated, but the surveyed tree does not preserve microReticulum's Apache licence/notice. Its Resource transfer is also described as simplified rather than wire-complete. Resolve licence debt before even binary matrix use. |

The peer matrix should become capability-based instead of implementation-based.
Each candidate earns rows for announce/path resolution, packet/proof, IFAC,
ratchets, both Link roles, request/response, Channel/Buffer, small and segmented
Resource in both directions, loss/retry, transport forwarding, persistence and
restart, then Outrider's direct, opportunistic and propagated LXMF paths.
Every row records sender, receiver, exact revisions, RNS/LXMF target, transport,
and whether the evidence is wire-independent or shares donor ancestry. There is
little value in running every possible pair; a standard consumer suite against
each peer gives stronger, cheaper evidence.

### Reticulum hard stops

Implementation source was not inspected for the following:

- official Python [RNS](https://github.com/markqvist/Reticulum/blob/b123a756b0e203070f7ff6325aaa2168504e0d82/LICENSE)
  and [LXMF](https://github.com/markqvist/LXMF/blob/795fdaa2b0777c13033787d933d1afc94a2377cb/LICENSE):
  Reticulum License; black-box boundary;
- `lelloman/rns-rs` and `lelloman/lxmf-rs`: Reticulum License;
- [ratspeak/rsReticulum](https://github.com/ratspeak/rsReticulum/blob/main/LICENSE)
  and `ratspeak/rsLXMF`: AGPL-3.0;
- [kageedwards/ferret-rns](https://github.com/kageedwards/ferret-rns/blob/main/LICENSE):
  LGPL-2.1, outside the readable set;
- [torlando-tech/microLXMF](https://github.com/torlando-tech/microLXMF/blob/main/LICENSE)
  and [pyxis](https://github.com/torlando-tech/pyxis/blob/main/LICENSE): GPL-3.0;
- [GlassHaven/Haven](https://github.com/GlassHaven/Haven/blob/main/LICENSE):
  AGPL-3.0;
- `jrl290/LXMF-rust`: placeholder licence text;
- `jrl290/Reticulum-rust` and `sergst83/reticulum-network-stack`: no detected
  licence;
- [thatSFguy/reticulum-specifications](https://github.com/thatSFguy/reticulum-specifications/blob/master/LICENSE):
  CC-BY-4.0, outside the CC0-only documentation gate.

## MeshChat is an application contract

There are two unrelated projects called MeshChat. They must stay distinct in
issues, packages and receipts.

| Application | Pin and licence | What it teaches | Boundary |
| --- | --- | --- | --- |
| [Liam Cottle Reticulum MeshChat](https://github.com/liamcottle/reticulum-meshchat/tree/df5aea94eab7f4be1cdfef494446e9d6979aed77) | `df5aea9`, MIT | LXMF text, images, audio and files; local history; peer discovery; retry after announce; propagation; identity import/export; NomadNet browsing | Read the application's own code and behavior. Its Python RNS/LXMF dependencies stay black-box. Use it as an Outrider/Signalman acceptance client through its REST/WebSocket process boundary. It binds to loopback by default and does not supply a hardened remote web boundary. |
| [MeshChatX](https://github.com/Quad4-Software/MeshChatX/tree/f0f6ed5764b4ff0e969c76242bb31daed812612f) | `f0f6ed5`, 0BSD for its modifications plus upstream MIT notices | Adds multiple identities, maps/offline tiles, richer local tools, HTTPS/WSS, optional authentication, CSRF protection, and a request-ID-based `rns.link.*` WebSocket API | **Strongest permissive black-box client seam.** Normal CI does not run its opt-in live LXMF suite, so parity tables are not a radio receipt. |
| [Crosstalk](https://github.com/buildwithparallel/crosstalk/tree/39b68bd55ab956e26175f68ca419335835eae6e9) | `39b68bd`, MIT | Interface import/export, explicit announce controls, field status, RNode/KISS/AX.25 configuration and a durable Iridium packet spool | Same application-only boundary. Its `RNSI` versioned whole-packet envelope, exact-frame duplicate cache, persisted queue and bounded retry policy are useful constrained-bearer design inputs. |
| [andrewdavidmackenzie/meshchat](https://github.com/andrewdavidmackenzie/meshchat/blob/a7fb3f97f07ce21ac9e6006e44a4505bfe740dd4/LICENSE) | GPL-3.0 | A different Meshtastic/MeshCore BLE application | **Stop.** Do not inspect source or conflate it with Reticulum MeshChat. |

Outrider already covers the hard network substrate: byte-preserving message
fields, direct and opportunistic delivery, Resource-backed large messages,
propagation client/server, persistence, stamps, and selected voice fields. The
MeshChat family points to the next consumer tests rather than a new wire stack:

- send text and a large attachment in both directions;
- retain unknown fields and attachment metadata across save/restart/forward;
- exercise identity import/export without changing identity bytes;
- prove retry-after-announce without duplicate delivery;
- submit to and fetch from each side's propagation server;
- show explicit interface state, announce cost, and exclusive RNode mode;
- report unsupported audio codecs as readable metadata rather than corrupting
  or silently dropping the message;
- for MeshChatX, exercise authentication, CSRF and every request-ID/lifecycle
  event at the WebSocket boundary.

Calls, maps, browsing and application layout belong above Outrider. They should
not become LXMF codec requirements unless observed wire behavior forces a codec
change.

## MeshCore and Tucket

The surveyed upstream [MeshCore packet format](https://github.com/meshcore-dev/MeshCore/blob/0679dbeffc504d562d2f09eb072fdc223f8ffc2a/docs/packet_format.md),
[payloads](https://github.com/meshcore-dev/MeshCore/blob/0679dbeffc504d562d2f09eb072fdc223f8ffc2a/docs/payloads.md),
and [companion protocol](https://github.com/meshcore-dev/MeshCore/blob/0679dbeffc504d562d2f09eb072fdc223f8ffc2a/docs/companion_protocol.md)
are MIT at `0679dbeffc504d562d2f09eb072fdc223f8ffc2a`. The network combines
first-seen flood discovery with compact source routes, signed adverts,
encrypted direct and group payloads, acknowledgements, companion/repeater/
room-server/sensor roles, BLE and serial companion APIs. The pinned packet
document describes optional four-byte transport-zone codes and up to 184 bytes
of payload.

Current release labels are ahead of the version named inside the packet
document. Treat documentation, companion, repeater and server versions as
separate pins. Do not infer a stable wire from one repository tag. Interop also
requires the complete receive profile, not merely frequency: current defaults
use private sync word `0x12`, while Sennet's receipted LongFast profile uses
`0x2B`.

MeshCore identity and security stay a foreign realm. Adverts are Ed25519-signed,
but normal routing can address contacts by very short public-key prefixes.
Direct payloads use static identity-derived key agreement and a two-byte
truncated authentication code; groups use shared keys and do not authenticate
displayed sender names. Retinue must retain the full advert key, detect prefix
collisions, label group authorship honestly, and never equate a MeshCore
contact with a Retinue identity. None of this crypto should become a Retinue
primitive.

Tucket already implements authenticated adverts, flooded text and ACKs,
forwarding, reciprocal route learning, direct retry and flood fallback. Its
headed receipts cover official companion 1.15.0 and repeater 1.16.0 firmware,
including encrypted text and acknowledgements in both directions through a
named one-hop relay. This makes Tucket the implementation authority. The MIT
[meshcore.js](https://github.com/meshcore-dev/meshcore.js/tree/1c142946f9597d60fc634afd9a681f546792b0d5)
and other host companion clients can inform management APIs, but they do not
replace an on-air Tucket receipt.

The current Tucket gap is sharper than "more payloads". Its codec can carry
one- to three-byte path elements and transport codes, while `Node` and the
forwarder still key contacts and forwarding decisions by one byte. Current
MeshCore also has request/response, anonymous, control/discovery, trace,
multipart, binary/group-data and raw custom forms whose consume, relay and ACK
policies are not yet proven in Tucket. Transport-scope policy needs the same
stock comparison.

The next Tucket slice is a re-pin, not a redesign:

1. pin current official companion 1.17.1 and one current repeater build;
2. replay advert, flood, learned direct route, encrypted text, delayed ACK and
   flood-fallback exchanges in both directions;
3. test one-, two- and three-byte paths, deliberate first-byte collisions,
   zero/one/multiple repeaters, route expiry and restart;
4. compare public, private, matching and mismatching transport scopes, including
   forward/drop behavior and hop caps;
5. compare current control, trace, group-data, binary, multipart, malformed and
   unknown payload behavior against stock;
6. inject loss, delayed ACKs and collisions, then assert bounded lease expiry
   and measured return to the Retinue scan plan;
7. record exact firmware artifacts, RF profiles, message bytes and direction.

MeshCore-over-Reticulum message translation and Reticulum-over-MeshCore bearer
traffic are later, separate features. A general tunnel should not be smuggled
into `GRP_DATA` or `RAW_CUSTOM`: usable frames are smaller than Reticulum's MTU,
fragmentation and congestion policy are absent, and foreign nodes would spend
airtime on opaque traffic. Any bearer must run through a real participating
MeshCore node and account its nested routing cost.

## Other readable LoRa work

| Project | Pin and licence gate | Architectural value | Retinue decision |
| --- | --- | --- | --- |
| [UMSH](https://github.com/darconeous/umsh/tree/3bab31881190e0b689ee48a904ad99d5a8a25d65) | `3bab318`, MIT OR Apache-2.0 for first-party core; one patched `nrf-sdc` repository has no detected licence | Rust/`no_std`; published v3 wire; Ed25519 identity, compact hints, pairwise keys, authenticated encryption, monotonic counters, flood/source routing and trace routes. Its [ULCP](https://github.com/darconeous/umsh/blob/3bab31881190e0b689ee48a904ad99d5a8a25d65/docs/protocol/src/ulcp.md) separates radio control, frame transport and narrowly delegated keys while the host retains long-term identity. | **Best new exact-wire candidate.** Audit `umsh-core`, `umsh-mac` and `umsh-crypto` independently of the blocked BSP dependency. Then require vector conformance and a two-device RF receipt. Mine ULCP's delegated-key and sleeping-host boundary even if the protocol adapter is deferred. |
| [tinySSB](https://github.com/ssbc/tinySSB/tree/39896b72c97b51159d46610c5f11ff7f5a279031) | `39896b7`, MIT core; Android vendors LGPL-2.1 Codec2, which is excluded | Signed, append-only feed replication over fixed 120-byte LoRa/BLE frames; WANT/CHNK/DATA replication and side chains for larger values | **Mere sidequest, not chat translation.** Import each foreign signed feed as an append-only source with replication receipts and projections. A later exact-wire adapter can carry opaque frames. Do not read or reuse the Android voice subtree. Alpha maturity keeps it behind UMSH. |
| [LoRaMesher](https://github.com/LoRaMesher/LoRaMesher/tree/1abec4a850389afcfdcae0e41c965b58bbeb701f) | `1abec4a`, MIT | FreeRTOS/C++ network-manager election, sponsor join, TDMA superframes, distance-vector routing, link aging and TTL/sequence dedup | **Scheduling donor, not bridge target yet.** Compare its TDMA admission and route aging against LE/FT, but authentication and key distribution remain future work. |
| [Bramble](https://github.com/justinlindh/bramble/tree/9d2ca32a312c19f13649b597fdbeb8d95d722705) | `9d2ca32`, MIT | Early ESP32/SX1262 mesh with signed identities, HMAC framing, optional fleet trust, encrypted direct ratchets/channels, delivery tiers, airtime buckets and adversarial simulator | **Security and test donor.** Its fail-closed provisioning, budget-gated transmit path and saturation tests are more valuable than immediate wire interop. |
| [ClusterDuck Protocol](https://github.com/ClusterDuck-Protocol/ClusterDuck-Protocol/tree/f15103bd78c3ebe3dd768ba7004a590a7b0fa700) | `f15103b`, Apache-2.0 | Role-based sensor relay network, request/reply routing, next-hop cache, Bloom dedup and MQTT sinks; compact source/destination DUIDs | **Optional sensor gateway.** It lacks cryptographic identity and payload protection, so translate explicit sensor events at a trusted gateway rather than presenting it as secure messaging. |
| [LoRaMac-node](https://github.com/Lora-net/LoRaMac-node/tree/dcbcfb329b4a343ab007bc19ac43a8dc952b3354) and SX1302 HAL | `dcbcfb3`, BSD-3-Clause; Semtech HAL BSD | LoRaWAN end-device and concentrator infrastructure | **Separate uplink/observer.** A multi-channel SX1302 can hear several LoRa channels, but LoRaWAN is not a peer mesh and a concentrator does not erase higher-layer or PHY-profile differences. Keep this outside the SX1262 resident executive. |
| [MeshRoute](https://github.com/stachuman/MeshRoute/tree/cb76d793295492d81a519f78b3a4e78fd37f8ddc) | `cb76d79`, BSD-3-Clause | Small experimental routing implementation | **Low priority.** Revisit only if it produces an external peer or a mechanism missing from stronger candidates. |
| RadioLib | MIT | Broad radio driver and modulation oracle | **Test oracle only.** Retinue already owns its Rust PHY and radio-executive boundary. Use RadioLib to cross-check register/profile behavior, not as core ownership. |

UMSH is the strongest architectural surprise. ULCP lets a companion radio hold
only the channel or pairwise keys needed for bounded receive, queue and ACK work
while the host retains the long-term private identity and may sleep. That is
close to Retinue's resident executive, bounded lease and delegated-custody
problem. It is worth a small dependency-audited experiment even if joining a
UMSH network never becomes a product feature.

tinySSB's useful relation is to Mere rather than Outrider. A tinySSB feed is a
foreign signed append-only source, naturally represented by projections and
replication receipts. Translating entries into chat messages would discard its
authorship, ordering and conflict semantics. Retinue can eventually carry its
opaque frames while Mere decides how to project them.

### Mixed or blocked adjacent trees

- **[Meshtastic firmware and protobuf schemas](https://github.com/meshtastic/firmware/blob/develop/LICENSE):**
  GPL-3.0. Stop. Sennet's existing
  capture-and-public-prose clean-room boundary stands.
- **meshtastic-lite:** BSD-3-Clause at its root, but its own comments say code
  and methods were extracted from named GPL firmware files. Reject it as a
  clean-room source. Stop at that provenance finding.
- **[MeshCom firmware](https://github.com/icssw-org/MeshCom-Firmware/blob/e9723548d6a84bcdd23a5d6352b4bd5f7580484a/README.md):**
  root MIT, but the surveyed tree contains a GPL-3.0 GFX
  component, LGPL-3.0 TinyGSM and CC-BY-SA-3.0 icons. Stop at those boundaries.
  Any future adapter needs independent protocol prose and observed bytes.
- **Andrew Mackenzie's MeshChat:** GPL-3.0. Stop.
- **ExpressLRS:** GPL-3.0. Stop.
- **disaster.radio:** no usable repository licence was found. Stop.

## The bridge boundary

A semantic bridge is a trusted application, not a router. It receives and
authenticates a native message, applies an owner-configured policy, creates a
new message on the destination network, and stores the relationship between
the two. Its durable bridge envelope needs at least:

- source protocol and configured network/channel;
- native source identity and native message ID;
- ingress gateway and receipt time;
- content kind plus opaque unsupported fields or attachments;
- observed security state, stated without upgrading it;
- stable bridge ID, direction and remaining bridge-hop budget;
- destination message IDs once emitted.

The stable bridge ID and hop budget prevent two gateways from reflecting the
same message forever. Deduplication uses native IDs plus the bridge ID, not
text hashes. Identity mappings are explicit configuration. A bridge may label
an author from another network, but it must not mint a lookalike key and imply
that the foreign author signed the destination-network message. Encryption is
terminated and re-established at the gateway, which the UI must say plainly.

Bridge policy is per direction and per content kind. Text, position, telemetry,
files, group messages and commands have different disclosure and amplification
risks. Defaults belong in Signalman configuration rather than protocol code.
Commands should remain disabled across semantic bridges unless a later threat
model and receipt explicitly admit them.

## Radio coexistence

The listener executive already supplies the right physical model. An SX1262
captures one exact `ReceiveProfile` at a time. Shared frequency/SF/BW can share
a CAD observation, but different sync words, coding rates, header modes or IQ
settings still require separate capture windows. MeshCore `0x12`, the existing
Sennet/LongFast `0x2B`, tinySSB `0x58`, and any UMSH profile therefore consume
real scan budget.

Adding a readable protocol to this survey does not mean adding it to every
board's scan plan. Each adapter first declares exact receive/transmit profiles,
worst-case lease and response windows, participation level, airtime cost and
session obligations. The runtime refuses an overfull registry. Continuous
coverage across several profiles remains a flock property or a separate-radio
property, not a promise one SX1262 can make.

## Recommended sequence and done conditions

1. **Reticulum peer expansion.** Upgrade the disposable Prns peer from 0.3.4 to
   0.3.7 only after a source/donor diff. Add released Quad4 Reticulum-Go and
   LXMF-rs binaries as source-derived peers. Done when the capability matrix
   records exact pins and both directions for each executed seam, while donor
   and independent evidence stay visibly separate.
2. **MeshCore re-pin.** Move current official companion and repeater builds
   through Tucket's existing headed harness. Done when advert, flood, learned
   route, encrypted text, ACK, retry, fallback, multi-byte paths, scope policy,
   restart and a multi-repeater case pass with pinned artifacts.
3. **MeshChat consumer receipt.** Run Liam Cottle MeshChat and MeshChatX as
   untouched localhost applications against Retinue/Outrider. Done when
   identity, announce, text, large file, unknown fields, retry, propagation,
   restart and link lifecycle pass without reading Python dependencies.
4. **UMSH feasibility slice.** Audit the isolated first-party core dependency
   graph, implement only enough adapter surface for published vectors, then run
   two independent radios. Done when the licence ledger is clean, vectors are
   byte-exact, a malformed frame cannot extend a lease, and both RF directions
   pass with exact revisions.
5. **Bearers and semantic bridges.** Start only after exact-wire membership for
   the underlying foreign mesh. Done when fragmentation, nested airtime,
   duplicate suppression, loop prevention, security labeling and opt-in policy
   are all exercised through a real participating node.
6. **Sidequests.** Prototype tinySSB as a foreign append-only Mere source; use
   LoRaMesher and Bramble for scheduler/security tests; keep ClusterDuck and
   LoRaWAN as explicit gateways; preserve Crosstalk's constrained-bearer lessons.
   None should enter the resident scan registry without a forcing consumer and
   a measured acquisition budget.

This sequence adds interoperability evidence before abstraction. Retinue's
existing `ProtocolAdapter`, lease, Tulle, Tucket, Sennet, Outrider and peer
matrix boundaries are sufficient. The survey found candidates and sharper
tests, not a missing universal protocol layer.
