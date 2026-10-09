//! The receive pipeline: dedup, dispatch and forwarding.

use alloc::vec::Vec;

use super::{DirectRoute, Event, Node};
use crate::advert::Advert;
use crate::mesh::{Forward, route_recv};
use crate::message::{TextMessage, decode_ack};
use crate::packet::{Packet, PacketRef, ROUTE_DIRECT, ROUTE_FLOOD, payload_type};
use crate::path::PathMessage;

impl Node {
    /// Handle one received raw frame. Returns `(events for the app, frames to retransmit)`.
    pub fn on_frame(&mut self, frame: &[u8]) -> (Vec<Event>, Vec<Vec<u8>>) {
        // At most one app event and two outbound frames (a PATH reply plus
        // forwarding) arise from one frame, so neither vector grows here.
        let mut events = Vec::with_capacity(1);
        let mut out = Vec::with_capacity(2);

        // Validate bounds and split the raw frame before allocating an owned
        // packet. Malformed or oversized frames do not touch resident state.
        let Some(raw) = PacketRef::decode(frame) else {
            return (events, out);
        };
        // This node implements V1 payloads in an unscoped mesh. Keep future
        // payload formats and transport scopes opaque at the codec layer;
        // neither may learn contacts, poison dedup, or cross into this realm.
        if raw.header >> 6 != 0 || raw.has_transport_codes() {
            return (events, out);
        }
        let packet = raw.to_owned();
        // A direct packet is processed only by its current next hop. Other radios hear the
        // same transmission but must not mark it seen before it reaches their turn in the
        // source route.
        let is_our_direct_hop = packet.path_hop_count() == 0
            || self
                .me
                .hash_matches(&packet.path[..packet.path_hop_size() as usize]);
        if !packet.is_flood() && !is_our_direct_hop {
            return (events, out);
        }
        if self.seen.has_seen(&packet) {
            return (events, out);
        }

        // An intermediate hop forwards the route. Payload delivery belongs to
        // the endpoint reached after every source-route entry is consumed.
        let consumed = if packet.is_flood() || packet.path_hop_count() == 0 {
            self.dispatch(&packet, &mut events, &mut out)
        } else {
            false
        };

        if let Forward::Retransmit(fwd) =
            route_recv(&packet, &self.me, self.allow_forward, consumed)
        {
            out.push(fwd.encode());
        }
        (events, out)
    }

    /// Process a packet by payload type, pushing any events. Returns whether it was consumed by
    /// this node (addressed to us and handled), which suppresses re-forwarding.
    fn dispatch(
        &mut self,
        packet: &Packet,
        events: &mut Vec<Event>,
        out: &mut Vec<Vec<u8>>,
    ) -> bool {
        match packet.payload_type() {
            payload_type::ADVERT => {
                if let Some(adv) = Advert::decode(&packet.payload) {
                    if self.try_add_contact(adv.identity.clone()).is_err() {
                        return false;
                    }
                    events.push(Event::Advert {
                        identity: adv.identity,
                        timestamp: adv.timestamp,
                        app_data: adv.app_data,
                    });
                }
                false // adverts flood onward
            }
            payload_type::TXT_MSG => {
                // Addressed to us? payload = dest_hash(1) || src_hash(1) || blob.
                if packet.payload.first() != Some(&self.my_hash()) {
                    return false; // not ours: forward it
                }
                let Some(&src_hash) = packet.payload.get(1) else {
                    return false;
                };
                let Some(sender) = self
                    .contacts
                    .iter()
                    .find(|(key, _)| *key == src_hash)
                    .map(|(_, c)| c.identity.clone())
                else {
                    // We do not know the sender yet, so we cannot derive the key. Let it forward
                    // in case another node can (and we may learn the sender's advert later).
                    return false;
                };
                let Some(secret) = self.identity.shared_secret(&sender) else {
                    return false;
                };
                match TextMessage::decode(&packet.payload, &secret) {
                    Some((_, _, message)) => {
                        let ack = message.ack_crc(&sender.pub_key);
                        events.push(Event::Message {
                            from: src_hash,
                            message,
                            ack,
                        });
                        if packet.is_flood()
                            && let Some(frame) = self.path_frame(
                                src_hash,
                                packet.path_len,
                                &packet.path,
                                payload_type::ACK,
                                &ack,
                                None,
                            )
                        {
                            out.push(frame);
                        }
                        true // ours, handled
                    }
                    None => false,
                }
            }
            payload_type::ACK => {
                if let Some(ack) = decode_ack(&packet.payload) {
                    events.push(Event::Ack(ack));
                }
                false // acks flood to whoever awaits them
            }
            payload_type::PATH => self.dispatch_path(packet, events, out),
            _ => false,
        }
    }

    fn dispatch_path(
        &mut self,
        packet: &Packet,
        events: &mut Vec<Event>,
        out: &mut Vec<Vec<u8>>,
    ) -> bool {
        if packet.payload.first() != Some(&self.my_hash()) {
            return false;
        }
        let Some(&src_hash) = packet.payload.get(1) else {
            return false;
        };
        let Some(sender) = self
            .contacts
            .iter()
            .find(|(key, _)| *key == src_hash)
            .map(|(_, c)| c.identity.clone())
        else {
            return false;
        };
        let Some(secret) = self.identity.shared_secret(&sender) else {
            return false;
        };
        let Some((dest, src, path)) = PathMessage::decode(&packet.payload, &secret) else {
            return false;
        };
        if dest != self.my_hash() || src != src_hash {
            return false;
        }

        let route = DirectRoute {
            path_len: path.path_len,
            path: path.path.clone(),
        };
        if let Some((_, contact)) = self.contacts.iter_mut().find(|(key, _)| *key == src_hash) {
            contact.route = Some(route.clone());
        }
        if path.extra_type == payload_type::ACK
            && let Some(ack) = decode_ack(&path.extra)
        {
            events.push(Event::Ack(ack));
        }

        if packet.is_flood()
            && let Some(frame) = self.path_frame(
                src_hash,
                packet.path_len,
                &packet.path,
                0,
                &[],
                Some(&route),
            )
        {
            out.push(frame);
        }
        true
    }

    fn path_frame(
        &mut self,
        to: u8,
        path_len: u8,
        path: &[u8],
        extra_type: u8,
        extra: &[u8],
        direct: Option<&DirectRoute>,
    ) -> Option<Vec<u8>> {
        let peer = self
            .contacts
            .iter()
            .find(|(key, _)| *key == to)?
            .1
            .identity
            .clone();
        let secret = self.identity.shared_secret(&peer)?;
        let message = PathMessage::new(path_len, path, extra_type, extra)?;
        let payload = message.encode(&secret, to, self.my_hash())?;
        let mut packet = Packet::new(
            if direct.is_some() {
                ROUTE_DIRECT
            } else {
                ROUTE_FLOOD
            },
            payload_type::PATH,
        );
        packet.payload = payload;
        if let Some(route) = direct {
            packet.path_len = route.path_len;
            packet.path = route.path.clone();
        }
        Some(self.seal_outgoing(&mut packet))
    }
}
