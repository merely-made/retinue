//! The router: dispatch of every inbound packet.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use tokio::sync::mpsc;

use crate::announce::Announce;
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
    // A path request for a destination we own: answer it with a path response (an announce
    // carrying context 0x0b) so a peer that lost its route to us can rediscover it. RNS drops
    // a tagless request and a repeated target and tag (`Transport.py` 1838-1856), and answers
    // for a local destination on the requesting interface only (`Transport.py` 3452-3456).
    // We answer only for our own destinations; with no announce cache we cannot answer for
    // others.
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
    // Transport-node forwarding (announces are re-forwarded in their own arm instead, so
    // they still populate our address book).
    let policy = shared.routing.lock().unwrap().clone();
    if pkt.packet_type != PacketType::Announce {
        // A packet whose destination is a link we bridge goes to the opposite side, whatever
        // its header type: the two endpoints may address it differently (one type-2 through
        // us, one type-1 direct, e.g. a responder that never learned it is behind us).
        // Traffic from either end of a bridge is proof it is still wanted. A packet
        // arriving on a third interface cannot use or refresh that bridge.
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
        // A header-type-2 packet addressed to us as the transport hop is likewise someone
        // else's traffic asking to be carried.
        let addressed_to_us_as_hop = pkt.header_type == crate::packet::HeaderType::Type2
            && pkt.transport == Some(shared.identity.public().hash());

        if bridged.is_some() || addressed_to_us_as_hop {
            // This is transit, not ours. Policy decides whether we carry it; a refusal is
            // counted and the packet is dropped rather than falling through to local
            // handling, since we are not its destination either way.
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
            // A relay rebroadcasting one of our own announces echoes it back. We are not our
            // own peer: drop it before it costs a signature check, or becomes a path to
            // ourselves, a `PeerAnnounce`, or a relay.
            if shared
                .registered
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.dest == pkt.destination)
            {
                return;
            }
            if let Ok(a) = Announce::decode(&pkt) {
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
                // Refuse a new link past the inbound caps or its destination's full accept
                // backlog before any key agreement, task, or buffer is spent on it. Only the
                // router admits links, so the room seen here is still there below.
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
                    // As in RNS a signalled 0 means the default; a request below the smallest
                    // workable link is held to that floor.
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
                            // Register eagerly with no peer yet: the driver learns the
                            // initiator's identity from the IDENTIFY it sends.
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
            // Complete a pending outbound link, binding it to the interface it came in on.
            // Validate the proof against the pending link BEFORE removing it: a forged proof
            // addressed to a real pending link id must not be able to evict it and strand the
            // genuine proof that follows. Only a proof that actually verifies removes it.
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
                // Otherwise a link-data proof for an established link: hand it to the
                // reliable driver, which matches its hash to an outstanding sequence.
                // Best-effort links never request proofs, so there is nothing to do.
                let packets = shared
                    .links
                    .lock()
                    .unwrap()
                    .get(&pkt.destination)
                    .and_then(|e| match &e.kind {
                        LinkKind::Reliable { packets } | LinkKind::Resource { packets } => {
                            Some(packets.clone())
                        }
                        LinkKind::BestEffort { .. } => None,
                    });
                note_link_inbound(shared, &pkt);
                if let Some(packets) = packets {
                    shared.queue_link_packet(&packets, pkt);
                }
            }
        }
        PacketType::Data => {
            // Link data: route to the matching stream by its delivery discipline. Clone the
            // sender(s) under the lock, then act on the packet once the lock is released.
            let (link, raw, best) = {
                let links = shared.links.lock().unwrap();
                match links.get(&pkt.destination) {
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
            // On one of our links, our own packet heard back or the far end's heard twice
            // is not new traffic, whichever discipline the link uses.
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
                // Heard from the peer. Keepalives and the RTT packet end here, but the RTT
                // packet is also what activates an inbound link (RNS activates on it), so the
                // inbound cap must see it first.
                if note_link_inbound(shared, &pkt) {
                    if let Some(link) = &link {
                        shared.note_inbound_traffic(link, &pkt);
                    }
                    return;
                }
                // A publisher asking again for a resource proof it did not hear: answered
                // from the proof kept for this link, as RNS's transport answers from its
                // packet cache. Cache requests are exempt from the duplicate window above,
                // because a publisher repeats them verbatim.
                if pkt.context == link::CTX_CACHE_REQUEST {
                    if let Some(proof) = shared.resend_resource_proof(pkt.destination, |proof| {
                        proof.full_hash()[..] == pkt.payload[..]
                    }) {
                        shared.send_on(iface, proof);
                    }
                    return;
                }
                // The same resource offered again after this side proved it: its proof
                // was lost. Answer with the kept one rather than receive it all again.
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
                // The reliable or resource driver owns this packet; hand it over raw. A
                // reliable driver recovers a dropped one by retransmission.
                shared.queue_link_packet(&packets, pkt);
            } else if let (Some(link), Some((inbound, fault, link_iface))) = (link, best) {
                match link.receive(&pkt) {
                    Some(Inbound::Data(bytes)) => {
                        if let Err(mpsc::error::TrySendError::Full(_)) = inbound.try_send(bytes) {
                            // Nothing re-sends best-effort data, so a dropped chunk would
                            // be a silent hole in the byte stream. Fail the stream instead:
                            // its reader gets what arrived, then an error.
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
                        // The peer closed the link: drop its entry so the inbound
                        // sender is released. The stream's inbound relay then ends
                        // and the local reader sees EOF (what read-to-end needs).
                        shared.remove_link(pkt.destination);
                    }
                    _ => {}
                }
            } else if pkt.destination_type == DestinationType::Single && shared.packet_is_new(&pkt)
            {
                deliver_single(shared, iface, &pkt);
            }
        }
    }
}
