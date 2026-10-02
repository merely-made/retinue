//! The run: a shared-radio medium, a clock, and the nodes, driven event by event.

use std::collections::{BTreeMap, BTreeSet};

use retinue::announce::{ANNOUNCE_NONCE_LEN, AnnounceBlob};
use retinue::destination::DestinationName;
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::PrivateIdentity;
use retinue::node::{Action, Actions, InterfaceId, Node, TransportConfig};
use retinue::packet::{HeaderType, Packet, PacketType};

use crate::scenario::Scenario;
use crate::trace::{
    Delivery, Effect, Event, FaceEvent, FaceEventKind, Message, NodeInfo, NodeState, Origin,
    PacketKind, PacketSummary, Profile, Refusal, RouteState, SCHEMA, Trace,
};

/// Every node has one radio.
const RADIO: InterfaceId = 0;

/// Why a scenario could not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SimError {
    DuplicateNode(String),
    UnknownNode(String),
    /// A cut names two nodes with no edge between them.
    NoSuchEdge(String, String),
    /// A `Node` call returned more actions than its `ACTIONS` bound held.
    ActionsOverflowed {
        node: String,
        t: u64,
    },
    /// A frame on the medium did not decode.
    Undecodable {
        frame: u32,
    },
    /// A timebase past the announce field's 40 bits.
    Timebase,
}

impl core::fmt::Display for SimError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SimError {}

/// Run with the T114 channel node's table bounds: 32 peers, 8 actions, 4 links, 16 routes.
pub fn run(scenario: &Scenario) -> Result<Trace, SimError> {
    run_with::<32, 8, 4, 16>(scenario)
}

/// Run with caller-chosen `Node` table bounds.
pub fn run_with<
    const PEERS: usize,
    const ACTIONS: usize,
    const LINKS: usize,
    const ROUTES: usize,
>(
    scenario: &Scenario,
) -> Result<Trace, SimError> {
    Sim::<PEERS, ACTIONS, LINKS, ROUTES>::new(scenario)?.run()
}

/// What the face shows that `Node` does not hold: radio counters and the event line.
#[derive(Default)]
struct Face {
    tx_frames: u32,
    rx_frames: u32,
    last_tx_len: Option<u16>,
    last_rx_len: Option<u16>,
    event: Option<FaceEvent>,
}

struct SimNode<const P: usize, const A: usize, const L: usize, const R: usize> {
    name: String,
    node: Node<P, A, L, R>,
    destination: AddressHash,
    identity: AddressHash,
    transit: bool,
    face: Face,
    /// Counters feeding derived announce nonces, link seeds and IVs.
    draws: u32,
}

enum Scheduled {
    Cut(usize),
    Send(usize),
    Poll(usize),
    Deliver { frame: u32, node: usize },
}

struct Frame {
    bytes: Vec<u8>,
    node: usize,
    origin: Origin,
    cause: Option<u32>,
}

struct MessageState {
    link: Option<AddressHash>,
    delivered: Option<Delivery>,
}

struct Sim<'a, const P: usize, const A: usize, const L: usize, const R: usize> {
    scenario: &'a Scenario,
    nodes: Vec<SimNode<P, A, L, R>>,
    neighbours: Vec<Vec<usize>>,
    cut: BTreeSet<(usize, usize)>,
    queue: BTreeMap<(u64, u64), Scheduled>,
    seq: u64,
    frames: Vec<Frame>,
    messages: Vec<MessageState>,
    events: Vec<Event>,
}

/// 64 deterministic bytes from a tag, a node name and a counter.
fn derive64(tag: &[u8], name: &str, counter: u32) -> [u8; 64] {
    let half = |part: u8| {
        let mut input = Vec::with_capacity(tag.len() + name.len() + 6);
        input.extend_from_slice(tag);
        input.push(part);
        input.extend_from_slice(name.as_bytes());
        input.extend_from_slice(&counter.to_le_bytes());
        full_hash(&input)
    };
    let mut out = [0_u8; 64];
    out[..32].copy_from_slice(&half(0));
    out[32..].copy_from_slice(&half(1));
    out
}

fn ordered(a: usize, b: usize) -> (usize, usize) {
    if a < b { (a, b) } else { (b, a) }
}

