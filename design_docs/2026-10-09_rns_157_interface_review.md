# RNS 1.5.7 interface comparative review (retinue)

**Date:** 2026-10-09. **Status:** findings. Fixes land in batches. Each
adaptation goes in `crates/retinue/NOTICE`. The relicence to adapt RNS is on
record (2026-10-05).

## Source and provenance

- **Upstream:** RNS 1.5.7, the oracle pin (`rns==1.5.7` in the repin venv). The
  wheel was unpacked into session scratchpads outside the repository
  (`ifreview-datagram/whl`, `ifreview-serial/rns`). Retinue was read at `main`
  @ `78b18d8`.
- **Training condition:** the same account as the LXMF 1.2.0 review (confirmed
  off on 2026-10-07).
- **Method:** four read-only reviews:
  - base interface behaviour;
  - stream carriers;
  - datagram carriers;
  - serial and radio carriers.

  No RNS code was copied, and every finding cites its lines. Claims marked
  **verified** were checked by a scratch probe that edits nothing in the repo,
  run either against stock 1.5.7 in the oracle venv or as a Rust crate that
  depends on retinue by path.
- **RNS lines read:**
  - **Read in full:**
    - Interfaces: `Interfaces/Interface.py`, `TCPInterface.py` (1-704),
      `BackboneInterface.py` (30-1125), `LocalInterface.py` (30-493),
      `PipeInterface.py`, `UDPInterface.py`, `AutoInterface.py` (1-710),
      `SerialInterface.py`, `KISSInterface.py`, `AX25KISSInterface.py`,
      `RNodeInterface.py` (30-1582; the BLE and TCP classes were read with their
      log lines elided).
    - Utilities: `util/HDLC.py`, `util/TransmitBuffer.py`, `util/netinfo.py`
      (30-324).
  - **Read in part:**
    - `RNodeMultiInterface.py`: 30-400, 535-700, 815-1165.
    - `WeaveInterface.py`: 38-135, 162-410, 560-750, 816-1083.
  - **Grep only:** `I2PInterface.py`, the `DEFAULT_IFAC_SIZE`, `HW_MTU` and
    `BITRATE_GUESS` constants in every interface, and `Discovery.py` 182 and 230.
  - **`Reticulum.py`:** 85-500, 600-1316 and 1498-1700, the union of the four
    reviews.
  - **`Transport.py`:** 40-180, 235-240, 307-365, 525-610, 805-830, 955-1102,
    1140-1165, 1268-1345, 1405-1632, 1682-2095, 2195-2570, 2760-2860,
    3160-3325 and 3420-3725.
  - **Other files:** `Link.py` 73 and 512-514; `Packet.py` 101-110;
    `Identity.py` 80 and 352 (grep); `vendor/platformutils.py` 55-61.
- **Host facts used by the gates:**
  - pyserial 3.5 opens a macOS pty slave with RNS's own `open()` arguments.
  - `TIOCMBIS` on a pty returns ENOTTY.
  - The feth cloner is present.
  - lo0 is skipped by stock AutoInterface on Darwin.
  - en1 is the usable fe80 multicast NIC here.
  - Backbone does not run on macOS.

## Testing gap

The repin gates pass while four interop breaks sit underneath them:

- `interop_ifac` pins `ifac_size = 64` bits on both sides.
- `interop_ifac` exchanges only a small announce and payload.
- No gate reads stock interface state: `has_path`, link status, resource
  status, `AutoInterface.peers`, or `get_interface_stats()`.
- There is no datagram lane and no serial lane.

New gates must pass on the **stock** side's own state. That means one of:

- stock proved the packet or completed the resource;
- stock learned the path;
- stock's link went ACTIVE;
- stock's peer table holds retinue.

Retinue's own output is not enough.

## Priority A: interop breaks, or common stock configs cannot be reproduced

