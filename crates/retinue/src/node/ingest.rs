//! Inbound dispatch, announce learning and path requests.

use super::{Action, Actions, InterfaceId, Node};
use crate::address_book::Ingested;
use crate::announce::Announce;
use crate::announce_freshness::{
    AnnounceFreshnessCandidate, AnnounceFreshnessDecision, AnnounceFreshnessReject,
};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType};
use crate::path::PathRequest;

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Feed a received packet in.
    ///
    /// Anything malformed, unsigned, or not addressed to work this node does is dropped
    /// silently, exactly as the desktop drops it: a peer must not be able to make a board
    /// spend memory by sending rubbish.
    pub fn ingest(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
    ) -> Actions<ACTIONS> {
        let mut actions = Actions::new();

        // IFAC is the interface's envelope: `Ifac::open` strips the flag, so a packet still
        // carrying it was decoded raw off an interface without IFAC. RNS drops those.
        if packet.ifac {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return actions;
        }

        if packet.encoded_len() > self.payload_limits.max_ingress_bytes {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return actions;
        }

        // RNS's packet filter, ahead of any dispatch (`Transport.py` 1629-1660): a
        // header-type-2 packet is carried only by the transport it names, and a PLAIN or GROUP
        // packet never travels past its first hop.
        if packet.packet_type != PacketType::Announce
            && ((packet.header_type == HeaderType::Type2
                && packet.transport != Some(self.identity.hash()))
                || (matches!(
                    packet.destination_type,
                    DestinationType::Plain | DestinationType::Group
                ) && packet.hops > 1))
        {
            self.count_filtered();
            return actions;
        }

        if let Some(request) = PathRequest::parse(packet) {
            self.on_path_request(interface, &request, now, &mut actions);
            return actions;
        }

        self.expire_transport_state(now);
        if packet.packet_type == PacketType::Proof
            && self.forward_reverse_proof(interface, packet, &mut actions)
        {
            return actions;
        }
        if packet.packet_type != PacketType::Announce
            && (self.forward_bridged_packet(interface, packet, now, &mut actions)
                || self.forward_transport_packet(interface, packet, now, &mut actions))
        {
            return actions;
        }

        match packet.packet_type {
            PacketType::Announce => {
                // A relay rebroadcasting our own announce echoes it back to us. We are not
                // our own peer, and the echo carries nothing new: drop it before it costs a
                // signature check or touches the peer, freshness or route state.
                if packet.destination == self.destination() {
                    return actions;
                }
                // `Announce::decode` verifies the signature and that the destination hash
                // matches the announced identity.
                if let Ok(announce) = Announce::decode(packet) {
                    // A known destination announced under another key is rejected outright,
                    // before freshness, routes or relaying (RNS `Identity.validate_announce`).
                    if self.book.key_conflicts(&announce) {
                        self.transport_counters.key_mismatch_announces = self
                            .transport_counters
                            .key_mismatch_announces
                            .saturating_add(1);
                        return actions;
                    }
                    let candidate = AnnounceFreshnessCandidate {
                        destination: announce.destination,
                        blob: crate::announce::AnnounceBlob::from_wire(announce.rand_hash),
                    };
                    // Freshness belongs to the route. A destination without a live one is a
                    // first sighting, as after an RNS path cull.
                    let route_live = self.routes.iter().any(|route| {
                        route.destination == announce.destination
                            && route.live(now, self.transport.route_ttl)
                    });
                    let accepted = match self.freshness.evaluate(candidate, route_live) {
                        AnnounceFreshnessDecision::Accept(accepted) => accepted,
                        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay) => {
                            self.transport_counters.replayed_announces =
                                self.transport_counters.replayed_announces.saturating_add(1);
                            return actions;
                        }
                        AnnounceFreshnessDecision::Reject(
                            AnnounceFreshnessReject::StaleTimebase,
                        ) => {
                            self.transport_counters.stale_announces =
                                self.transport_counters.stale_announces.saturating_add(1);
                            return actions;
                        }
                    };

                    // The book makes room by evicting the least recently heard peer with no
                    // live route. Links and pending requests carry their own copy of the
                    // peer's keys, so they do not need the entry. A refusal (every peer
                    // routed) only keeps the identity out of the book: route learning and
                    // relaying follow the route table, as RNS relays from its path table.
                    let route_ttl = self.transport.route_ttl;
                    let routes = &self.routes;
                    let admitted = self.book.ingest_at(&announce, now, |destination| {
                        routes.iter().any(|route| {
                            route.destination == destination && route.live(now, route_ttl)
                        })
                    }) != Ingested::Refused;
                    if !admitted {
                        self.refused_peers = self.refused_peers.saturating_add(1);
                    }

                    let record = self.freshness.record_accepted(candidate, accepted);
                    if let Some(evicted) = record.evicted_destination {
                        // A route never outlives its freshness row.
                        self.routes.retain(|route| route.destination != evicted);
                        self.transport_counters.evicted_freshness_rows = self
                            .transport_counters
                            .evicted_freshness_rows
                            .saturating_add(1);
                    }
                    self.transport_counters.evicted_freshness_blobs = self
                        .transport_counters
                        .evicted_freshness_blobs
                        .saturating_add(u16::from(record.evicted_blob.is_some()));

                    // Every node learns routes, as `Endpoint` learns paths whatever its
                    // policy: a leaf needs one to address its first relay. Only the
                    // transport policy decides whether this node forwards.
                    self.learn_route(
                        announce.destination,
                        interface,
                        packet.hops,
                        packet.transport,
                        now,
                    );
                    if admitted {
                        actions.push(Action::Learned {
                            destination: announce.destination,
                        });
                    }
                    self.relay_announce(interface, packet, announce.destination, now, &mut actions);
                }
            }
            PacketType::LinkRequest => self.on_link_request(interface, packet, now, &mut actions),
            PacketType::Proof => self.on_proof(interface, packet, now, &mut actions),
            PacketType::Data => self.on_link_data(interface, packet, now, &mut actions),
        }

        actions
    }

    fn count_filtered(&mut self) {
        self.transport_counters.filtered_packets =
            self.transport_counters.filtered_packets.saturating_add(1);
    }

    /// A path request. RNS ignores a tagless one and a repeat of a target and tag it has seen
    /// (`Transport.py` 1838-1856), and answers for a local destination with an announce on the
    /// requesting interface alone (`Transport.py` 3452-3456). This node answers only for
    /// itself: it keeps no announces to answer for others from.
    fn on_path_request(
        &mut self,
        interface: InterfaceId,
        request: &PathRequest,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        let fresh = request
            .unique_tag()
            .is_some_and(|tag| self.path_request_tags.insert(tag, now, u64::MAX));
        if !fresh {
            self.count_filtered();
            return;
        }
        if request.target != self.destination() {
            return;
        }
        let Some(blob) = self.announced_blob else {
            return;
        };
        match self.try_announce(&blob, None) {
            Ok(mut packet) => {
                packet.context = crate::path::CTX_PATH_RESPONSE;
                actions.push(Action::Send { interface, packet });
            }
            Err(_) => self.refused_payloads = self.refused_payloads.saturating_add(1),
        }
    }
}
