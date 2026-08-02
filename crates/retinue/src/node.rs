//! The executor-neutral node: the shape a board runs.
//!
//! [`Endpoint`](crate::endpoint) is the desktop shell. It owns tokio tasks, unbounded
//! channels, sockets and a clock, none of which a 256 KB board has. This is the same
//! protocol work with the shell removed:
//!
//! ```text
//! node.ingest(interface, packet, now) -> Actions
//! node.poll(now)                      -> Actions
//! ```
//!
//! Nothing here reads a clock, allocates without a bound, or performs I/O. Time arrives as
//! a `now` argument, entropy arrives as caller-supplied bytes (the same discipline
//! [`announce::build`](crate::announce::build) already follows so fixtures reproduce byte
//! for byte), and everything the node wants to happen leaves as an [`Action`] for a shell
//! to carry out. That is what makes it testable at a desk and runnable under embassy
//! without either knowing about the other.
//!
//! # Why this is not a second implementation
//!
//! The node calls the same `announce`, `link`, `channel` and `resource` code the desktop
//! calls, at the small capacity profile instead of the large one. If it re-implemented any
//! of that, `Endpoint` would stop being an oracle for the board and become a different
//! program that merely interoperates. See the plan's structural decision 1.

use alloc::vec::Vec;

use heapless::Vec as BoundedVec;

use crate::address_book::{AddressBook, Ingested};
use crate::announce::{self, Announce, RAND_HASH_LEN, RATCHET_LEN};
use crate::hash::{AddressHash, NameHash};
use crate::identity::PrivateIdentity;
use crate::link::{self, Inbound, Link, LinkMode, LinkTrailer, PendingLink};
use crate::packet::{Packet, PacketType};

/// Which interface a packet arrived on or should leave by.
///
/// A plain integer chosen by the shell, matching the desktop's `InterfaceId`, so a board
/// with one radio and one host link can simply number them.
pub type InterfaceId = u32;

/// Something the node wants the shell to do.
///
/// The node never acts; it decides. A shell reads these and performs them with whatever
/// radio, timer and link it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Put this packet on the wire, by this interface.
    Send {
        interface: InterfaceId,
        packet: Packet,
    },
    /// A destination was learned or refreshed from a valid announce. The shell may show it
    /// on a face or hand it to an application; the node has already recorded it.
    Learned { destination: AddressHash },
    /// A link is established, in either direction. The shell may now carry data on it.
    LinkUp { link_id: AddressHash },
    /// A link ended, because the peer closed it or the node dropped it.
    LinkDown { link_id: AddressHash },
    /// Application bytes arrived on a link, already decrypted.
    Data {
        link_id: AddressHash,
        payload: Vec<u8>,
    },
}

/// What one `ingest` or `poll` produced.
///
/// Bounded, because a single call must never be able to demand unbounded work of a shell
/// that has 256 KB. `overflowed` reports honestly when the bound was reached rather than
/// silently dropping, per the plan's rule that a full table stays operational and says so.
#[derive(Debug)]
pub struct Actions<const N: usize> {
    items: BoundedVec<Action, N>,
    overflowed: u16,
}

impl<const N: usize> Actions<N> {
    fn new() -> Self {
        Self {
            items: BoundedVec::new(),
            overflowed: 0,
        }
    }

