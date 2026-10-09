//! Carrying others' traffic: link bridges, the reverse table, and forwarding.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::address_book::AddressBook;
use crate::hash::AddressHash;
use crate::link;
use crate::packet::{Packet, PacketType};

use super::interface::InterfaceId;
use super::queue::TrafficClass;
use super::routing::RoutingPolicy;
use super::shared::Shared;

/// How long a validated bridge is remembered after its last packet: far longer than a live
/// link goes quiet. An unproved one lapses at its proof deadline.
pub(super) const LINK_TRANSPORT_TTL: Duration = Duration::from_secs(3600);

/// How long a validated link may go unheard before a new request may take its place in a full
/// table: RNS's `Transport.LINK_TIMEOUT`, as [`crate::node::LINK_TRANSPORT_IDLE`].
pub(super) const LINK_TRANSPORT_IDLE: Duration =
    Duration::from_millis(crate::node::LINK_TRANSPORT_IDLE);

/// The most links a transport node carries at once. A request arriving at the bound displaces
/// the stalest link still awaiting its proof, or else a validated one unheard for
/// [`LINK_TRANSPORT_IDLE`]; a recently heard validated link is never displaced, so forged
/// requests cannot evict carried links or grow the table.
#[cfg(not(test))]
pub(super) const LINK_TRANSPORT_CAPACITY: usize = 4096;
#[cfg(test)]
pub(super) const LINK_TRANSPORT_CAPACITY: usize = 4;

/// A link carried through this node: its two interfaces, and whether its destination has
/// proved it yet.
#[derive(Clone, Copy, Debug)]
pub(super) struct LinkBridge {
    pub(super) from: InterfaceId,
    pub(super) out: InterfaceId,
    /// The destination the request named, whose identity signs the proof.
    pub(super) destination: AddressHash,
    pub(super) seen: Instant,
    /// When an unproved link lapses; `None` once the destination's proof has passed.
    pub(super) proof_deadline: Option<Instant>,
}

impl LinkBridge {
    pub(super) fn lapsed(&self, now: Instant) -> bool {
        self.proof_deadline.map_or(
            now.duration_since(self.seen) >= LINK_TRANSPORT_TTL,
            |deadline| now >= deadline,
        )
    }

    /// Whether `pkt`, heard on `iface`, may cross: `None` if not, else whether it is the proof
    /// that validates the bridge once carried. Only the two sides may use it, nothing crosses
    /// before the proof, and the proof must come from the destination's side under its
    /// signature (the side alone when its identity is unknown).
    pub(super) fn admit(
        &self,
        iface: InterfaceId,
        pkt: &Packet,
        book: &Mutex<AddressBook>,
    ) -> Option<bool> {
        if iface != self.from && iface != self.out {
            return None;
        }
        if pkt.packet_type != PacketType::Proof || pkt.context != link::CTX_LRPROOF {
            return self.proof_deadline.is_none().then_some(false);
        }
        let signed = iface == self.out
            && book
                .lock()
                .unwrap()
                .resolve(self.destination)
                .is_none_or(|peer| link::proof_is_signed_by(pkt, &peer.identity));
        signed.then_some(true)
    }
}

/// How long a carried packet's return path is kept for its proof (RNS
/// `Transport.REVERSE_TIMEOUT`).
pub(super) const REVERSE_TIMEOUT: Duration = Duration::from_secs(8 * 60);

/// Carried packets whose return path is remembered at once. At capacity the oldest goes.
#[cfg(not(test))]
pub(super) const REVERSE_TABLE_CAPACITY: usize = 4096;
#[cfg(test)]
pub(super) const REVERSE_TABLE_CAPACITY: usize = 4;

/// The way back for a carried packet's proof.
pub(super) struct ReverseEntry {
    received: InterfaceId,
    outbound: InterfaceId,
    pub(super) at: Instant,
}

