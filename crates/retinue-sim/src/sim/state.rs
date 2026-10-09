//! The run's state: nodes, the medium's frames, the event queue and message records.

use std::collections::{BTreeMap, BTreeSet};

use retinue::destination::DestinationName;
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::PrivateIdentity;
use retinue::node::{Node, TransportConfig};

use super::SimError;
use crate::scenario::Scenario;
use crate::trace::{Delivery, Event, FaceEvent, Origin};

/// What the face shows that `Node` does not hold: radio counters and the event line.
#[derive(Default)]
pub(super) struct Face {
    pub(super) tx_frames: u32,
    pub(super) rx_frames: u32,
    pub(super) last_tx_len: Option<u16>,
    pub(super) last_rx_len: Option<u16>,
    pub(super) event: Option<FaceEvent>,
}

pub(super) struct SimNode<const P: usize, const A: usize, const L: usize, const R: usize> {
    pub(super) name: String,
    pub(super) node: Node<P, A, L, R>,
    pub(super) destination: AddressHash,
    pub(super) identity: AddressHash,
    pub(super) transit: bool,
    pub(super) face: Face,
    /// Counters feeding derived announce nonces, link seeds and IVs.
    pub(super) draws: u32,
}

pub(super) enum Scheduled {
    Cut(usize),
    Send(usize),
    Poll(usize),
    Deliver { frame: u32, node: usize },
}

pub(super) struct Frame {
    pub(super) bytes: Vec<u8>,
    pub(super) node: usize,
    pub(super) origin: Origin,
    pub(super) cause: Option<u32>,
}

pub(super) struct MessageState {
    pub(super) link: Option<AddressHash>,
    pub(super) delivered: Option<Delivery>,
}

pub(super) struct Sim<'a, const P: usize, const A: usize, const L: usize, const R: usize> {
    pub(super) scenario: &'a Scenario,
    pub(super) nodes: Vec<SimNode<P, A, L, R>>,
    pub(super) neighbours: Vec<Vec<usize>>,
    pub(super) cut: BTreeSet<(usize, usize)>,
    pub(super) queue: BTreeMap<(u64, u64), Scheduled>,
    pub(super) seq: u64,
    pub(super) frames: Vec<Frame>,
    pub(super) messages: Vec<MessageState>,
    pub(super) events: Vec<Event>,
}

/// 64 deterministic bytes from a tag, a node name and a counter.
pub(super) fn derive64(tag: &[u8], name: &str, counter: u32) -> [u8; 64] {
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

pub(super) fn ordered(a: usize, b: usize) -> (usize, usize) {
    if a < b { (a, b) } else { (b, a) }
}

impl<'a, const P: usize, const A: usize, const L: usize, const R: usize> Sim<'a, P, A, L, R> {
    pub(super) fn new(scenario: &'a Scenario) -> Result<Self, SimError> {
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
            let name = DestinationName::try_new("retinue", ["sim", spec.name.as_str()])
                .ok_or_else(|| SimError::BadNodeName(spec.name.clone()))?;
            let node = Node::new(identity, name.name_hash())
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

    pub(super) fn schedule(&mut self, at: u64, item: Scheduled) {
        if at <= self.scenario.timing.end {
            self.queue.insert((at, self.seq), item);
            self.seq += 1;
        }
    }

    pub(super) fn index(&self, name: &str) -> usize {
        self.nodes
            .iter()
            .position(|n| n.name == name)
            .expect("validated at construction")
    }
}
