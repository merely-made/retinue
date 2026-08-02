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

/// An executor-neutral Reticulum node.
///
/// `PEERS` bounds the address book. `ACTIONS` bounds what one call can ask of the shell.
/// Both default to the board profile, because the desktop has `Endpoint` and does not want
/// this type.
pub struct Node<const PEERS: usize = 32, const ACTIONS: usize = 8> {
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
}

impl<const PEERS: usize, const ACTIONS: usize> Node<PEERS, ACTIONS> {
    /// A node with an identity and the destination it answers to.
    pub fn new(identity: PrivateIdentity, name_hash: NameHash) -> Self {
        Self {
            identity,
            name_hash,
            book: AddressBook::with_max_peers(PEERS),
            app_data: Vec::new(),
            last_announce: None,
            announce_interval: DEFAULT_ANNOUNCE_INTERVAL,
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

    /// Feed a received packet in.
    ///
    /// Anything malformed, unsigned, or not addressed to work this node does is dropped
    /// silently, exactly as the desktop drops it: a peer must not be able to make a board
    /// spend memory by sending rubbish.
    pub fn ingest(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        _now: u64,
    ) -> Actions<ACTIONS> {
        let mut actions = Actions::new();
        let _ = interface;

        if packet.packet_type == PacketType::Announce {
            // `Announce::decode` verifies the signature and that the destination hash
            // matches the announced identity, so an entry can only come from an announce
            // whose maths checked out. The invalid-announce fixtures are the proof.
            if let Ok(announce) = Announce::decode(packet)
                && self.book.ingest(&announce) != Ingested::Refused
            {
                actions.push(Action::Learned {
                    destination: announce.destination,
                });
            }
        }

        actions
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
}