/// Make room for one more entry in a bounded table: drop what `expired` says has lapsed,
/// then, still full, the entry with the smallest `age`, which is returned.
pub(super) fn make_room<V, A: Ord>(
    table: &mut HashMap<AddressHash, V>,
    capacity: usize,
    expired: impl Fn(&V) -> bool,
    age: impl Fn(&V) -> A,
) -> Option<V> {
    if table.len() < capacity {
        return None;
    }
    table.retain(|_, entry| !expired(entry));
    if table.len() < capacity {
        return None;
    }
    let oldest = table.iter().min_by_key(|(_, entry)| age(entry))?.0;
    let oldest = *oldest;
    table.remove(&oldest)
}

impl Shared {
    /// Remember the way back for a carried packet's proof (RNS `Transport.py` 2104-2110).
    pub(super) fn remember_reverse(
        &self,
        packet: AddressHash,
        received: InterfaceId,
        outbound: InterfaceId,
    ) {
        let now = Instant::now();
        let mut table = self.reverse_table.lock().unwrap();
        if !table.contains_key(&packet) {
            make_room(
                &mut table,
                REVERSE_TABLE_CAPACITY,
                |entry| now.duration_since(entry.at) >= REVERSE_TIMEOUT,
                |entry| entry.at,
            );
        }
        table.insert(
            packet,
            ReverseEntry {
                received,
                outbound,
                at: now,
            },
        );
    }

    /// Consume the return path for a proof addressed to `packet`, yielding the interface to
    /// carry it out on if it arrived on the one the packet left by (RNS `Transport.py`
    /// 2733-2744).
    pub(super) fn take_reverse(
        &self,
        packet: AddressHash,
        arrived: InterfaceId,
    ) -> Option<InterfaceId> {
        let entry = self.reverse_table.lock().unwrap().remove(&packet)?;
        (entry.at.elapsed() < REVERSE_TIMEOUT && entry.outbound == arrived)
            .then_some(entry.received)
    }
}

