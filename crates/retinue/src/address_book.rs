//! The address book: learning peers from announces.
//!
//! A destination hash cannot be turned back into an identity, so to reach a peer you must
//! have heard it announce. The address book ingests validated [`Announce`]s and answers the
//! question a link needs: given a destination hash, what is the identity (to verify its
//! proof) and its current ratchet?
//!
//! This is pure state over [`Announce`], which is itself already validated on decode, so an
//! entry only ever comes from an announce whose signature checked out. Cadence and I/O live
//! in the tokio shell above; this holds no timers and does no network.

// Needed by the test build or the tokio shell; the bare no_std lib does not reach it.
#[allow(unused_imports)]
use alloc::format;

use alloc::vec::Vec;

use alloc::collections::BTreeMap;

use crate::announce::{Announce, RATCHET_LEN};
use crate::hash::{AddressHash, NameHash};
use crate::identity::Identity;

/// What the book knows about one destination.
#[derive(Clone, Debug)]
pub struct Peer {
    /// The destination's identity, enough to verify a link proof from it.
    pub identity: Identity,
    /// The destination's name hash, as announced.
    pub name_hash: NameHash,
    /// The most recently announced app data.
    pub app_data: Vec<u8>,
    /// The destination's current ratchet public key, if it advertises ratchets. Kept so a
    /// single-packet encryption to this destination can use the ratchet rather than the
    /// long-term key.
    pub ratchet: Option<[u8; RATCHET_LEN]>,
    /// How many announces for this destination have been ingested. A cheap freshness and
    /// liveness signal without a clock, which this layer deliberately does not have.
    pub announces_seen: u64,
    /// The caller's tick when an announce for this destination was last ingested by
    /// [`AddressBook::ingest_at`]. Zero for an entry that only [`AddressBook::ingest`] has
    /// touched. It orders eviction; the book never reads a clock itself.
    pub last_heard: u64,
}

/// The most destinations a book holds unless told otherwise.
///
/// A runtime cap on a growable map rather than a fixed-size table: the count is a policy
/// choice that differs by an order of magnitude between a desktop and a board, and a
/// structural bound would commit the desktop's whole worst case as static storage.
pub const DEFAULT_MAX_PEERS: usize = 4096;

/// What [`AddressBook::ingest`] did with an announce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ingested {
    /// A destination not previously known was added.
    Learned,
    /// A known destination's entry was refreshed.
    Refreshed,
    /// The book is at capacity, this destination is not in it, and nothing could be evicted
    /// for it. The book keeps serving every destination it already knows; the shell makes
    /// room with [`AddressBook::forget`].
    Refused,
}

/// A store of peers learned from announces, keyed by destination hash.
pub struct AddressBook {
    peers: BTreeMap<AddressHash, Peer>,
    max_peers: usize,
    refused: u64,
    evicted: u64,
}

impl Default for AddressBook {
    fn default() -> Self {
        Self::with_max_peers(DEFAULT_MAX_PEERS)
    }
}