impl<'a, const P: usize, const A: usize, const L: usize, const R: usize> Sim<'a, P, A, L, R> {
    fn new(scenario: &'a Scenario) -> Result<Self, SimError> {
        let mut nodes: Vec<SimNode<P, A, L, R>> = Vec::new();
        for spec in &scenario.topology.nodes {
            if nodes.iter().any(|n| n.name == spec.name) {
                return Err(SimError::DuplicateNode(spec.name.clone()));
            }
            let identity = PrivateIdentity::from_secret_bytes(&derive64(
                b"retinue-sim/identity",
                &spec.name,
                0,
            ));
            let identity_hash = identity.hash();
            let transport = if spec.transit {
                TransportConfig::transit()
            } else {
                TransportConfig::none()
            };
            let node = Node::new(
                identity,
                DestinationName::new("retinue", ["sim", spec.name.as_str()]).name_hash(),
            )
            .with_announce_interval(scenario.timing.announce_interval)
            .with_transport_config(transport);
            nodes.push(SimNode {
                name: spec.name.clone(),
                destination: node.destination(),
                identity: identity_hash,
                transit: spec.transit,
                node,
                face: Face::default(),
                draws: 0,
            });
        }
        let names: Vec<String> = nodes.iter().map(|n| n.name.clone()).collect();
        let index = |name: &str| {
            names
                .iter()
                .position(|n| n == name)
                .ok_or_else(|| SimError::UnknownNode(name.to_owned()))
        };
        let mut neighbours = vec![Vec::new(); nodes.len()];
        for edge in &scenario.topology.edges {
            let (a, b) = (index(&edge.a)?, index(&edge.b)?);
            if a != b && !neighbours[a].contains(&b) {
                neighbours[a].push(b);
                neighbours[b].push(a);
            }
        }
        for list in &mut neighbours {
            list.sort_unstable();
        }
        let mut sim = Self {
            scenario,
            nodes,
            neighbours,
            cut: BTreeSet::new(),
            queue: BTreeMap::new(),
            seq: 0,
            frames: Vec::new(),
            messages: Vec::new(),
            events: Vec::new(),
        };
        for (i, cut) in scenario.cuts.iter().enumerate() {
            let (a, b) = (index(&cut.a)?, index(&cut.b)?);
            if !sim.neighbours[a].contains(&b) {
                return Err(SimError::NoSuchEdge(cut.a.clone(), cut.b.clone()));
            }
            sim.schedule(cut.at, Scheduled::Cut(i));
        }
        for (i, send) in scenario.sends.iter().enumerate() {
            index(&send.from)?;
            index(&send.to)?;
            sim.schedule(send.at, Scheduled::Send(i));
            sim.messages.push(MessageState {
                link: None,
                delivered: None,
            });
        }
        for n in 0..sim.nodes.len() {
            sim.schedule(0, Scheduled::Poll(n));
        }
        Ok(sim)
    }

    fn schedule(&mut self, at: u64, item: Scheduled) {
        if at <= self.scenario.timing.end {
            self.queue.insert((at, self.seq), item);
            self.seq += 1;
        }
    }

    fn index(&self, name: &str) -> usize {
        self.nodes
            .iter()
            .position(|n| n.name == name)
            .expect("validated at construction")
    }

    fn run(mut self) -> Result<Trace, SimError> {
        while let Some(((t, _), item)) = self.queue.pop_first() {
            match item {
                Scheduled::Cut(i) => self.on_cut(t, i),
                Scheduled::Send(i) => self.on_send(t, i)?,
                Scheduled::Poll(n) => self.on_poll(t, n)?,
                Scheduled::Deliver { frame, node } => self.on_deliver(t, frame, node)?,
            }
        }
        Ok(self.finish())
    }

    fn on_cut(&mut self, t: u64, i: usize) {
        let cut = &self.scenario.cuts[i];
        let (a, b) = (self.index(&cut.a), self.index(&cut.b));
        self.cut.insert(ordered(a, b));
        self.events.push(Event::Cut {
            t,
            a: cut.a.clone(),
            b: cut.b.clone(),
        });
    }

    fn on_poll(&mut self, t: u64, n: usize) -> Result<(), SimError> {
        let blob = if self.nodes[n].node.announce_due(t) {
            let node = &mut self.nodes[n];
            node.draws += 1;
            let seed = derive64(b"retinue-sim/announce-nonce", &node.name, node.draws);
            let mut nonce = [0_u8; ANNOUNCE_NONCE_LEN];
            nonce.copy_from_slice(&seed[..ANNOUNCE_NONCE_LEN]);
            Some(AnnounceBlob::mint(nonce, t / 1_000).map_err(|_| SimError::Timebase)?)
        } else {
            None
        };
        let actions = self.nodes[n].node.poll(t, RADIO, blob.as_ref());
        self.perform(t, n, actions, None)?;
        self.schedule(t + self.scenario.timing.poll_interval, Scheduled::Poll(n));
        Ok(())
    }

