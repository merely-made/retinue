//! What the run reports: node names, packet summaries, node state and the trace.

use retinue::hash::AddressHash;
use retinue::packet::{HeaderType, Packet, PacketType};

use super::state::Sim;
use crate::trace::{
    FaceEvent, FaceEventKind, Message, NodeInfo, NodeState, PacketKind, PacketSummary, Profile,
    RouteState, SCHEMA, Trace,
};

impl<'a, const P: usize, const A: usize, const L: usize, const R: usize> Sim<'a, P, A, L, R> {
    pub(super) fn note(&mut self, n: usize, kind: FaceEventKind, text: &str) {
        self.nodes[n].face.event = Some(FaceEvent {
            kind,
            text: text.to_owned(),
        });
    }

    pub(super) fn message_on(&self, link_id: AddressHash) -> Option<u32> {
        self.messages
            .iter()
            .position(|m| m.link == Some(link_id))
            .map(|m| m as u32)
    }

    pub(super) fn name_of_destination(&self, hash: AddressHash) -> String {
        self.nodes
            .iter()
            .find(|n| n.destination == hash)
            .map_or_else(|| hash.to_string(), |n| n.name.clone())
    }

    pub(super) fn name_of_identity(&self, hash: AddressHash) -> String {
        self.nodes
            .iter()
            .find(|n| n.identity == hash)
            .map_or_else(|| hash.to_string(), |n| n.name.clone())
    }

    pub(super) fn summary(&self, packet: &Packet, len: usize) -> PacketSummary {
        PacketSummary {
            packet_type: match packet.packet_type {
                PacketType::Announce => PacketKind::Announce,
                PacketType::LinkRequest => PacketKind::LinkRequest,
                PacketType::Proof => PacketKind::Proof,
                PacketType::Data => PacketKind::Data,
            },
            header: match packet.header_type {
                HeaderType::Type1 => 1,
                HeaderType::Type2 => 2,
            },
            hops: packet.hops,
            destination: packet.destination.to_string(),
            destination_node: self
                .nodes
                .iter()
                .find(|n| n.destination == packet.destination)
                .map(|n| n.name.clone()),
            transport: packet.transport.map(|hash| self.name_of_identity(hash)),
            context: packet.context,
            len: len as u16,
            hash: packet.hash().to_string(),
        }
    }

    pub(super) fn state(&self, t: u64, n: usize) -> NodeState {
        let node = &self.nodes[n];
        let routes = self
            .nodes
            .iter()
            .filter(|other| other.name != node.name)
            .filter_map(|other| {
                node.node
                    .next_hop(other.destination, t)
                    .map(|hop| RouteState {
                        to: other.name.clone(),
                        via: hop.via.map(|hash| self.name_of_identity(hash)),
                        hops: hop.hops,
                    })
            })
            .collect();
        NodeState {
            tx_frames: node.face.tx_frames,
            rx_frames: node.face.rx_frames,
            last_tx_len: node.face.last_tx_len,
            last_rx_len: node.face.last_rx_len,
            links: node.node.link_count() as u32,
            pending_links: node.node.pause_assessment().pending_handshakes as u32,
            peers: node.node.peers().len() as u32,
            routes,
            event: node.face.event.clone(),
        }
    }

    pub(super) fn finish(self) -> Trace {
        let scenario = self.scenario;
        Trace {
            schema: SCHEMA.to_owned(),
            scenario: scenario.name.clone(),
            profile: Profile {
                peers: P as u32,
                actions: A as u32,
                links: L as u32,
                routes: R as u32,
            },
            timing: scenario.timing,
            nodes: self
                .nodes
                .iter()
                .map(|n| NodeInfo {
                    name: n.name.clone(),
                    destination: n.destination.to_string(),
                    identity: n.identity.to_string(),
                    transit: n.transit,
                })
                .collect(),
            edges: scenario.topology.edges.clone(),
            cuts: scenario.cuts.clone(),
            sends: scenario.sends.clone(),
            events: self.events,
            messages: self
                .messages
                .into_iter()
                .enumerate()
                .map(|(i, m)| {
                    let send = &scenario.sends[i];
                    Message {
                        id: i as u32,
                        from: send.from.clone(),
                        to: send.to.clone(),
                        sent_at: send.at,
                        link: m.link.map(|link| link.to_string()),
                        delivered: m.delivered,
                    }
                })
                .collect(),
        }
    }
}