impl AddressBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// A book that holds at most `max_peers` destinations.
    pub fn with_max_peers(max_peers: usize) -> Self {
        Self {
            peers: BTreeMap::new(),
            max_peers,
            refused: 0,
            evicted: 0,
        }
    }

    /// Announces refused because the book was full. Nonzero means the shell's expiry policy
    /// is not keeping up with what the mesh is announcing.
    pub fn refused(&self) -> u64 {
        self.refused
    }

    /// Peers displaced by [`AddressBook::ingest_at`] to make room for a new destination.
    pub fn evicted(&self) -> u64 {
        self.evicted
    }

    /// Whether the book can learn a destination it does not already know.
    pub fn is_full(&self) -> bool {
        self.peers.len() >= self.max_peers
    }

    /// Record an announce. A later announce for the same destination refreshes the entry
    /// (app data, ratchet) and bumps the count. Because [`Announce`] only exists once its
    /// signature has verified, ingesting one cannot poison the book with a forged identity.
    /// A full book still refreshes destinations it already knows, and refuses only new ones,
    /// so a flood of unknown destinations cannot displace established peers. It never
    /// evicts; [`AddressBook::ingest_at`] is the form that does.
    pub fn ingest(&mut self, announce: &Announce) -> Ingested {
        self.admit(announce, None, |_| true)
    }

    /// Record an announce heard at `now`, a tick in whatever unit the caller keeps, making
    /// room in a full book by evicting one peer if it has to.
    ///
    /// The eviction rule: when the book is full and `announce` names a destination it does
    /// not hold, the peer with the oldest `last_heard` for which `protected` returns `false`
    /// is forgotten and the new destination takes its place (ties go to the lowest
    /// destination hash, so the choice is deterministic). The caller's predicate says which
    /// peers are still in use, such as those with a live route, link, or outstanding request.
    /// Only when every held peer is protected is the announce [`Ingested::Refused`]. A
    /// refreshed peer's `last_heard` never moves backwards.
    ///
    /// This is the book's half of RNS's age-based culling of known destinations: a peer that
    /// stopped announcing and that nothing uses any more yields its slot to one that is live.
    pub fn ingest_at(
        &mut self,
        announce: &Announce,
        now: u64,
        protected: impl Fn(AddressHash) -> bool,
    ) -> Ingested {
        self.admit(announce, Some(now), protected)
    }

    fn admit(
        &mut self,
        announce: &Announce,
        now: Option<u64>,
        protected: impl Fn(AddressHash) -> bool,
    ) -> Ingested {
        if let Some(p) = self.peers.get_mut(&announce.destination) {
            p.identity = announce.identity;
            p.name_hash = announce.name_hash;
            p.app_data = announce.app_data.clone();
            p.ratchet = announce.ratchet;
            p.announces_seen += 1;
            if let Some(now) = now {
                p.last_heard = p.last_heard.max(now);
            }
            return Ingested::Refreshed;
        }
        if self.is_full() {
            let stalest = self
                .peers
                .iter()
                .filter(|(destination, _)| !protected(**destination))
                .min_by_key(|(_, peer)| peer.last_heard)
                .map(|(destination, _)| *destination);
            match stalest {
                Some(destination) if self.max_peers > 0 => {
                    self.peers.remove(&destination);
                    self.evicted = self.evicted.saturating_add(1);
                }
                _ => {
                    self.refused = self.refused.saturating_add(1);
                    return Ingested::Refused;
                }
            }
        }
        self.peers.insert(
            announce.destination,
            Peer {
                identity: announce.identity,
                name_hash: announce.name_hash,
                app_data: announce.app_data.clone(),
                ratchet: announce.ratchet,
                announces_seen: 1,
                last_heard: now.unwrap_or(0),
            },
        );
        Ingested::Learned
    }

    /// Resolve a destination hash to what we know about it.
    pub fn resolve(&self, destination: AddressHash) -> Option<&Peer> {
        self.peers.get(&destination)
    }

    /// Whether we can reach a destination, i.e. have heard it announce.
    pub fn knows(&self, destination: AddressHash) -> bool {
        self.peers.contains_key(&destination)
    }

    /// Every destination currently known.
    pub fn destinations(&self) -> impl Iterator<Item = AddressHash> + '_ {
        self.peers.keys().copied()
    }

    /// Number of destinations known.
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// Forget a destination, e.g. after it has been unreachable past a policy the shell
    /// enforces.
    pub fn forget(&mut self, destination: AddressHash) -> Option<Peer> {
        self.peers.remove(&destination)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::packet::Packet;

    fn announce(fixture: &str) -> Announce {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
        let raw = std::fs::read(format!("{path}{fixture}")).unwrap();
        Announce::decode(&Packet::decode(&raw).unwrap()).unwrap()
    }

    /// A full book keeps serving what it knows and refuses only new destinations, so a mesh
    /// announcing more than this node budgeted for cannot displace established peers.
    #[test]
    fn a_full_book_refreshes_the_known_and_refuses_the_new() {
        let a = announce("announce_appdata.bin");
        let mut book = AddressBook::with_max_peers(1);

        assert_eq!(book.ingest(&a), Ingested::Learned);
        assert!(book.is_full());

        // The same destination still refreshes, and its count still climbs.
        assert_eq!(book.ingest(&a), Ingested::Refreshed);
        assert_eq!(book.resolve(a.destination).unwrap().announces_seen, 2);
        assert_eq!(book.refused(), 0);

        // A different destination is refused, counted, and does not evict the known one.
        let mut other = announce("announce_appdata.bin");
        other.destination = AddressHash::from_bytes([0x5A; 16]);
        assert_eq!(book.ingest(&other), Ingested::Refused);
        assert_eq!(book.refused(), 1);
        assert_eq!(book.len(), 1);
        assert!(book.knows(a.destination), "the established peer survives");
        assert!(!book.knows(other.destination));
    }

    fn announce_for(byte: u8) -> Announce {
        let mut a = announce("announce_appdata.bin");
        a.destination = AddressHash::from_bytes([byte; 16]);
        a
    }

    /// A full book makes room by forgetting the least recently heard peer the caller does not
    /// protect, and refuses only when every peer it holds is protected.
    #[test]
    fn a_full_book_evicts_the_least_recently_heard_unprotected_peer() {
        let (a, b, c) = (announce_for(1), announce_for(2), announce_for(3));
        let mut book = AddressBook::with_max_peers(2);
        assert_eq!(book.ingest_at(&a, 10, |_| false), Ingested::Learned);
        assert_eq!(book.ingest_at(&b, 20, |_| false), Ingested::Learned);
        // Hearing `a` again makes `b` the stalest.
        assert_eq!(book.ingest_at(&a, 30, |_| false), Ingested::Refreshed);
        assert_eq!(book.resolve(a.destination).unwrap().last_heard, 30);

        // Every peer protected: nothing can go, so the newcomer is refused.
        assert_eq!(book.ingest_at(&c, 40, |_| true), Ingested::Refused);
        assert_eq!(book.refused(), 1);
        assert_eq!(book.evicted(), 0);

        // `b` is the stalest, but protected; `a` goes instead.
        let protect_b = |d: AddressHash| d == b.destination;
        assert_eq!(book.ingest_at(&c, 50, protect_b), Ingested::Learned);
        assert!(!book.knows(a.destination));
        assert!(book.knows(b.destination));
        assert_eq!(book.resolve(c.destination).unwrap().last_heard, 50);
        assert_eq!(book.evicted(), 1);

        // Unprotected, the stalest (`b`, heard at 20) goes.
        assert_eq!(book.ingest_at(&a, 60, |_| false), Ingested::Learned);
        assert!(!book.knows(b.destination));
        assert!(book.knows(c.destination));
        assert_eq!(book.len(), 2);
    }

    #[test]
    fn a_zero_capacity_book_refuses_even_with_eviction() {
        let mut book = AddressBook::with_max_peers(0);
        assert_eq!(
            book.ingest_at(&announce_for(1), 1, |_| false),
            Ingested::Refused
        );
        assert!(book.is_empty());
    }

    #[test]
    fn ingest_and_resolve_a_real_announce() {
        let a = announce("announce_appdata.bin");
        let mut book = AddressBook::new();
        assert!(!book.knows(a.destination));
        book.ingest(&a);

        let peer = book.resolve(a.destination).expect("resolved");
        assert_eq!(peer.identity.hash(), a.identity.hash());
        assert_eq!(peer.app_data, b"retinue-r0-fixture");
        assert_eq!(peer.announces_seen, 1);
        assert!(peer.ratchet.is_none());
    }

    #[test]
    fn a_ratcheted_announce_carries_its_ratchet() {
        let a = announce("announce_ratchet.bin");
        let mut book = AddressBook::new();
        book.ingest(&a);
        assert!(book.resolve(a.destination).unwrap().ratchet.is_some());
    }

    #[test]
    fn re_ingesting_refreshes_rather_than_duplicates() {
        let plain = announce("announce_plain.bin");
        let with_data = announce("announce_appdata.bin");
        // Same identity and name, so same destination hash.
        assert_eq!(plain.destination, with_data.destination);

        let mut book = AddressBook::new();
        book.ingest(&plain);
        book.ingest(&with_data);
        assert_eq!(book.len(), 1);
        let peer = book.resolve(plain.destination).unwrap();
        assert_eq!(peer.announces_seen, 2);
        assert_eq!(peer.app_data, b"retinue-r0-fixture"); // the later one won
    }
}