    fn on_send(&mut self, t: u64, i: usize) -> Result<(), SimError> {
        let send = &self.scenario.sends[i];
        let (from, to) = (self.index(&send.from), self.index(&send.to));
        let destination = self.nodes[to].destination;
        let node = &mut self.nodes[from];
        node.draws += 1;
        let seed = derive64(b"retinue-sim/link-seed", &node.name, node.draws);
        let hop = node.node.next_hop(destination, t);
        let message = i as u32;
        let Some(actions) = node.node.open_link(destination, RADIO, &seed, t) else {
            let reason = if node.node.peers().knows(destination) {
                Refusal::PendingFull
            } else {
                Refusal::UnknownDestination
            };
            self.events.push(Event::SendRefused {
                t,
                message,
                from: send.from.clone(),
                to: send.to.clone(),
                reason,
            });
            return Ok(());
        };
        let request = actions.iter().find_map(|action| match action {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        });
        self.messages[i].link = request
            .as_ref()
            .and_then(|packet| retinue::link::link_id(packet).ok());
        let via = request
            .as_ref()
            .and_then(|packet| packet.transport)
            .map(|hash| self.name_of_identity(hash));
        self.events.push(Event::Send {
            t,
            message,
            from: send.from.clone(),
            to: send.to.clone(),
            via,
            hops: hop.map(|hop| hop.hops),
        });
        self.perform(t, from, actions, None)
    }

    fn on_deliver(&mut self, t: u64, frame: u32, n: usize) -> Result<(), SimError> {
        let bytes = self.frames[frame as usize].bytes.clone();
        let packet = Packet::decode(&bytes).map_err(|_| SimError::Undecodable { frame })?;
        let node = &mut self.nodes[n];
        node.face.rx_frames = node.face.rx_frames.saturating_add(1);
        node.face.last_rx_len = Some(bytes.len() as u16);
        let actions = node.node.ingest(RADIO, &packet, t);
        self.perform(t, n, actions, Some((frame, packet.hash())))
    }

    /// Carry out one call's actions. `heard` is the frame that produced them, if any.
    fn perform(
        &mut self,
        t: u64,
        n: usize,
        actions: Actions<A>,
        heard: Option<(u32, AddressHash)>,
    ) -> Result<(), SimError> {
        if actions.overflowed() != 0 {
            return Err(SimError::ActionsOverflowed {
                node: self.nodes[n].name.clone(),
                t,
            });
        }
        let mut effects = Vec::new();
        let mut sends = Vec::new();
        let mut links_up = Vec::new();
        let mut deliveries = Vec::new();
        for action in actions {
            match action {
                Action::Send { packet, .. } => sends.push(packet),
                Action::Learned { destination } => effects.push(Effect::Learned {
                    destination: self.name_of_destination(destination),
                }),
                Action::LinkUp { link_id } => {
                    self.note(n, FaceEventKind::Info, "link up");
                    effects.push(Effect::LinkUp {
                        link: link_id.to_string(),
                    });
                    links_up.push(link_id);
                }
                Action::LinkDown { link_id } => {
                    self.note(n, FaceEventKind::Info, "link down");
                    effects.push(Effect::LinkDown {
                        link: link_id.to_string(),
                    });
                }
                Action::Data { link_id, payload } => {
                    let message = self.message_on(link_id);
                    effects.push(Effect::Data {
                        link: link_id.to_string(),
                        len: payload.len() as u32,
                        message,
                    });
                    if let Some(m) = message
                        && self.scenario.sends[m as usize].to == self.nodes[n].name
                        && let Some((frame, _)) = heard
                    {
                        deliveries.push((m, frame));
                    }
                }
                Action::Resource { link_id, data } => effects.push(Effect::Resource {
                    link: link_id.to_string(),
                    len: data.len() as u32,
                }),
            }
        }
        if let Some((frame, _)) = heard {
            let state = self.state(t, n);
            self.events.push(Event::Receive {
                t,
                frame,
                node: self.nodes[n].name.clone(),
                effects,
                state,
            });
        }
        for packet in sends {
            let (origin, cause) = match heard {
                None if packet.packet_type == PacketType::Announce => (Origin::Announce, None),
                None => (Origin::App, None),
                Some((frame, hash)) if packet.hash() == hash => (Origin::Forward, Some(frame)),
                Some((frame, _)) => (Origin::Reply, Some(frame)),
            };
            self.transmit(t, n, packet, origin, cause);
        }
        for (m, frame) in deliveries {
            let path = self.path(frame, n);
            self.messages[m as usize].delivered = Some(Delivery {
                t,
                path: path.clone(),
            });
            self.events.push(Event::Delivered {
                t,
                message: m,
                node: self.nodes[n].name.clone(),
                path,
            });
        }
        for link_id in links_up {
            self.send_payload(t, n, link_id)?;
        }
        Ok(())
    }

