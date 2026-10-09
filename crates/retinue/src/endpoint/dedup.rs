//! Bounded hash memories that drop repeated packets and announces.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::Ordering;

use crate::hash::AddressHash;
use crate::packet::{DestinationType, Packet, PacketType};

use super::shared::Shared;

/// How many recent announce packet-hashes to remember for de-duplication.
pub(super) const SEEN_ANNOUNCES: usize = 4096;

/// Recent link packet hashes, both ways, across every link this endpoint holds.
///
/// On a shared medium a relay's retransmission of our own link packet reaches us under the
/// shared link key, with hops+1 and the same hash: `sent` marks it as ours rather than the
/// far end's. That covers Channel too, where taking our own sequence for the far end's would
/// also drop the far end's real one as a repeat. The same medium hands us the far end's packet
/// twice, directly and from a relay: `received` delivers it once.
pub(super) struct LinkPacketMemory {
    sent: HashWindow,
    received: HashWindow,
}

impl LinkPacketMemory {
    pub(super) fn new() -> Self {
        Self {
            sent: HashWindow::new(crate::capacity::desktop::OWN_ECHO_HASHES),
            received: HashWindow::new(crate::capacity::desktop::DUPLICATE_HASHES),
        }
    }

    /// Note a link packet we transmit.
    pub(super) fn note_sent(&mut self, pkt: &Packet) {
        if is_remembered_link_packet(pkt) {
            self.sent.insert(pkt.hash());
        }
    }

    /// Whether an inbound packet on one of our links is new, our own, or a repeat.
    pub(super) fn admit(&mut self, pkt: &Packet) -> LinkPacketAdmission {
        if !is_remembered_link_packet(pkt) {
            return LinkPacketAdmission::New;
        }
        let hash = pkt.hash();
        if self.sent.contains(&hash) {
            LinkPacketAdmission::OwnEcho
        } else if self.received.insert(hash) {
            LinkPacketAdmission::New
        } else {
            LinkPacketAdmission::Duplicate
        }
    }
}

/// What [`LinkPacketMemory::admit`] made of an inbound link packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LinkPacketAdmission {
    New,
    OwnEcho,
    Duplicate,
}

fn is_remembered_link_packet(pkt: &Packet) -> bool {
    pkt.packet_type == PacketType::Data
        && pkt.destination_type == DestinationType::Link
        && crate::node::is_deduplicated_link_context(pkt.context)
}

/// A bounded set of packet hashes that forgets the oldest first. A burst can outrun it; that
/// only lets a late copy through, never drops a new packet.
pub(super) struct HashWindow {
    set: HashSet<AddressHash>,
    pub(super) order: VecDeque<AddressHash>,
    capacity: usize,
}

impl HashWindow {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            set: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    pub(super) fn contains(&self, hash: &AddressHash) -> bool {
        self.set.contains(hash)
    }

    /// Record a hash; false if it was already there.
    pub(super) fn insert(&mut self, hash: AddressHash) -> bool {
        if !self.set.insert(hash) {
            return false;
        }
        self.order.push_back(hash);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.set.remove(&oldest);
        }
        true
    }
}

/// Hashes remembered in two generations, as RNS keeps its packet hash list and the one before
/// it (`Transport.py` 832-834): once the current generation holds `capacity`, it becomes the
/// previous one, so a burst forgets the older half rather than everything.
pub(super) struct HashList {
    current: HashSet<AddressHash>,
    previous: HashSet<AddressHash>,
    capacity: usize,
}

impl HashList {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            current: HashSet::new(),
            previous: HashSet::new(),
            capacity,
        }
    }

    /// Record a hash; false if it was already remembered.
    pub(super) fn insert(&mut self, hash: AddressHash) -> bool {
        if self.previous.contains(&hash) || !self.current.insert(hash) {
            return false;
        }
        if self.current.len() >= self.capacity {
            self.previous = core::mem::take(&mut self.current);
        }
        true
    }
}

/// Packet hashes held per generation by the endpoint's packet filter.
pub(super) const PACKET_HASHES: usize = 32_768;

/// Path request tags held per generation, RNS's `max_pr_tags` (`Transport.py` 195).
pub(super) const PATH_REQUEST_TAGS: usize = 16_000;

impl Shared {
    /// Record a transit or single packet's hash; false for a copy already seen. Resource parts
    /// and keepalives legitimately repeat their hash and are always new, as RNS exempts them
    /// (`Transport.py` 1635-1640). Channel is filtered, unlike RNS: see N10.
    pub(super) fn packet_is_new(&self, pkt: &Packet) -> bool {
        if !crate::node::is_deduplicated_link_context(pkt.context)
            || self.packet_filter.lock().unwrap().insert(pkt.hash())
        {
            return true;
        }
        self.routing_stats
            .filtered_packets
            .fetch_add(1, Ordering::Relaxed);
        false
    }

    /// Whether this announce (by packet hash) is new; records it if so.
    pub(super) fn announce_is_new(&self, hash: AddressHash) -> bool {
        let mut g = self.seen_announces.lock().unwrap();
        if g.0.contains(&hash) {
            return false;
        }
        g.0.insert(hash);
        g.1.push_back(hash);
        if g.1.len() > SEEN_ANNOUNCES
            && let Some(old) = g.1.pop_front()
        {
            g.0.remove(&old);
        }
        true
    }
}