/// Forward a header-type-2 packet addressed to us as a transport hop, toward its
/// destination. `from` is the interface it arrived on.
pub(super) fn forward(
    shared: &Arc<Shared>,
    from: InterfaceId,
    pkt: Packet,
    policy: &RoutingPolicy,
) {
    if pkt.hops.saturating_add(1) >= policy.max_hops {
        shared
            .routing_stats
            .hop_limit_dropped
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    // A copy already carried is a loop or a second path, never new work.
    // One hash serves the packet filter and the reverse entry.
    let hash = pkt.hash();
    if !shared.packet_is_new(pkt.context, hash) {
        return;
    }
    let dest = pkt.destination;

    let next = shared.path_iface(dest);
    if let Some(out) = next {
        // Refuse before recording a bridge that policy would never carry.
        if !policy.allowed_egress.allows(out) {
            shared
                .routing_stats
                .policy_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        // A link request establishes a bridge for its proof and later link data.
        let mut pkt = pkt;
        let mut admitted = None;
        if pkt.packet_type == PacketType::LinkRequest
            && let Ok(link_id) = link::link_id(&pkt)
        {
            match admit_link_request(shared, from, out, link_id, &mut pkt) {
                BridgeAdmission::Refused => {
                    shared
                        .routing_stats
                        .policy_rejected
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
                BridgeAdmission::New => admitted = Some(link_id),
                BridgeAdmission::Known => {}
            }
        } else if pkt.packet_type != PacketType::LinkRequest {
            // RNS records a reverse entry for every other carried packet, so its proof can
            // come back the same way (`Transport.py` 2104-2110).
            shared.remember_reverse(hash, from, out);
        }
        // A route carrying transit is in use, and RNS refreshes it (`Transport.py` 2113).
        shared.touch_path(dest);
        // A request that never left must not hold the slot it was given until its deadline.
        if !forward_on(shared, out, pkt, policy)
            && let Some(link_id) = admitted
        {
            shared.link_transport.lock().unwrap().remove(&link_id);
        }
    }
}

/// What [`admit_link_request`] made of a request.
enum BridgeAdmission {
    /// Drop it.
    Refused,
    /// Carry it; its bridge is new.
    New,
    /// Carry it; it retransmits a request already bridged.
    Known,
}

/// Prepare to carry a link request from `from` to `out`: lower its signalled MTU to what both
/// interfaces carry, and record an unproved bridge with a proof deadline of the per-hop
/// allowance for each hop still ahead plus the outbound first-hop airtime. Refuses a request
/// whose MTU must be lowered under a link mode this node cannot encode, or a full table of
/// recently heard validated links.
fn admit_link_request(
    shared: &Shared,
    from: InterfaceId,
    out: InterfaceId,
    link_id: AddressHash,
    pkt: &mut Packet,
) -> BridgeAdmission {
    let limit = [from, out]
        .into_iter()
        .filter_map(|iface| shared.link_mtu_on(iface))
        .min();
    if let Some(limit) = limit
        && link::clamp_request_mtu(pkt, limit).is_err()
    {
        return BridgeAdmission::Refused;
    }
    let hops = shared
        .path_table
        .lock()
        .unwrap()
        .get(&pkt.destination)
        .map_or(0, |entry| entry.hops);
    // RNS adds the outbound interface's MTU airtime (`Transport.py` 2059-2062, 3200-3202).
    let allowance =
        crate::node::transit_proof_timeout(hops).saturating_add(shared.first_hop_airtime(out));
    let now = Instant::now();
    let bridge = LinkBridge {
        from,
        out,
        destination: pkt.destination,
        seen: now,
        proof_deadline: Some(now + Duration::from_millis(allowance)),
    };
    let mut bridges = shared.link_transport.lock().unwrap();
    // Prune before inserting, so the work tracks the requests that cause growth.
    bridges.retain(|_, bridge| !bridge.lapsed(now));
    if let Some(existing) = bridges.get_mut(&link_id) {
        // A retransmitted request: an unproved bridge follows it, a validated one stands.
        if existing.proof_deadline.is_some() {
            *existing = bridge;
        }
        return BridgeAdmission::Known;
    }
    if bridges.len() >= LINK_TRANSPORT_CAPACITY {
        let Some(stalest) = bridges
            .iter()
            .filter(|(_, bridge)| bridge.proof_deadline.is_some())
            .min_by_key(|(_, bridge)| bridge.seen)
            .or_else(|| {
                bridges
                    .iter()
                    .filter(|(_, bridge)| now.duration_since(bridge.seen) >= LINK_TRANSPORT_IDLE)
                    .min_by_key(|(_, bridge)| bridge.seen)
            })
            .map(|(id, _)| *id)
        else {
            return BridgeAdmission::Refused;
        };
        bridges.remove(&stalest);
    }
    bridges.insert(link_id, bridge);
    BridgeAdmission::New
}

/// Re-address a forwarded packet for the interface it leaves on (stripping our transport
/// stamp, so `send_on` re-adds the next hop's), bump hops, and send. Transit's single egress
/// point, where egress permission and the forwarded count are enforced.
pub(super) fn forward_on(
    shared: &Arc<Shared>,
    out: InterfaceId,
    mut pkt: Packet,
    policy: &RoutingPolicy,
) -> bool {
    if !policy.allowed_egress.allows(out) {
        shared
            .routing_stats
            .policy_rejected
            .fetch_add(1, Ordering::Relaxed);
        return false;
    }
    if pkt.hops.saturating_add(1) >= policy.max_hops {
        shared
            .routing_stats
            .hop_limit_dropped
            .fetch_add(1, Ordering::Relaxed);
        return false;
    }
    shared
        .routing_stats
        .forwarded_packets
        .fetch_add(1, Ordering::Relaxed);
    pkt.hops += 1;
    pkt.header_type = crate::packet::HeaderType::Type1;
    pkt.transport = None;
    shared.try_send_on_class(out, pkt, TrafficClass::Transit)
}