| # | Finding | Effort |
|---|---|---|
| A1 | **The HDLC deframer drops IFAC frames above 500 bytes.** `Deframer` caps frames at `packet::MTU` before IFAC is stripped (`iface/hdlc.rs` 43-46, 113-117), on both TCP paths (`attach.rs` 349, `iface/tcp.rs` 89). A full link packet is 499 bytes, so it is 515 on the wire with 16-byte IFAC and 507 with 8. RNS bounds frames by HW_MTU + ifac_size (`HDLC.py` 62-106; `Transport.py` 1764-1791; `TCPInterface.py` 338-341). Full link packets, resource parts and channel frames are lost without a counter, and retinue's own outbound frame limit (`attach.rs` 186, 318) breaks retinue-to-retinue IFAC in the same way. **verified** (a 515-byte sealed frame gives 0 frames). Reviews: IF-1, stream-01, SER-1. | S |
| A2 | **`Ifac::DEFAULT_SIZE` is 8, but stock TCP, UDP, Auto, Backbone, I2P and Local default to 16.** Only Serial, KISS, AX.25, Pipe and RNode default to 8 (`Interface.py` 96; `TCPInterface.py` 77, 466; `UDPInterface.py` 42; `AutoInterface.py` 50; `SerialInterface.py` 53; `RNodeInterface.py` 110). Config `ifac_size` is in bits, must be at least 8, and is divided by 8; `""` and `"None"` count as unset (`Reticulum.py` 884-902, 1041-1042). A stock `networkname`/`passphrase` config fails IFAC in both directions. **verified** live: a stock TCPServerInterface reports 16. Reviews: IF-2, stream-02, DG-7. | S |
| A3 | **A path response is held behind an ingress burst.** RNS exempts destinations in `path_requests` and `discovery_path_requests` for 45 s (`Transport.py` 134, 980-986, 1098-1101, 1815-1821). Retinue exempts only destinations it already has a route to (`router.rs` 146-160). A client that has just attached to a busy hub waits 15 s or more for `request_path`. Review: IF-3. | S |
| A4 | **Interface modes change only route TTL.** RNS has 7 modes with outbound announce rules: AP blocks every announce that has no attached interface, own announces included; ROAMING/BOUNDARY blocks; INTERNAL with `announces_from/to_internal` (`Interface.py` 45-56; `Transport.py` 1458-1521). Retinue's AccessPoint still floods relays and its own announces to AP clients. PointToPoint, Boundary, Gateway and Internal cannot be expressed (`node/params.rs`). The path-request rules (`Transport.py` 3420-3436, 3468, 3505, 3574) wait for recursive discovery. Review: IF-4. | M |
| A5 | **A dropped TCP hub connection is permanent.** The client dials a `SocketAddr` once, with no name resolution, no 5 s connect timeout and no reconnect. EOF calls `forget_interface`, which culls every route (`attach.rs` 43-75, 221-244, 374). There is no keepalive or user-timeout (Linux: 24 s, 5/2/12; i2p: 45 s, 10/9/5), and a peer that stops reading blocks `write_all` forever, with no dead-time teardown. The writer's exit does not end the reader. RNS keeps the interface and its paths, flips `online`, and retries every 5 s (`TCPInterface.py` 80-207, 231-303, 415-446; `Transport.py` 975-978, 1449, 1916; `BackboneInterface.py` 63-69, 411-499). Reviews: IF-7, stream-03, stream-05, stream-09. | M |
| A6 | **Retinue cannot attach to a stock shared instance (rnsd) on Linux**, where it is the abstract AF_UNIX socket `\0rns/<name>`. Over TCP 37428 it also misses: hop transparency (`Transport.py` 1936-1939), the client-side HEADER_2 insertion (1416-1425), the 8 s reconnect, and the re-announce 10 s after reconnect (`LocalInterface.py` 153-202). Review: stream-04. | M |
| A7 | **No UDPInterface.** The carrier is one raw packet per datagram, with optional IFAC. HW_MTU is 1064 and the default IFAC is 16. `device` maps to the subnet broadcast. Each send uses a fresh socket with SO_BROADCAST. The host's own broadcasts loop back, so retinue needs an echo guard for path requests and plain data (its existing announce and link guards are in `router.rs` 122-133 and `dedup.rs`) (`UDPInterface.py` 41-151; `netinfo.py` 57-79). Reviews: DG-1, DG-9. | S |
| A8 | **No AutoInterface.** It needs these pieces, all of which must match byte for byte or stock peers drop tokens at DEBUG level:<br>• Discovery group: `ff<T><S>:0:` followed by SHA-256(group) bytes 2..13. The default is `ff12:0:d70b:fb1c:16e4:5e39:485e:31e1`.<br>• Peering token: SHA-256(group ‖ RFC 5952 link-local text, with KAME scope and `%zone` removed).<br>• Sockets: multicast 29716, unicast 29717 and data 42671.<br>• Timing: tokens every 1.6 s, reverse peering every 5.2 s, 22 s peer timeout.<br>• Self-echo set, so the node does not peer with itself.<br>• One spawned interface per peer, with HW_MTU 1196 and FIXED_MTU.<br>• Interface selection through getifaddrs, using the Darwin and Android skip lists and adopting the last fe80 address.<br>(`AutoInterface.py` 44-710; `netinfo.py` 157-232). Reviews: DG-2, DG-3, DG-4. | L |
| A9 | **No SerialInterface (HDLC) and no KISSInterface (TNC).** Serial needs: speed and 8N1, a 100 ms idle reset of partial frames, a 5 s reopen, and bitrate = speed. KISS needs the startup sequence TXDELAY/TXTAIL/P/SLOTTIME then READY, the port nibble masked, READY flow control with a 5 s unlock, and the zero-padded 15-byte ID beacon (`SerialInterface.py` 63-230; `KISSInterface.py` 38-394). Reviews: SER-2, SER-3. | M |
| A10 | **The tulle RNode host misses RNS config semantics.** Missing pieces:<br>• ALOCK short/long (u16 = percent×100).<br>• Echo validation of BW, TXP and SF, with a ±100 Hz frequency tolerance.<br>• A firmware floor of 1.52.<br>• On detach, RADIO_STATE 0 then LEAVE.<br>• ERROR classification: fatal or recorded.<br>• RESET 0xF8 while online, treated as a reinit.<br>• A 5 s reconnect.<br>• A 100 ms idle reset.<br>RNS's RNode HW_MTU is 508, so a stock 500-byte packet with 8-byte IFAC is dropped by tulle's 500/508 caps (`RNodeInterface.py` 110, 195, 428-500, 619-695, 770, 1076-1209; `tulle/src/rnode.rs` 55-80). Reviews: SER-4, SER-10. | M |
| A11 | **No hardware-free RNode gate.** Every RNode claim rests on headed hardware. `radio-hand::rnode` already answers the device side, so a two-device virtual air over ptys is feasible. It is blocked by tulle's hard DTR/RTS failure on a pty (`tulle/src/serial/link.rs` 47-52). Review: SER-5. | M |