    /// The application half: once a send's link is up at its sender, carry the payload.
    fn send_payload(&mut self, t: u64, n: usize, link_id: AddressHash) -> Result<(), SimError> {
        let Some(m) = self.message_on(link_id) else {
            return Ok(());
        };
        if self.scenario.sends[m as usize].from != self.nodes[n].name {
            return Ok(());
        }
        let node = &mut self.nodes[n];
        node.draws += 1;
        let seed = derive64(b"retinue-sim/iv", &node.name, node.draws);
        let mut iv = [0_u8; retinue::token::IV_LEN];
        iv.copy_from_slice(&seed[..retinue::token::IV_LEN]);
        let payload = self.scenario.sends[m as usize].payload.as_bytes();
        if let Some(actions) = node.node.send(link_id, RADIO, payload, &iv) {
            self.perform(t, n, actions, None)?;
        }
        Ok(())
    }

    fn transmit(&mut self, t: u64, n: usize, packet: Packet, origin: Origin, cause: Option<u32>) {
        let bytes = packet.encode();
        let frame = self.frames.len() as u32;
        let mut heard_by = Vec::new();
        let mut blocked = Vec::new();
        for &other in &self.neighbours[n] {
            if self.cut.contains(&ordered(n, other)) {
                blocked.push(other);
            } else {
                heard_by.push(other);
            }
        }
        let node = &mut self.nodes[n];
        node.face.tx_frames = node.face.tx_frames.saturating_add(1);
        node.face.last_tx_len = Some(bytes.len() as u16);
        let summary = self.summary(&packet, bytes.len());
        let state = self.state(t, n);
        self.events.push(Event::Transmit {
            t,
            frame,
            node: self.nodes[n].name.clone(),
            origin,
            cause,
            packet: summary,
            heard_by: heard_by
                .iter()
                .map(|&i| self.nodes[i].name.clone())
                .collect(),
            blocked: blocked
                .iter()
                .map(|&i| self.nodes[i].name.clone())
                .collect(),
            state,
        });
        self.frames.push(Frame {
            bytes,
            node: n,
            origin,
            cause,
        });
        let arrival = t + self.scenario.timing.hop_delay;
        for other in heard_by {
            self.schedule(arrival, Scheduled::Deliver { frame, node: other });
        }
    }

    /// The forwarding chain that carried `frame` to node `n`, sender first.
    fn path(&self, frame: u32, n: usize) -> Vec<String> {
        let mut path = vec![self.nodes[n].name.clone()];
        let mut at = Some(frame);
        while let Some(id) = at {
            let record = &self.frames[id as usize];
            path.push(self.nodes[record.node].name.clone());
            at = match record.origin {
                Origin::Forward => record.cause,
                _ => None,
            };
        }
        path.reverse();
        path
    }

    fn note(&mut self, n: usize, kind: FaceEventKind, text: &str) {
        self.nodes[n].face.event = Some(FaceEvent {
            kind,
            text: text.to_owned(),
        });
    }

    fn message_on(&self, link_id: AddressHash) -> Option<u32> {
        self.messages
            .iter()
            .position(|m| m.link == Some(link_id))
            .map(|m| m as u32)
    }

    fn name_of_destination(&self, hash: AddressHash) -> String {
        self.nodes
            .iter()
            .find(|n| n.destination == hash)
            .map_or_else(|| hash.to_string(), |n| n.name.clone())
    }

    fn name_of_identity(&self, hash: AddressHash) -> String {
        self.nodes
            .iter()
            .find(|n| n.identity == hash)
            .map_or_else(|| hash.to_string(), |n| n.name.clone())
    }

    fn summary(&self, packet: &Packet, len: usize) -> PacketSummary {
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

    fn state(&self, t: u64, n: usize) -> NodeState {
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

    fn finish(self) -> Trace {
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
