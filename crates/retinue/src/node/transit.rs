//! Carrying traffic for other destinations: bridges, the reverse table and relaying.

use super::tables::{LinkBridge, ReverseEntry, is_deduplicated_link_context};
use super::{
    Action, Actions, InterfaceId, LINK_TRANSPORT_IDLE, Node, TRANSPORT_DEDUP_TIMEOUT,
    transit_proof_timeout,
};
use crate::hash::AddressHash;
use crate::link;
use crate::packet::{HeaderType, Packet, PacketType};

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Whether this is a fresh packet for a shared-radio relay. Resource parts and keepalives
    /// legitimately repeat their hash, so they are never filtered, as RNS exempts them
    /// (`Transport.py` 1635-1640). Channel is filtered here, unlike RNS: see N10.
    pub(super) fn transit_is_new(&mut self, packet: &Packet, now: u64) -> bool {
        self.transit_hash_is_new(packet.context, packet.hash(), now)
    }

    /// [`Self::transit_is_new`] for a packet whose hash is already in hand.
    fn transit_hash_is_new(&mut self, context: u8, hash: AddressHash, now: u64) -> bool {
        !is_deduplicated_link_context(context)
            || self
                .transit_filter
                .insert(hash, now, TRANSPORT_DEDUP_TIMEOUT)
    }

    /// Record a link request this node is about to carry, unvalidated until `proof_deadline`.
    /// At capacity the stalest unvalidated bridge makes room, or failing one, a validated
    /// bridge unheard for [`LINK_TRANSPORT_IDLE`]. A recently heard validated link is never
    /// evicted for a request, so forged requests cannot displace carried links; with every
    /// slot holding one, the request is refused. Returns whether it may be carried.
    pub(super) fn remember_bridge(
        &mut self,
        link_id: AddressHash,
        destination: AddressHash,
        from: InterfaceId,
        out: InterfaceId,
        proof_deadline: u64,
        now: u64,
    ) -> bool {
        let bridge = LinkBridge {
            link_id,
            destination,
            from,
            out,
            seen: now,
            proof_deadline: Some(proof_deadline),
        };
        if let Some(existing) = self
            .bridges
            .iter_mut()
            .find(|bridge| bridge.link_id == link_id)
        {
            // A retransmitted request: an unproved bridge follows it, a validated one stands.
            if existing.proof_deadline.is_some() {
                *existing = bridge;
            }
            return true;
        }
        if self.bridges.is_full() {
            let Some(index) = self
                .bridges
                .iter()
                .enumerate()
                .filter(|(_, bridge)| bridge.proof_deadline.is_some())
                .min_by_key(|(_, bridge)| bridge.seen)
                .or_else(|| {
                    self.bridges
                        .iter()
                        .enumerate()
                        .filter(|(_, bridge)| {
                            now.saturating_sub(bridge.seen) >= LINK_TRANSPORT_IDLE
                        })
                        .min_by_key(|(_, bridge)| bridge.seen)
                })
                .map(|(index, _)| index)
            else {
                self.transport_counters.refused_bridges =
                    self.transport_counters.refused_bridges.saturating_add(1);
                return false;
            };
            self.bridges.swap_remove(index);
            self.transport_counters.evicted_bridges =
                self.transport_counters.evicted_bridges.saturating_add(1);
        }
        self.bridges.push(bridge).is_ok()
    }

    /// Relay a packet already associated with a carried link. Link proofs and data name the
    /// link id rather than the original destination, so this lookup precedes normal transit
    /// routing.
    pub(super) fn forward_bridged_packet(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) -> bool {
        if !self.transport.relay_packets {
            return false;
        }
        let Some(index) = self
            .bridges
            .iter()
            .position(|bridge| bridge.link_id == packet.destination)
        else {
            return false;
        };
        let bridge = self.bridges[index];
        let out = if interface == bridge.from {
            bridge.out
        } else if interface == bridge.out {
            bridge.from
        } else {
            // A third interface cannot extend this bridge's lifetime or poison the
            // relay de-duplication cache for a later packet from a real endpoint.
            return true;
        };
        // Nothing crosses a bridge before the destination's proof, and the proof itself
        // must come from the destination's side under its signature. With its identity
        // unknown here, the side check stands alone. The proof validates the bridge only
        // once it is on its way to the initiator.
        let proves = packet.packet_type == PacketType::Proof && packet.context == link::CTX_LRPROOF;
        if proves {
            let signed = interface == bridge.out
                && self
                    .book
                    .resolve(bridge.destination)
                    .is_none_or(|peer| link::proof_is_signed_by(packet, &peer.identity));
            if !signed {
                self.transport_counters.unvalidated_link_packets = self
                    .transport_counters
                    .unvalidated_link_packets
                    .saturating_add(1);
                return true;
            }
        } else if bridge.proof_deadline.is_some() {
            self.transport_counters.unvalidated_link_packets = self
                .transport_counters
                .unvalidated_link_packets
                .saturating_add(1);
            return true;
        }
        if packet.hops >= self.transport.max_hops {
            self.transport_counters.hop_limit_dropped =
                self.transport_counters.hop_limit_dropped.saturating_add(1);
            return true;
        }
        if !self.transit_is_new(packet, now) {
            return true;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        forwarded.header_type = HeaderType::Type1;
        forwarded.transport = None;
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return true;
        }
        if actions.push(Action::Send {
            interface: out,
            packet: forwarded,
        }) {
            if proves {
                self.bridges[index].proof_deadline = None;
            }
            self.bridges[index].seen = now;
            self.transport_counters.forwarded_packets =
                self.transport_counters.forwarded_packets.saturating_add(1);
        }
        true
    }

    /// Carry a header-type-2 packet addressed to this node towards its learned destination.
    pub(super) fn forward_transport_packet(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) -> bool {
        if !self.transport.relay_packets
            || packet.header_type != HeaderType::Type2
            || packet.transport != Some(self.identity.hash())
            || packet.destination == self.destination()
        {
            return false;
        }
        if packet.hops >= self.transport.max_hops {
            self.transport_counters.hop_limit_dropped =
                self.transport_counters.hop_limit_dropped.saturating_add(1);
            return true;
        }
        let Some(route) = self
            .routes
            .iter()
            .find(|route| route.destination == packet.destination)
            .copied()
        else {
            self.transport_counters.unroutable_packets =
                self.transport_counters.unroutable_packets.saturating_add(1);
            return true;
        };
        // One hash serves the transit filter and the reverse entry.
        let hash = packet.hash();
        if !self.transit_hash_is_new(packet.context, hash, now) {
            return true;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        forwarded.header_type = HeaderType::Type1;
        forwarded.transport = None;
        if let Some(next_transport) = route.transport {
            forwarded.header_type = HeaderType::Type2;
            forwarded.transport = Some(next_transport);
        }
        // Every interface here carries `logical_mtu`, so it bounds the link on both sides.
        if packet.packet_type == PacketType::LinkRequest
            && link::clamp_request_mtu(&mut forwarded, self.logical_mtu).is_err()
        {
            self.transport_counters.undecodable_link_requests = self
                .transport_counters
                .undecodable_link_requests
                .saturating_add(1);
            return true;
        }
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return true;
        }
        // Only a request that will be sent takes a bridge slot: an unsent one would hold it,
        // or evict another, until its deadline. A full action list refuses the send below.
        if packet.packet_type == PacketType::LinkRequest
            && actions.len() < ACTIONS
            && let Ok(link_id) = link::link_id(packet)
        {
            let proof_deadline = now
                .saturating_add(transit_proof_timeout(route.hops))
                .saturating_add(self.first_hop_airtime(route.interface));
            if !self.remember_bridge(
                link_id,
                packet.destination,
                interface,
                route.interface,
                proof_deadline,
                now,
            ) {
                return true;
            }
        }
        if actions.push(Action::Send {
            interface: route.interface,
            packet: forwarded,
        }) {
            // A route carrying transit is in use, and RNS refreshes it (`Transport.py` 2113).
            self.touch_route(packet.destination, now);
            if packet.packet_type != PacketType::LinkRequest {
                // RNS's reverse entry, so the packet's proof can come back (`Transport.py`
                // 2104-2110). A link request's bridge was admitted before forwarding.
                self.remember_reverse(hash, interface, route.interface, now);
            }
            self.transport_counters.forwarded_packets =
                self.transport_counters.forwarded_packets.saturating_add(1);
        }
        true
    }

    /// Record the way back for a carried packet's proof (RNS `Transport.py` 2104-2110).
    fn remember_reverse(
        &mut self,
        packet: AddressHash,
        received: InterfaceId,
        outbound: InterfaceId,
        now: u64,
    ) {
        self.reverse.retain(|entry| entry.packet != packet);
        if self.reverse.is_full()
            && let Some(index) = self
                .reverse
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.seen)
                .map(|(index, _)| index)
        {
            self.reverse.swap_remove(index);
        }
        let _ = self.reverse.push(ReverseEntry {
            packet,
            received,
            outbound,
            seen: now,
        });
    }

    /// Carry a delivery proof back along a remembered reverse path. The entry is consumed
    /// either way; a proof arriving on any interface but the one the packet left by is not
    /// carried (RNS `Transport.py` 2733-2744). Returns whether the proof was carried.
    pub(super) fn forward_reverse_proof(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        actions: &mut Actions<ACTIONS>,
    ) -> bool {
        if !self.transport.relay_packets
            || matches!(packet.context, link::CTX_LRPROOF | link::CTX_RESOURCE_PRF)
        {
            return false;
        }
        let Some(index) = self
            .reverse
            .iter()
            .position(|entry| entry.packet == packet.destination)
        else {
            return false;
        };
        let entry = self.reverse.swap_remove(index);
        if interface != entry.outbound {
            return false;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return true;
        }
        if actions.push(Action::Send {
            interface: entry.received,
            packet: forwarded,
        }) {
            self.transport_counters.forwarded_packets =
                self.transport_counters.forwarded_packets.saturating_add(1);
        }
        true
    }

    /// Re-broadcast a verified announce with this node recorded as the transport hop.
    pub(super) fn relay_announce(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        destination: AddressHash,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // A path response answers one requester. RNS learns from it but never queues it for
        // rebroadcast, so one path request cannot flood the mesh.
        if !self.transport.relay_announces
            || destination == self.destination()
            || packet.context == crate::path::CTX_PATH_RESPONSE
        {
            return;
        }
        if packet.hops >= self.transport.max_hops {
            self.transport_counters.hop_limit_dropped =
                self.transport_counters.hop_limit_dropped.saturating_add(1);
            return;
        }
        if !self.transit_is_new(packet, now) {
            return;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        forwarded.header_type = HeaderType::Type2;
        forwarded.transport = Some(self.identity.hash());
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return;
        }
        if actions.push(Action::Send {
            interface,
            packet: forwarded,
        }) {
            self.transport_counters.forwarded_announces = self
                .transport_counters
                .forwarded_announces
                .saturating_add(1);
        }
    }
}