## Priority B: per-interface policy, robustness, carrier features

| # | Finding | Effort |
|---|---|---|
| B1 | **Ingress control is global; RNS's is per interface.** RNS has `ingress_control` and `ic_*` overrides (`Reticulum.py` 904-927, 1066-1078). It charges burst frequency for known announces too, keeps 256 held announces per interface, refuses hops ≥ 127, and releases the fewest-hops entry first (`Interface.py` 189-305). Retinue has one 256-entry FIFO shared by every interface (`announces.rs` 101-120, 273-279) and exempts known announces before charging them (`announce_admission.rs` 208-210). Serial-family interfaces never ingress-limit in RNS. Reviews: IF-5, SER-11. | M |
| B2 | **The destination announce-rate defaults and semantics differ.** RNS uses target 3600 s, grace 5, penalty 0, on transport instances only. It keeps a violation counter with decay, and a blocked announce is learned but not relayed (`Transport.py` 2298-2338; `Reticulum.py` 271-285). Retinue uses 1 s / 0 / 0 with a token model, so it relays about 3600 times more re-announces (`announce_admission.rs` 59-63, 240-276). Review: IF-6. | S |
| B3 | **The endpoint transit proof deadline omits outbound airtime** (`Transport.py` 2059-2062, 3200-3202; `endpoint/transit.rs` 270, whereas the Node core adds it at `node/transit.rs` 241). Review: IF-8. | S |
| B4 | **Inbound processing is one FIFO.** RNS keeps four bounded priority lanes: data 1024, announce 128, path request 128, ingress-limited 8 (`Transport.py` 46-95, 1796-1912). Review: IF-10. | M |
| B5 | **No mapping from RNS `[interfaces]` config to retinue settings.** The keys are mode aliases, IFAC in bits, bitrate ≥ 5, `announce_cap`, `ic_*`, `announce_rate_*`, `gravity`, `outgoing`, and `announces_*_internal` (`Reticulum.py` 843-1060). Review: IF-12. | M |
| B6 | **No tunnel synthesis.** On reconnect RNS sends a 170-byte signed `rnstransport.tunnel.synthesize` packet and gets its paths restored, with an 8 h tunnel lifetime (`TCPInterface.py` 169-180; `Transport.py` 2764-2860). Retinue neither sends it nor honours it. Review: stream-06. | M |
| B7 | **The listener dies on its first `accept()` error**, for example EMFILE or ECONNABORTED (`attach.rs` 273), and there is no handle to close one listener. Review: stream-07. | S |
| B8 | **Spawned connections inherit only IFAC.** RNS copies mode, ingress and announce-rate settings, cap, bitrate and HW_MTU (`TCPInterface.py` 587-670). Spawned ids are never returned, so `set_interface_mode` cannot reach them. Review: stream-08. | S |
| B9 | **Auto duplicate suppression and lifecycle.** Needed: MIF dedup (48 entries, 0.75 s, across all of a parent's peers), carrier echo tracking (6.5 s), and rebinding all three sockets when the link-local changes (`AutoInterface.py` 72-73, 376-482, 649-665). Reviews: DG-5, DG-6. | S-M |
| B10 | **RNode over TCP** (`tcp://host`, port 7633, a DETECT keepalive every 3.5 s) (`RNodeInterface.py` 174-193, 1151-1154, 1452-1582). Review: SER-6. | S |
| B11 | **RNode telemetry is not parsed:** CHTM, PHYPRM, CSMA, BAT and TEMP; the SNR-quality mapping; bitrate computed from the echoes (`RNodeInterface.py` 698-704, 882-1069). Review: SER-7. | S |
| B12 | **RNode and KISS flow control (READY) and the amateur ID beacon are missing.** Ham operators need the beacon for legal station ID (`RNodeInterface.py` 711-744, 1098, 1145; `KISSInterface.py` 270-366). Review: SER-8. | S |
| B13 | **No AX25KISSInterface** (a 16-byte UI header: `APZRNS`, callsign/SSID, 0x03 0xF0) (`AX25KISSInterface.py` 60-348). Review: SER-9. | S |

## Priority C: smaller items

- **C1. Announce cap is fixed at 2%.** RNS allows a per-interface percentage
  in (0, 100] (`Reticulum.py` 286-289; `Transport.py` 1517-1585). IF-11.
- **C2. No per-interface statistics.** Needed: rx/tx bytes and packets,
  announce and path-request counts, `ifac_violations`, `protocol_violations`,
  `held_announces`, `burst_active`, `online` and `mode` (`Interface.py` 98-180;
  `Reticulum.py` 1498-1660). IFAC and decode failures in the TCP reader are not
  counted. IF-13, DG-10.
- **C3. Gravity and `outgoing = False`.** Gravity: a repeated emission that
  arrives on a higher-gravity interface re-points the path
  (`Transport.py` 2225-2251). Retinue's freshness check drops it first.
  `outgoing = False` must also block local transmissions (`Transport.py` 1449).
  IF-14.
- **C4. PipeInterface:** HDLC over a child process's stdio, with respawn.
  stream-10.
- **C5. KISS framing on a TCP client** (`kiss_framing = yes`). stream-11.
- **C6. Backbone fast-flap blocking:** 20 s threshold, grace 5, 12 h block.
  stream-12.
- **C7. Listener and dial address options:** `device`, hostname `listen_ip`,
  `prefer_ipv6`, and fe80 scope. stream-14.
- **C8. LoRa parameter ranges:** SF 5/6 (with the SX126x airtime case) and
  signed TX power. SER-14.

## Later work

- **L1. Per-interface HW_MTU and MTU autoconfiguration.**
  - Stock values: TCP 16384, Backbone 32768, Auto FIXED 1196, Local 524288.
  - The link request signals the initiator's HW_MTU, and each hop clamps it.
  - Needs `Packet::decode_with_limit`, `Iface.hw_mtu`, and link MTU signalling
    up to the minimum along the path.
  - This is efficiency only: interop already holds through the clamps.
  - Sources: `Interface.py` 251-265; `Transport.py` 2063-2088, 2543-2563,
    3173-3178. IF-9, stream-13, DG-8.
- **L2. Serving as the shared instance** for NomadNet, Sideband and MeshChat.
  Needs the local-client role in six routing places. Depends on A6. stream-15.
- **L3. RNodeMultiInterface:** SEL_INT, INTERFACES, per-chip ranges, firmware
  1.74 or later. SER-12.
- **L4. WeaveInterface (WDCL):** discovery, signed handshake, per-endpoint
  peers, dedup. SER-13.
- **L5. RNode BLE transport and framebuffer/display commands.** SER-15.

## Upstream defects not to port

- **Config parsing and defaults:**
  - `Reticulum.py` 859-862: `interface_mode = gateway|gw|internal` without a
    `mode` key raises KeyError. **verified**.
  - `Reticulum.py` 1271-1314: `configured or DEFAULT` ignores a configured 0, so
    `default_ar_target = 0`, `ic_burst_penalty = 0` and
    `ic_held_release_interval = 0` silently revert to the defaults.
- **HW_MTU None with a low bitrate:**
  - `Interface.py` 251-265: an AUTOCONFIGURE interface with `bitrate` below
    62500 gets `HW_MTU None`, and the frame check then raises `None + int`.
    **verified**.
  - `Transport.py` 2073-2076: in the same state, the transit MTU clamp raises
    TypeError.
- **Announce handling:**
  - `Transport.py` 1489-1521: relayed announces on ROAMING and BOUNDARY bypass
    the announce cap.
  - `Interface.py` 296: a released held announce re-enters ingress, is
    double-counted, and can be re-held.
- **TCP:**
  - `TCPInterface.py` 241: the `connect_timeout` config is ignored.
  - `TCPInterface.py` 276-299: `max_reconnect_tries` never stops the client.
  - `TCPInterface.py` 196-207: on macOS, keepalive sets only the idle time,
    leaving about 10 min to detect a dead peer.
  - `TCPInterface.py` 564-570: v6 listener threads are not daemon threads.
- **Frame size and escaping:**
  - `TCPInterface.py` (KISS), `PipeInterface.py`, `SerialInterface.py` 183,
    `KISSInterface.py` 320 and `RNodeInterface.py` 770: oversize frames are
    truncated and still delivered.
  - Dangling escape state carries into the next frame.
- **Backbone:**
  - `BackboneInterface.py` 382-410: an unguarded division can raise
    ZeroDivisionError inside inbound enqueue.
  - `BackboneInterface.py` 287: `listen(1)`.
- **Pipe:**
  - `PipeInterface.py` 146-151: EOF on stdout raises from `ord(b'')`.
  - Writes to a dead child raise into `Transport.transmit`.
- **Auto:**
  - `AutoInterface.py` 383-396: the `peers` dict is iterated without a lock, so
    the race can kill the jobs thread.
  - `AutoInterface.py` 433-451: on a link-local change the old server is never
    closed, which can stall in a rebind loop, and the unicast discovery socket
    is never rebound.
  - `AutoInterface.py` 416-424: an interface with several fe80 addresses flips
    its adopted address every 4 s.
  - `AutoInterface.py` 84: the KAME descope regex misses addresses like
    `fe80:4:0:0:1::`.
  - `AutoInterface.py` 190-201: an unknown `discovery_scope` raises
    AttributeError and panics.
  - `AutoInterface.py` 493 and 507: the announce socket is unbound, so
    non-link scopes may hash the wrong source address.
  - `AutoInterface.py` 342: the data-port bind is not guarded.
  - `AutoInterface.py` 67: en5 is hardcoded for the T2 bridge, but on this host
    it is en6.
- **UDP:** `UDPInterface.py` 119 and 141: a forward-only config has no
  `bind_ip` or `server`, so `__str__` and detach raise.
- **KISS and AX.25:**
  - `KISSInterface.py` 270-281 and 361-365: the padded beacon re-arms itself,
    so it repeats forever.
  - `KISSInterface.py` 246-253: READY is always sent.
  - `AX25KISSInterface.py` 286-298: pads with 0x20 instead of 0x40, and RX
    strips 16 bytes without checking them.
- **RNode:**
  - `RNodeInterface.py` 864 and 878: `ord()` on an int, so STAT_RX and STAT_TX
    raise.
  - `RNodeInterface.py` 816-895: single-byte reports are not unescaped.
  - `RNodeInterface.py` 678-692: CR and ALOCK are not validated.
- **RNodeMulti:**
  - `RNodeMultiInterface.py` 583-618: INTn_DATA is dropped, and the command
    codes collide with FEND and ERROR.
  - `RNodeMultiInterface.py` 1033: a missing txpower raises TypeError.
- **Weave:** `WeaveInterface.py` 725, 971 and 1027: `RNS.LOG`, `return false`,
  and `pop(object)`.

## Checked and equivalent

All of the following match upstream:

- **IFAC:**
  - key derivation, the HKDF salt and the Ed25519 identity;
  - mask and tag layout (`Reticulum.py` 1083-1097; `Transport.py` 1272-1322,
    1689-1717);
  - an IFAC-flagged frame on an interface without IFAC is rejected and counted;
  - per-datagram seal and open is the same for Auto and UDP peers.
- **Framing:**
  - HDLC escaping, flags shared between adjacent frames, empty frames
    (including the shared instance's FLAG FLAG keepalive) ignored, and resync
    after junk;
  - KISS escape rules (`selvage::kiss`, `tulle::kiss`).
- **Routing:**
  - route lifetime by mode: one week, one day, six hours;
  - announce-cap arithmetic and queue: fewest hops first, 3 h life, 4096 per
    interface, local announces and path responses exempt;
  - ingress default values (the algorithm differs: B1);
  - a path response is learned but not relayed;
  - interface removal culls routes and bridges.
- **Link MTU:**
  - the transit link-request MTU clamp, gated by `interop_transit_mtu`;
  - the responder clamps to 500 against FIXED_MTU 1196 peers;
  - the first-hop airtime term in the Node core.
- **TCP:**
  - TCP_NODELAY;
  - spawned sockets inherit the listener's IFAC;
  - per-connection backpressure, through the awaited `router_tx`.
- **Outbound buffering:** the per-class bounded queues stand in for
  TransmitBuffer, with no difference on the wire.
- **RNode:**
  - the init order DETECT, probes, FREQ/BW/TXP/SF/CR, RADIO_STATE;
  - the RX triplet (RSSI − 157, SNR / 4);
  - `radio-hand` echoes FW 1.86, which satisfies stock's validation.
- **Auto:** the per-peer interface lifecycle and mode inheritance.

## Batch plan

**Batch 1 (this split, six worktrees after seam S0):**

- Priority A: A1-A5 and A7-A11.
- Priority B: B1, B2, B3, B7, B8, B9 and B12.
- Priority C: C1 and C3.

**Batch 2, which needs the router seams from batch 1:**

- A6;
- B4, B5, B6, B10, B11 and B13;
- C2 and C4-C8.

L1 to L5 stay deferred.