    fn push(&mut self, action: Action) {
        if self.items.push(action).is_err() {
            self.overflowed = self.overflowed.saturating_add(1);
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Action> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Actions that did not fit. Nonzero means the shell is not draining fast enough, or
    /// `ACTIONS` is too small for this traffic.
    pub fn overflowed(&self) -> u16 {
        self.overflowed
    }
}

impl<const N: usize> IntoIterator for Actions<N> {
    type Item = Action;
    type IntoIter = <BoundedVec<Action, N> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

/// How often this node re-announces itself, in the caller's tick unit.
///
/// A board on a shared band should not announce often; this is a starting cadence a shell
/// can override, not a protocol constant.
pub const DEFAULT_ANNOUNCE_INTERVAL: u64 = 600_000;

/// The link MTU this node offers.
///
/// 255, the SX1262's frame size, because the trunk is retinue-to-retinue over direct PHY.
/// Carrying stock RNS's 500 over the air needs the long-packet fragmentation lane, which
/// belongs to the RNode personality; see the plan's pressure point 4.
pub const LINK_MTU: u32 = 255;

/// An executor-neutral Reticulum node.
///
/// `PEERS` bounds the address book. `ACTIONS` bounds what one call can ask of the shell.
/// Both default to the board profile, because the desktop has `Endpoint` and does not want
/// this type.
pub struct Node<const PEERS: usize = 32, const ACTIONS: usize = 8, const LINKS: usize = 4> {
    identity: PrivateIdentity,
    /// The destination this node announces. One for now: a board is one thing.
    name_hash: NameHash,
    book: AddressBook,
    /// Application data carried in our announces.
    app_data: Vec<u8>,
    /// When we last announced, and how often to. `None` until the first poll, so a node
    /// announces promptly on boot rather than waiting a full interval.
    last_announce: Option<u64>,
    announce_interval: u64,
    /// Established links, each with the proof that established it.
    ///
    /// The proof is kept so a retransmitted request is answered with the *same* proof
    /// rather than establishing a second link. On a medium that drops, the peer not hearing
    /// our proof is ordinary, and answering twice would leave the two sides holding
    /// different keys for what the initiator thinks is one link.
    links: BoundedVec<(Link, Packet), LINKS>,
    /// Links we opened, awaiting the peer's proof.
    pending: BoundedVec<PendingLink, LINKS>,
    /// Link requests refused because the table was full. Visible rather than silent.
    refused_links: u16,
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize> Node<PEERS, ACTIONS, LINKS> {
    /// A node with an identity and the destination it answers to.
    pub fn new(identity: PrivateIdentity, name_hash: NameHash) -> Self {
        Self {
            identity,
            name_hash,
            book: AddressBook::with_max_peers(PEERS),
            app_data: Vec::new(),
            last_announce: None,
            announce_interval: DEFAULT_ANNOUNCE_INTERVAL,
            links: BoundedVec::new(),
            pending: BoundedVec::new(),
            refused_links: 0,
        }
    }

    /// Set the application data carried in our announces.
    pub fn with_app_data(mut self, app_data: &[u8]) -> Self {
        self.app_data.clear();
        self.app_data.extend_from_slice(app_data);
        self
    }

    /// Set the re-announce cadence, in the caller's tick unit.
    pub fn with_announce_interval(mut self, interval: u64) -> Self {
        self.announce_interval = interval;
        self
    }

    /// This node's own destination hash: what a peer addresses to reach it.
    pub fn destination(&self) -> AddressHash {
        crate::destination::destination_hash(self.name_hash, self.identity.hash())
    }

    /// The peers this node has heard announce.
    pub fn peers(&self) -> &AddressBook {
        &self.book
    }

    /// Established links.
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    /// Whether a link with this id is established.
    pub fn has_link(&self, link_id: AddressHash) -> bool {
        self.links.iter().any(|(link, _)| link.id() == link_id)
    }

    /// Link requests refused because the table was full. Nonzero means `LINKS` is too small
    /// for the traffic this node sees, and peers are being turned away.
    pub fn refused_links(&self) -> u16 {
        self.refused_links
    }

    /// Open a link to a destination this node has heard announce.
    ///
    /// `ephemeral_seed` is caller-supplied, per attempt, for the same reason every other
    /// key here is: no RNG in the protocol layer. Returns `None` if the peer is unknown or
    /// the pending table is full.
    pub fn open_link(
        &mut self,
        destination: AddressHash,
        interface: InterfaceId,
        ephemeral_seed: &[u8; 64],
    ) -> Option<Actions<ACTIONS>> {
        let peer = self.book.resolve(destination)?.identity;
        if self.pending.is_full() {
            self.refused_links = self.refused_links.saturating_add(1);
            return None;
        }

        let (attempt, request) = PendingLink::open(
            destination,
            peer,
            ephemeral_seed,
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: LINK_MTU,
            },
        );
        let _ = self.pending.push(attempt);

        let mut actions = Actions::new();
        actions.push(Action::Send {
            interface,
            packet: request,
        });
        Some(actions)
    }

    /// Send application bytes on an established link.
    ///
    /// `iv` is caller-supplied and must not repeat for a link's key.
    pub fn send(
        &self,
        link_id: AddressHash,
        interface: InterfaceId,
        payload: &[u8],
        iv: &[u8; crate::token::IV_LEN],
    ) -> Option<Actions<ACTIONS>> {
        let (link, _) = self.links.iter().find(|(l, _)| l.id() == link_id)?;
        let mut actions = Actions::new();
        actions.push(Action::Send {
            interface,
            packet: link.data_packet(payload, iv),
        });
        Some(actions)
    }

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
        let _ = now;

        match packet.packet_type {
            PacketType::Announce => {
                // `Announce::decode` verifies the signature and that the destination hash
                // matches the announced identity, so an entry can only come from an
                // announce whose maths checked out. The invalid fixtures are the proof.
                if let Ok(announce) = Announce::decode(packet)
                    && self.book.ingest(&announce) != Ingested::Refused
                {
                    actions.push(Action::Learned {
                        destination: announce.destination,
                    });
                }
            }
            PacketType::LinkRequest => self.on_link_request(interface, packet, &mut actions),
            PacketType::Proof => self.on_proof(packet, &mut actions),
            PacketType::Data => self.on_link_data(packet, &mut actions),
        }

        actions
    }

    /// A peer wants a link to us.
    fn on_link_request(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        actions: &mut Actions<ACTIONS>,
    ) {
        // Only for the destination this node answers to. Anything else is not ours, and a
        // board is not a transport node.
        if packet.destination != self.destination() {
            return;
        }
        let Ok(id) = link::link_id(packet) else {
            return;
        };

        // Already established: the peer did not hear our proof, so send the same one again.
        // A fresh accept here would give the two sides different keys for one link.
        if let Some((_, proof)) = self.links.iter().find(|(link, _)| link.id() == id) {
            actions.push(Action::Send {
                interface,
                packet: proof.clone(),
            });
            return;
        }

        if self.links.is_full() {
            self.refused_links = self.refused_links.saturating_add(1);
            return;
        }

        // The responder's ephemeral seed is derived rather than random, because this layer
        // holds no RNG. It is bound to the link id and our identity, so it differs per
        // request and cannot be predicted without our private key.
        let seed = self.responder_seed(&id);
        let offered = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: LINK_MTU,
        };
        if let Ok((link, proof)) = link::accept(packet, &self.identity, &seed, offered) {
            let link_id = link.id();
            let _ = self.links.push((link, proof.clone()));
            actions.push(Action::Send {
                interface,
                packet: proof,
            });
            actions.push(Action::LinkUp { link_id });
        }
    }

    /// A proof for a link we opened.
    fn on_proof(&mut self, packet: &Packet, actions: &mut Actions<ACTIONS>) {
        let Some(index) = self
            .pending
            .iter()
            .position(|attempt| attempt.prove(packet).is_ok())
        else {
            return;
        };
        let attempt = self.pending.swap_remove(index);
        let Ok(link) = attempt.prove(packet) else {
            return;
        };
        if self.links.is_full() {
            self.refused_links = self.refused_links.saturating_add(1);
            return;
        }
        let link_id = link.id();
        // Our own proof has no place here: this side was the initiator, so there is nothing
        // to re-send. The stored packet is the proof we received, kept only for symmetry.
        let _ = self.links.push((link, packet.clone()));
        actions.push(Action::LinkUp { link_id });
    }

    /// Traffic on an established link.
    fn on_link_data(&mut self, packet: &Packet, actions: &mut Actions<ACTIONS>) {
        let Some(index) = self
            .links
            .iter()
            .position(|(link, _)| link.id() == packet.destination)
        else {
            return;
        };

        match self.links[index].0.receive(packet) {
            Some(Inbound::Data(payload)) => actions.push(Action::Data {
                link_id: packet.destination,
                payload,
            }),
            Some(Inbound::Close) => {
                let link_id = self.links[index].0.id();
                self.links.swap_remove(index);
                actions.push(Action::LinkDown { link_id });
            }
            // Keepalives, RTT, requests, responses and anything unrecognised are not this
            // gate's work. They are dropped rather than mishandled, and the boundary is
            // pinned by a test so the next gate's work shows up as a change.
            _ => {}
        }
    }

    /// A responder ephemeral seed, derived from our identity and the link id.
    ///
    /// This layer has no RNG, and an initiator supplies its own seed from the shell. A
    /// responder answers packets it did not ask for, so it cannot be handed one per
    /// request without threading entropy through every ingest. Deriving it keeps the
    /// forward secrecy that matters (the seed is unpredictable without our private key)
    /// and makes a retransmitted request reproduce the same proof.
    fn responder_seed(&self, link_id: &AddressHash) -> [u8; 64] {
        let secret = self.identity.to_secret_bytes();
        let half = |tag: &[u8]| {
            let mut input = Vec::with_capacity(tag.len() + secret.len() + 16);
            input.extend_from_slice(tag);
            input.extend_from_slice(&secret);
            input.extend_from_slice(link_id.as_slice());
            crate::hash::full_hash(&input)
        };
        let mut seed = [0_u8; 64];
        seed[..32].copy_from_slice(&half(b"retinue/node/responder/a"));
        seed[32..].copy_from_slice(&half(b"retinue/node/responder/b"));
        seed
    }

    /// Advance the node's own timers.
    ///
    /// `rand_hash` is caller-supplied entropy for the announce, kept out of here for the
    /// same reason [`announce::build`] keeps it out: no RNG in the protocol layer, and
    /// fixtures stay reproducible.
    pub fn poll(
        &mut self,
        now: u64,
        interface: InterfaceId,
        rand_hash: &[u8; RAND_HASH_LEN],
    ) -> Actions<ACTIONS> {
        let mut actions = Actions::new();

        let due = match self.last_announce {
            None => true,
            Some(last) => now.saturating_sub(last) >= self.announce_interval,
        };
        if due {
            self.last_announce = Some(now);
            actions.push(Action::Send {
                interface,
                packet: self.announce(rand_hash, None),
            });
        }

        actions
    }

    /// Build this node's announce packet.
    pub fn announce(
        &self,
        rand_hash: &[u8; RAND_HASH_LEN],
        ratchet: Option<&[u8; RATCHET_LEN]>,
    ) -> Packet {
        announce::build(
            &self.identity,
            self.name_hash,
            rand_hash,
            ratchet,
            &self.app_data,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;

    const IFACE: InterfaceId = 0;

    fn fixture(name: &str) -> Packet {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
        let raw = std::fs::read(std::format!("{path}{name}")).unwrap();
        Packet::decode(&raw).unwrap()
    }

    /// The first packet a set of actions wants sent.
    fn sent<const N: usize>(actions: &Actions<N>) -> Option<Packet> {
        actions.iter().find_map(|a| match a {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
    }

    /// The link id a set of actions reports coming up.
    fn link_up<const N: usize>(actions: &Actions<N>) -> Option<AddressHash> {
        actions.iter().find_map(|a| match a {
            Action::LinkUp { link_id } => Some(*link_id),
            _ => None,
        })
    }

    /// Two nodes that have not met.
    fn pair() -> (Node<32, 8, 4>, Node<32, 8, 4>) {
        (
            Node::new(
                PrivateIdentity::from_secret_bytes(&[0x11; 64]),
                DestinationName::new("retinue", ["a"]).name_hash(),
            ),
            Node::new(
                PrivateIdentity::from_secret_bytes(&[0x22; 64]),
                DestinationName::new("retinue", ["b"]).name_hash(),
            ),
        )
    }

    /// Two nodes with a link already established between them.
    fn linked() -> (Node<32, 8, 4>, Node<32, 8, 4>, AddressHash) {
        let (mut a, mut b) = pair();
        a.ingest(IFACE, &b.announce(&[2; RAND_HASH_LEN], None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64]).unwrap()).unwrap();
        let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
        let id = link_up(&a.ingest(IFACE, &proof, 0)).expect("link did not come up");
        (a, b, id)
    }

    fn node() -> Node {
        let identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let name = DestinationName::new("retinue", ["node"]);
        Node::new(identity, name.name_hash())
    }

    /// A real RNS announce teaches the node a peer it can then reach.
    #[test]
    fn a_real_announce_is_learned() {
        let mut n = node();
        let packet = fixture("announce_appdata.bin");
        let actions = n.ingest(IFACE, &packet, 0);

        assert_eq!(actions.len(), 1, "one learned destination");
        assert_eq!(n.peers().len(), 1);
        match actions.iter().next().unwrap() {
            Action::Learned { destination } => assert!(n.peers().knows(*destination)),
            other => panic!("expected Learned, got {other:?}"),
        }
    }

    /// Every RNS-generated invalid announce is refused, and none of them leaves a trace.
    ///
    /// This is the oracle that matters most for a board: the fixtures were produced by real
    /// RNS with one field corrupted each, so a node that accepted any of them would be
    /// letting a peer populate its tables with unverified identity.
    #[test]
    fn every_invalid_announce_fixture_is_refused() {
        for name in [
            "announce_invalid_signature.bin",
            "announce_invalid_pubkey.bin",
            "announce_invalid_desthash.bin",
            "announce_invalid_namehash.bin",
            "announce_invalid_randhash.bin",
            "announce_invalid_appdata.bin",
        ] {
            let mut n = node();
            let actions = n.ingest(IFACE, &fixture(name), 0);
            assert!(actions.is_empty(), "{name} produced an action");
            assert_eq!(n.peers().len(), 0, "{name} populated the address book");
        }
    }

    /// A node announces itself promptly on boot, then holds off for its interval.
    #[test]
    fn announces_on_boot_then_waits_for_the_interval() {
        let mut n = node().with_announce_interval(1_000);
        let rand = [0x55; RAND_HASH_LEN];

        let first = n.poll(0, IFACE, &rand);
        assert_eq!(first.len(), 1, "a fresh node announces without waiting");

        assert!(n.poll(1, IFACE, &rand).is_empty(), "not due yet");
        assert!(n.poll(999, IFACE, &rand).is_empty(), "still not due");
        assert_eq!(n.poll(1_000, IFACE, &rand).len(), 1, "due at the interval");
    }

    /// Our own announce is a real one: it decodes, verifies, and names us.
    #[test]
    fn our_announce_round_trips_through_the_decoder() {
        let n = node().with_app_data(b"retinue-node");
        let packet = n.announce(&[0x22; RAND_HASH_LEN], None);

        let decoded = Announce::decode(&packet).expect("our own announce must verify");
        assert_eq!(decoded.destination, n.destination());
        assert_eq!(decoded.app_data, b"retinue-node");
    }

    /// Two nodes learn each other from each other's announces, which is the whole of the
    /// discovery half of a link.
    #[test]
    fn two_nodes_learn_each_other() {
        let mut a = Node::<32, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0xA1; 64]),
            DestinationName::new("retinue", ["a"]).name_hash(),
        );
        let mut b = Node::<32, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0xB2; 64]),
            DestinationName::new("retinue", ["b"]).name_hash(),
        );

        let from_a = a.announce(&[1; RAND_HASH_LEN], None);
        let from_b = b.announce(&[2; RAND_HASH_LEN], None);

        assert_eq!(b.ingest(IFACE, &from_a, 0).len(), 1);
        assert_eq!(a.ingest(IFACE, &from_b, 0).len(), 1);

        assert!(b.peers().knows(a.destination()), "b can now reach a");
        assert!(a.peers().knows(b.destination()), "a can now reach b");
    }

    /// A full address book keeps serving and stops learning, rather than growing.
    #[test]
    fn a_full_book_stops_learning_without_faulting() {
        let mut n = Node::<1, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["node"]).name_hash(),
        );
        assert_eq!(
            n.ingest(IFACE, &fixture("announce_appdata.bin"), 0).len(),
            1
        );

        // A different destination cannot be learned, and says nothing rather than faulting.
        let other = Node::<32, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0xC3; 64]),
            DestinationName::new("retinue", ["other"]).name_hash(),
        )
        .announce(&[9; RAND_HASH_LEN], None);
        assert!(n.ingest(IFACE, &other, 0).is_empty());
        assert_eq!(n.peers().len(), 1, "the established peer survives");
        assert_eq!(n.peers().refused(), 1, "and the refusal is counted");
    }

    /// Actions are bounded, and say so when they fill.
    #[test]
    fn actions_report_overflow_rather_than_dropping_silently() {
        let mut actions = Actions::<2>::new();
        for _ in 0..5 {
            actions.push(Action::Learned {
                destination: AddressHash::from_bytes([0; 16]),
            });
        }
        assert_eq!(actions.len(), 2, "held to its bound");
        assert_eq!(actions.overflowed(), 3, "and counted what did not fit");
    }

    /// Data and link packets are not yet handled, and must be dropped rather than
    /// mishandled. This pins the boundary so the next gate's work is visible as a change.
    #[test]
    fn packets_this_gate_does_not_handle_are_dropped() {
        let mut n = node();
        // The same bytes that would be learned as an announce, relabelled. Nothing is
        // learned, because the type decides the handling and this gate handles one type.
        let packet = Packet {
            packet_type: PacketType::Data,
            ..fixture("announce_appdata.bin")
        };
        let actions = n.ingest(IFACE, &packet, 0);
        assert!(actions.is_empty());
        assert_eq!(n.peers().len(), 0);
    }

    /// Two nodes establish a link in the shape a radio carries it: announce, learn, open,
    /// accept, prove.
    #[test]
    fn two_nodes_establish_a_link() {
        let (mut a, mut b) = pair();

        // Discovery first: a must have heard b announce before it can address b.
        a.ingest(IFACE, &b.announce(&[2; RAND_HASH_LEN], None), 0);

        let opened = a
            .open_link(b.destination(), IFACE, &[0x31; 64])
            .expect("b is known, so a link can be opened");
        let request = sent(&opened).expect("a link request goes out");

        let accepted = b.ingest(IFACE, &request, 0);
        let proof = sent(&accepted).expect("b answers with a proof");
        assert_eq!(b.link_count(), 1, "b holds the link immediately");
        assert!(link_up(&accepted).is_some(), "b reports the link up");

        let completed = a.ingest(IFACE, &proof, 0);
        assert_eq!(a.link_count(), 1, "a holds the link once proved");
        let id = link_up(&completed).expect("a reports the link up");

        assert!(
            a.has_link(id) && b.has_link(id),
            "one link, one id, both sides"
        );
    }

    /// A retransmitted link request is answered with the SAME proof, not a second link.
    ///
    /// A lossy medium creates this constantly: the initiator does not hear the proof and
    /// asks again. Accepting twice would leave the two sides holding different keys for
    /// what the initiator believes is one link, which fails later and confusingly.
    #[test]
    fn a_retransmitted_request_is_answered_with_the_same_proof() {
        let (mut a, mut b) = pair();
        a.ingest(IFACE, &b.announce(&[2; RAND_HASH_LEN], None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64]).unwrap()).unwrap();

        let first = sent(&b.ingest(IFACE, &request, 0)).expect("first proof");
        let second = sent(&b.ingest(IFACE, &request, 0)).expect("second proof");

        assert_eq!(first, second, "the same proof, byte for byte");
        assert_eq!(b.link_count(), 1, "and still exactly one link");
    }

    /// Data crosses an established link and arrives decrypted.
    #[test]
    fn data_crosses_an_established_link() {
        let (a, mut b, id) = linked();

        let out = a
            .send(id, IFACE, b"hello over the air", &[7; crate::token::IV_LEN])
            .expect("a can send on a link it holds");
        let packet = sent(&out).expect("a data packet goes out");

        let received = b.ingest(IFACE, &packet, 0);
        let found = received.iter().find_map(|x| match x {
            Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
            _ => None,
        });
        match found {
            Some((link_id, payload)) => {
                assert_eq!(link_id, id);
                assert_eq!(payload.as_slice(), b"hello over the air");
            }
            None => panic!("expected decrypted Data"),
        }
    }

    /// A link request for another destination is ignored: a board is not a transport node
    /// and must not answer for addresses it does not hold.
    #[test]
    fn a_link_request_for_another_destination_is_ignored() {
        let (mut a, b) = pair();
        a.ingest(IFACE, &b.announce(&[2; RAND_HASH_LEN], None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64]).unwrap()).unwrap();

        let mut c = Node::<32, 8, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0xCC; 64]),
            DestinationName::new("retinue", ["c"]).name_hash(),
        );
        assert!(c.ingest(IFACE, &request, 0).is_empty());
        assert_eq!(c.link_count(), 0);
    }

    /// A peer closing the link drops it and reports it.
    #[test]
    fn a_peer_closing_the_link_drops_it() {
        let (mut a, b, id) = linked();
        let close = b
            .links
            .iter()
            .find(|(l, _)| l.id() == id)
            .map(|(l, _)| l.close_packet(&[3; crate::token::IV_LEN]))
            .unwrap();

        let actions = a.ingest(IFACE, &close, 0);
        assert!(actions.iter().any(|x| matches!(x, Action::LinkDown { .. })));
        assert_eq!(a.link_count(), 0, "the link is gone");
        assert!(!a.has_link(id));
    }

    /// A full link table refuses new peers and keeps the ones it has.
    #[test]
    fn a_full_link_table_refuses_and_counts() {
        let mut server = Node::<32, 8, 1>::new(
            PrivateIdentity::from_secret_bytes(&[0x22; 64]),
            DestinationName::new("retinue", ["b"]).name_hash(),
        );
        let mut first = Node::<32, 8, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["a"]).name_hash(),
        );
        let mut second = Node::<32, 8, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0xDD; 64]),
            DestinationName::new("retinue", ["d"]).name_hash(),
        );
        let ann = server.announce(&[2; RAND_HASH_LEN], None);
        first.ingest(IFACE, &ann, 0);
        second.ingest(IFACE, &ann, 0);

        let r1 = sent(
            &first
                .open_link(server.destination(), IFACE, &[0x31; 64])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            server.ingest(IFACE, &r1, 0).len(),
            2,
            "accepted: proof plus LinkUp"
        );

        let r2 = sent(
            &second
                .open_link(server.destination(), IFACE, &[0x41; 64])
                .unwrap(),
        )
        .unwrap();
        assert!(
            server.ingest(IFACE, &r2, 0).is_empty(),
            "refused, and nothing goes to the wire"
        );
        assert_eq!(server.link_count(), 1, "the established link survives");
        assert_eq!(server.refused_links(), 1, "and the refusal is counted");
    }

    /// Link traffic this gate does not handle is dropped rather than mishandled.
    #[test]
    fn unhandled_link_traffic_is_dropped() {
        let (mut a, b, id) = linked();
        let keepalive = b
            .links
            .iter()
            .find(|(l, _)| l.id() == id)
            .map(|(l, _)| l.keepalive_packet(0xff))
            .unwrap();
        assert!(a.ingest(IFACE, &keepalive, 0).is_empty());
        assert!(a.has_link(id), "and the link survives being spoken to");
    }
}
