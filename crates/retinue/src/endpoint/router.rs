//! The router: dispatch of every inbound packet.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::announce_admission::InterfaceVerdict;
use crate::link::{self, Inbound, LinkMode, LinkTrailer};
use crate::link_liveness::Liveness;
use crate::packet::{DestinationType, Packet, PacketType};

use super::announces::{HeldAnnounce, process_verified_announce, start_held_announce_release};
use super::dedup::LinkPacketAdmission;
use super::entropy::{ephemeral_seed, next_iv};
use super::facts::{LinkDirection, LinkRemoteFact};
use super::inbound::{
    Accepted, AcceptedResource, Admission, LINK_REQUEST_CACHE, LINK_REQUEST_CACHE_TTL,
};
use super::interface::InterfaceId;
use super::queue::TrafficClass;
use super::registration::RegistrationKind;
use super::reliable_driver::register_reliable_stream;
use super::resource_session::register_resource_session;
use super::shared::{LinkKind, Shared};
use super::single::deliver_single;
use super::stream::{StreamFault, register_stream};
use super::transit::{forward, forward_on};
use super::watchdog::note_link_inbound;

/// Dispatch one inbound packet that arrived on `iface`.
pub(super) fn route(shared: &Arc<Shared>, iface: InterfaceId, pkt: Packet) {
    // RNS's packet filter, ahead of any dispatch (`Transport.py` 1629-1660): a header-type-2
    // packet is carried only by the transport it names, and a PLAIN or GROUP packet never
    // travels past its first hop.
    if pkt.packet_type != PacketType::Announce
        && ((pkt.header_type == crate::packet::HeaderType::Type2
            && pkt.transport != Some(shared.identity.public().hash()))
            || (matches!(
                pkt.destination_type,
                DestinationType::Plain | DestinationType::Group
            ) && pkt.hops > 1))
    {
        shared
            .routing_stats
            .filtered_packets
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    // Answer a path request for one of our destinations with a path response. RNS drops a
    // tagless request and a repeated target and tag (`Transport.py` 1838-1856), and answers
    // on the requesting interface only (`Transport.py` 3452-3456).
    if let Some(request) = crate::path::PathRequest::parse(&pkt) {
        let fresh = request
            .unique_tag()
            .is_some_and(|tag| shared.path_request_tags.lock().unwrap().insert(tag));
        if !fresh {
            shared
                .routing_stats
                .filtered_packets
                .fetch_add(1, Ordering::Relaxed);
        } else if let Some(resp) = shared.path_response(request.target) {
            shared.send_on_class(iface, resp, TrafficClass::Control);
        }
        return;
    }
    // Transit. Announces are relayed from their own arm, so they still reach the book.
    let policy = shared.routing.lock().unwrap().clone();
    if pkt.packet_type != PacketType::Announce {
        // A packet for a link we bridge crosses to the other side whatever its header type,
        // since the two ends may address it differently. Traffic from either end refreshes
        // the bridge; a third interface can neither use nor refresh it.
        let bridged = {
            let mut bridges = shared.link_transport.lock().unwrap();
            let now = Instant::now();
            match bridges.get_mut(&pkt.destination) {
                Some(bridge) if !bridge.lapsed(now) => {
                    let Some(proves) = bridge.admit(iface, &pkt, &shared.address_book) else {
                        shared
                            .routing_stats
                            .policy_rejected
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    bridge.seen = now;
                    Some((bridge.from, bridge.out, proves))
                }
                _ => None,
            }
        };
        let addressed_to_us_as_hop = pkt.header_type == crate::packet::HeaderType::Type2
            && pkt.transport == Some(shared.identity.public().hash());

        if bridged.is_some() || addressed_to_us_as_hop {
            // Transit, never ours: a refusal drops it rather than handling it locally.
            if !policy.accepts_transit_from(iface) {
                shared
                    .routing_stats
                    .policy_rejected
                    .fetch_add(1, Ordering::Relaxed);
                return;
            }
            match bridged {
                Some((a, b, proves)) => {
                    let link_id = pkt.destination;
                    let out = if iface == a { b } else { a };
                    // As in RNS, the bridge is validated as its proof goes out, not before.
                    if forward_on(shared, out, pkt, &policy)
                        && proves
                        && let Some(bridge) =
                            shared.link_transport.lock().unwrap().get_mut(&link_id)
                    {
                        bridge.proof_deadline = None;
                    }
                }
                None => forward(shared, iface, pkt, &policy),
            }
            return;
        }
    }
    match pkt.packet_type {
        PacketType::Announce => {
            // An echo of our own announce: drop it before it costs a signature check or
            // becomes a path to ourselves, a `PeerAnnounce`, or a relay.
            if shared
                .registered
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.dest == pkt.destination)
            {
                return;
            }
            // A replay or stale emission needs no signature check, and a copy of an announce
            // that verified recently skips it. A neighbour relaying the announce we hold for
            // rebroadcast repeats its blob, so it is turned away here; it still ends our retry.
            if shared.announce_is_stale_unverified(&pkt) {
                if pkt.transport.is_some() {
                    shared.hear_rebroadcast_copy(&pkt);
                }
                return;
            }
            let decoded = shared.verified_announces.lock().unwrap().decode(&pkt);
            if let Ok(a) = decoded {
                if pkt.transport.is_some() && !shared.address_book.lock().unwrap().key_conflicts(&a)
                {
                    shared.hear_rebroadcast(a.destination, pkt.hops);
                }
                let route_is_known = shared
                    .path_table
                    .lock()
                    .unwrap()
                    .contains_key(&a.destination);
                let verdict = shared.announce_admission.lock().unwrap().observe_interface(
                    iface,
                    route_is_known,
                    shared.announce_admission_now_ms(),
                );
                match verdict {
                    InterfaceVerdict::Process => {
                        process_verified_announce(shared, iface, pkt, a);
                    }
                    InterfaceVerdict::Hold { release_at_ms } => {
                        let held = HeldAnnounce {
                            interface: iface,
                            packet: pkt,
                            announce: a,
                        };
                        if shared.hold_announce(held) {
                            shared.announce_admission.lock().unwrap().note_held(iface);
                            shared
                                .routing_stats
                                .held_announces
                                .fetch_add(1, Ordering::Relaxed);
                            start_held_announce_release(shared, iface, release_at_ms);
                        } else {
                            shared
                                .announce_admission
                                .lock()
                                .unwrap()
                                .note_held_dropped(iface);
                            shared
                                .routing_stats
                                .held_announces_dropped
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        PacketType::LinkRequest => {
            if !shared.is_running() {
                return;
            }
            let dest = pkt.destination;
            let kind = shared
                .registered
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.dest == dest)
                .map(|r| r.kind);
            if let Some(kind) = kind {
                let request_link_id = link::link_id(&pkt).ok();
                if let Some(link_id) = request_link_id {
                    let cached = {
                        let mut cache = shared.inbound_link_proofs.lock().unwrap();
                        cache.retain(|_, (_, at)| at.elapsed() < LINK_REQUEST_CACHE_TTL);
                        cache.get(&link_id).map(|(proof, _)| proof.clone())
                    };
                    if let Some(proof) = cached {
                        shared.send_on(iface, proof);
                        return;
                    }
                }
                // Refuse past the caps before spending key agreement, a task, or a buffer.
                // Only the router admits links, so the room seen here is still there below.
                let admission = shared.inbound.lock().unwrap().admission(dest);
                if matches!(admission, Admission::Refuse) {
                    shared
                        .routing_stats
                        .inbound_links_refused
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let ephemeral = ephemeral_seed();
                // As in RNS, the link is no larger than the interface that heard its request.
                let configured_mtu = shared.link_mtu.load(Ordering::Relaxed);
                let configured_mtu = shared
                    .link_mtu_on(iface)
                    .map_or(configured_mtu, |limit| configured_mtu.min(limit));
                let requested_mtu = pkt
                    .payload
                    .get(link::LINK_KEYS_LEN..link::LINK_KEYS_LEN + link::TRAILER_LEN)
                    .and_then(|bytes| bytes.try_into().ok())
                    .and_then(|trailer| LinkTrailer::decode(trailer).ok())
                    .map(|trailer| trailer.mtu)
                    // As in RNS a signalled 0 means the default; a smaller request is floored.
                    .filter(|&mtu| mtu != 0)
                    .map_or(configured_mtu, |mtu| mtu.max(crate::node::MIN_LOGICAL_MTU));
                if let Ok((link, proof)) = link::accept(
                    &pkt,
                    &shared.identity,
                    &ephemeral,
                    LinkTrailer {
                        mode: LinkMode::Aes256Cbc,
                        mtu: configured_mtu.min(requested_mtu),
                    },
                ) {
                    {
                        let mut cache = shared.inbound_link_proofs.lock().unwrap();
                        if cache.len() >= LINK_REQUEST_CACHE {
                            cache.retain(|_, (_, at)| at.elapsed() < LINK_REQUEST_CACHE_TTL);
                        }
                        if cache.len() >= LINK_REQUEST_CACHE
                            && let Some(oldest) = cache
                                .iter()
                                .min_by_key(|(_, (_, at))| *at)
                                .map(|(id, _)| *id)
                        {
                            cache.remove(&oldest);
                        }
                        cache.insert(link.id(), (proof.clone(), Instant::now()));
                    }
                    let link_id = link.id();
                    if let Admission::Evict(stale) = admission {
                        shared.evict_inbound(stale);
                    }
                    shared.inbound.lock().unwrap().admit(link_id, dest);
                    shared.send_on(iface, proof);
                    let liveness = Liveness::responder(shared.link_clock_ms(), pkt.hops);
                    match kind {
                        RegistrationKind::Reliable => {
                            // No peer yet: the driver learns it from the IDENTIFY.
                            if let Some(stream) = register_reliable_stream(
                                shared,
                                link,
                                iface,
                                liveness,
                                None,
                                LinkDirection::Inbound,
                                LinkRemoteFact::default(),
                            ) {
                                shared.inbound.lock().unwrap().queued(dest);
                                let _ = shared.reliable_accepted_tx.send(Accepted {
                                    stream,
                                    destination: dest,
                                    interface: iface,
                                });
                            }
                        }
                        RegistrationKind::Resource => {
                            if let Some(session) = register_resource_session(
                                shared,
                                link,
                                iface,
                                liveness,
                                LinkDirection::Inbound,
                                LinkRemoteFact::default(),
                            ) {
                                shared.inbound.lock().unwrap().queued(dest);
                                let _ = shared.resource_accepted_tx.send(AcceptedResource {
                                    session,
                                    destination: dest,
                                    interface: iface,
                                });
                            } else {
                                shared.remove_link(link_id);
                            }
                        }
                        RegistrationKind::BestEffort => {
                            if let Some(stream) = register_stream(
                                shared,
                                link,
                                iface,
                                liveness,
                                LinkDirection::Inbound,
                                LinkRemoteFact::default(),
                            ) {
                                shared.inbound.lock().unwrap().queued(dest);
                                let _ = shared.accepted_tx.send(Accepted {
                                    stream,
                                    destination: dest,
                                    interface: iface,
                                });
                            }
                        }
                    }
                }
            }
        }
        PacketType::Proof => {
            // A single-packet proof: carry it back the way its packet came, or conclude one
            // of our receipts (RNS `Transport.py` 2724-2761).
            if !matches!(pkt.context, link::CTX_LRPROOF | link::CTX_RESOURCE_PRF) {
                if let Some(back) = shared.take_reverse(pkt.destination, iface) {
                    if policy.accepts_transit_from(iface) {
                        forward_on(shared, back, pkt, &policy);
                    } else {
                        shared
                            .routing_stats
                            .policy_rejected
                            .fetch_add(1, Ordering::Relaxed);
                    }
                    return;
                }
                if shared.conclude_single_receipt(&pkt) {
                    return;
                }
            }
            // Complete a pending outbound link on the interface the proof came in on. Only a
            // proof that verifies removes the pending link, so a forgery cannot strand the
            // genuine proof behind it.
            let proved = {
                let mut pend = shared.pending_links.lock().unwrap();
                let link = pend.get(&pkt.destination).and_then(|p| p.prove(&pkt).ok());
                if link.is_some() {
                    pend.remove(&pkt.destination);
                }
                link
            };
            if let Some(link) = proved {
                if let Some(tx) = shared.pending.lock().unwrap().remove(&pkt.destination) {
                    let _ = tx.send((link, iface));
                }
            } else {
                // A link-data proof, for the reliable or resource driver. Best-effort links
                // never request proofs.
                let packets = {
                    let links = shared.links.lock().unwrap();
                    let entry = links.get(&pkt.destination);
                    // RNS hands a resource proof to the link, which holds it to the link's
                    // interface (`Link.py` 938-941); other proofs conclude receipts
                    // wherever they arrive.
                    if pkt.context == link::CTX_RESOURCE_PRF
                        && entry.is_some_and(|e| e.iface != iface)
                    {
                        shared
                            .routing_stats
                            .filtered_packets
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    entry.and_then(|e| match &e.kind {
                        LinkKind::Reliable { packets } | LinkKind::Resource { packets } => {
                            Some(packets.clone())
                        }
                        LinkKind::BestEffort { .. } => None,
                    })
                };
                note_link_inbound(shared, &pkt);
                if let Some(packets) = packets {
                    shared.queue_link_packet(&packets, pkt);
                }
            }
        }
        PacketType::Data => {
            // Link data, by the link's discipline. Senders are cloned under the lock and
            // used after it is released.
            let (link, raw, best) = {
                let links = shared.links.lock().unwrap();
                match links.get(&pkt.destination) {
                    // A link's packets arrive on its own interface (`Link.py` 938-941,
                    // `Transport.py` 2573-2574). Refused before the duplicate memory sees it,
                    // so the genuine copy still counts when it arrives (`Transport.py`
                    // 2585-2593).
                    Some(e) if e.iface != iface => {
                        shared
                            .routing_stats
                            .filtered_packets
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                    Some(e) => match &e.kind {
                        LinkKind::Reliable { packets } | LinkKind::Resource { packets } => {
                            (Some(e.link.clone()), Some(packets.clone()), None)
                        }
                        LinkKind::BestEffort { inbound, fault } => (
                            Some(e.link.clone()),
                            None,
                            Some((inbound.clone(), Arc::clone(fault), e.iface)),
                        ),
                    },
                    None => (None, None, None),
                }
            };
            // Our own packet heard back, or the far end's heard twice, is not new traffic.
            if raw.is_some() || best.is_some() {
                let admission = shared.link_packets.lock().unwrap().admit(&pkt);
                let dropped = match admission {
                    LinkPacketAdmission::New => None,
                    LinkPacketAdmission::OwnEcho => Some(&shared.routing_stats.own_echo_dropped),
                    LinkPacketAdmission::Duplicate => Some(&shared.routing_stats.duplicate_dropped),
                };
                if let Some(counter) = dropped {
                    counter.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                // Keepalives and the RTT packet end here, but the RTT packet also activates an
                // inbound link (as in RNS), so the inbound cap sees it first.
                if note_link_inbound(shared, &pkt) {
                    if let Some(link) = &link {
                        shared.note_inbound_traffic(link, &pkt);
                    }
                    return;
                }
                // A publisher asking again for a lost resource proof, answered from the kept
                // one as RNS answers from its packet cache. Publishers repeat these verbatim,
                // so they are exempt from the duplicate window.
                if pkt.context == link::CTX_CACHE_REQUEST {
                    if let Some(proof) = shared.resend_resource_proof(pkt.destination, |proof| {
                        proof.full_hash()[..] == pkt.payload[..]
                    }) {
                        shared.send_on(iface, proof);
                    }
                    return;
                }
                // A resource offered again after we proved it: its proof was lost.
                if pkt.context == link::CTX_RESOURCE_ADV
                    && let Some(proof) = shared.proof_for_advertisement(pkt.destination, &pkt)
                {
                    shared.send_on(iface, proof);
                    return;
                }
            }
            if let Some(link) = &link {
                shared.note_inbound_traffic(link, &pkt);
            }
            if let Some(packets) = raw {
                // The driver owns it; a reliable one recovers a drop by retransmission.
                shared.queue_link_packet(&packets, pkt);
            } else if let (Some(link), Some((inbound, fault, link_iface))) = (link, best) {
                match link.receive(&pkt) {
                    Some(Inbound::Data(bytes)) => {
                        if let Err(mpsc::error::TrySendError::Full(_)) = inbound.try_send(bytes) {
                            // Nothing re-sends best-effort data: fail the stream rather than
                            // leave a silent hole in it.
                            shared
                                .routing_stats
                                .link_queue_dropped
                                .fetch_add(1, Ordering::Relaxed);
                            *fault.lock().unwrap() = Some(StreamFault::Overrun);
                            shared.send_on(link_iface, link.close_packet(&next_iv()));
                            shared.remove_link(pkt.destination);
                        }
                    }
                    Some(Inbound::Close) => {
                        // Dropping the entry releases the inbound sender, so the local
                        // reader sees EOF.
                        shared.remove_link(pkt.destination);
                    }
                    _ => {}
                }
            } else if pkt.destination_type == DestinationType::Single {
                // One hash serves the packet filter and the delivery's proof.
                let full = pkt.full_hash();
                if shared.packet_is_new(pkt.context, crate::proof::truncated(&full)) {
                    deliver_single(shared, iface, &pkt, full);
                }
            }
        }
    }
}
