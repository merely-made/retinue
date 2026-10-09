//! Opening, accepting, carrying and keeping links alive.

use alloc::vec::Vec;

use super::tables::{derived_iv, is_deduplicated_link_context, is_resource_context, remember_hash};
use super::{Action, Actions, InterfaceId, MIN_LOGICAL_MTU, Node, link_request_timeout};
use crate::hash::AddressHash;
use crate::link::{self, Inbound, LinkMode, LinkTrailer, PendingLink};
use crate::link_liveness::{self, Due, Liveness};
use crate::packet::Packet;

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Open a link to a destination this node has heard announce.
    ///
    /// `ephemeral_seed` is caller-supplied, per attempt, for the same reason every other
    /// key here is: no RNG in the protocol layer. Returns `None` if the peer is unknown or
    /// the pending table is full.
    ///
    /// A destination learned through a transport node is addressed to it (header type 2),
    /// as `Endpoint` does, so the relay carries the request on. Every node learns routes,
    /// whatever its transport policy. A route past its TTL at `now` is not used, whether or
    /// not it has been evicted yet; the request then goes out as header type 1.
    ///
    /// When the proof arrives, [`Node::ingest`] reports [`Action::LinkUp`] and sends the RTT
    /// packet, carrying the time from `now` to the proof, which activates the responder. From
    /// then [`Node::poll`] runs the link's keepalives and stale teardown
    /// ([`crate::link_liveness`]); a link it tears down is reported as [`Action::LinkDown`].
    ///
    /// The request is not retried. If no proof arrives by `now` plus
    /// [`link_request_timeout`] of the route's relay count (zero with no route) plus
    /// `interface`'s [`Self::first_hop_airtime`], the first
    /// [`Node::poll`] at or after that deadline drops it and reports
    /// [`Action::LinkRequestTimedOut`] with its link id, which `link::link_id` reads from the
    /// request. A later proof is ignored.
    ///
    /// Requests already past their deadline at `now` are dropped first, so a full table is
    /// never refused only because the caller has not polled. Each is reported as
    /// [`Action::LinkRequestTimedOut`] ahead of the new request's send, exactly as `poll`
    /// would have. A refusal (unknown peer, or a table full of live requests) drops nothing.
    pub fn open_link(
        &mut self,
        destination: AddressHash,
        interface: InterfaceId,
        ephemeral_seed: &[u8; 64],
        now: u64,
    ) -> Option<Actions<ACTIONS>> {
        let peer = self.book.resolve(destination)?.identity;
        let mut actions = Actions::new();
        for link_id in self.expire_link_requests(now) {
            actions.push(Action::LinkRequestTimedOut { link_id });
        }
        if self.pending.is_full() {
            // Only live requests remain: had any expired, its slot would now be free.
            debug_assert!(actions.is_empty());
            self.refused_links = self.refused_links.saturating_add(1);
            return None;
        }

        let (attempt, mut request) = PendingLink::open(
            destination,
            peer,
            ephemeral_seed,
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: self.logical_mtu,
            },
        );
        let hop = self.next_hop(destination, now);
        let via = hop.and_then(|hop| hop.via);
        request.address_via(via);
        if via.is_some() {
            // RNS refreshes a path each time it inserts a packet into transport by it
            // (`Transport.py` 1406, 1426).
            self.touch_route(destination, now);
        }
        let deadline = now
            .saturating_add(link_request_timeout(hop.map_or(0, |hop| hop.hops)))
            .saturating_add(self.first_hop_airtime(interface));
        let _ = self.pending.push((attempt, deadline, now));

        actions.push(Action::Send {
            interface,
            packet: request,
        });
        Some(actions)
    }

    /// Send application bytes on an established link.
    ///
    /// `iv` is caller-supplied and must not repeat for a link's key. Both ends share that key,
    /// so this holds across the two of them: the packet's hash is remembered, and a copy heard
    /// back (a relay's retransmission) is recognised as our own rather than delivered.
    pub fn send(
        &mut self,
        link_id: AddressHash,
        interface: InterfaceId,
        payload: &[u8],
        iv: &[u8; crate::token::IV_LEN],
    ) -> Option<Actions<ACTIONS>> {
        if payload.len() > self.payload_limits.max_link_payload {
            return None;
        }
        let (link, _, _) = self.links.iter().find(|(l, _, _)| l.id() == link_id)?;
        // CBC always adds a padding block, even for aligned plaintext. Refuse before
        // encryption/allocation, then verify the codec's actual packet size as well.
        let padded = payload
            .len()
            .checked_div(16)?
            .checked_add(1)?
            .checked_mul(16)?;
        let encoded = padded
            .checked_add(crate::token::TOKEN_OVERHEAD)?
            .checked_add(crate::packet::HEADER_MIN_LEN)?;
        let budget = link.mtu().min(self.logical_mtu) as usize;
        if encoded > budget {
            return None;
        }
        let packet = link.data_packet(payload, iv);
        if packet.encoded_len() > budget {
            return None;
        }
        self.remember_sent_link_data(packet.hash());
        let mut actions = Actions::new();
        actions.push(Action::Send { interface, packet });
        Some(actions)
    }

    /// Remember a link data packet we sent. At capacity the oldest is forgotten, so a long
    /// burst can outrun the window; that only lets a late echo through, never drops data.
    fn remember_sent_link_data(&mut self, hash: AddressHash) {
        remember_hash(&mut self.sent_link_data, hash);
    }

    /// A peer wants a link to us.
    pub(super) fn on_link_request(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // Only for the destination this node answers to. Transport requests were handled
        // before local dispatch; anything that reaches here is not ours to answer.
        if packet.destination != self.destination() {
            return;
        }
        let Ok(id) = link::link_id(packet) else {
            return;
        };

        // Already established: the peer did not hear our proof, so send the same one again.
        // A fresh accept here would give the two sides different keys for one link.
        if let Some((_, proof, _)) = self.links.iter().find(|(link, _, _)| link.id() == id) {
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

        let seed = self.responder_seed(&id);
        // No more than was asked for, and as in RNS a signalled 0 means the 500-byte default.
        // A request below the smallest workable budget is held to that floor.
        let asked = match link::request_trailer(packet).ok().flatten() {
            Some(LinkTrailer { mtu: 0, .. }) | None => crate::packet::MTU as u32,
            Some(trailer) => trailer.mtu.max(MIN_LOGICAL_MTU),
        };
        let offered = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: asked.min(self.logical_mtu),
        };
        if let Ok((link, proof)) = link::accept(packet, &self.identity, &seed, offered) {
            let link_id = link.id();
            let _ = self
                .links
                .push((link, proof.clone(), Liveness::responder(now, packet.hops)));
            actions.push(Action::Send {
                interface,
                packet: proof,
            });
            actions.push(Action::LinkUp { link_id });
        }
    }

    /// A proof for a link we opened, or a resource proof for a transfer we are sending.
    pub(super) fn on_proof(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // RNS proves receipt of a resource with a PROOF-type packet on the link. It belongs
        // to an outbound transfer only, so with no sender on that link it is dropped rather
        // than handed to a receiver it could only confuse.
        if packet.context == link::CTX_RESOURCE_PRF {
            let link_id = packet.destination;
            if self.senders.iter().any(|(id, _, _)| *id == link_id)
                && let Some(index) = self
                    .links
                    .iter()
                    .position(|(link, _, _)| link.id() == link_id)
            {
                self.links[index].2.on_inbound(now);
                self.on_resource(interface, link_id, index, packet, now, actions);
            }
            return;
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|(attempt, _, _)| attempt.prove(packet).is_ok())
        else {
            return;
        };
        let (attempt, _, opened) = self.pending.swap_remove(index);
        let Ok(link) = attempt.prove(packet) else {
            return;
        };
        if self.links.is_full() {
            self.refused_links = self.refused_links.saturating_add(1);
            return;
        }
        let link_id = link.id();
        // The request is never retried, so the proof times exactly one round trip. The
        // responder stays in its handshake until it hears the RTT packet. It follows the
        // `LinkUp`, which a full action buffer must not be the one to lose; a responder
        // takes data before the RTT packet anyway.
        let rtt = now.saturating_sub(opened);
        let seed = self.identity.to_secret_bytes();
        let iv = derived_iv(&seed, link_id, &mut self.iv_counter);
        let rtt_packet = link.rtt_packet(link_liveness::rtt_seconds(rtt), &iv);
        // Our own proof has no place here: this side was the initiator, so there is nothing
        // to re-send. The stored packet is the proof we received, kept only for symmetry.
        let _ = self
            .links
            .push((link, packet.clone(), Liveness::initiator(rtt, now)));
        actions.push(Action::LinkUp { link_id });
        actions.push(Action::Send {
            interface,
            packet: rtt_packet,
        });
    }

    /// Traffic on an established link.
    pub(super) fn on_link_data(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        let link_id = packet.destination;
        let Some(index) = self
            .links
            .iter()
            .position(|(link, _, _)| link.id() == link_id)
        else {
            return;
        };

        // Our own packet, heard back from a relay. Not the far end's data, and not evidence
        // that the far end is alive.
        if self.sent_link_data.contains(&packet.hash()) {
            self.transport_counters.own_echo_dropped =
                self.transport_counters.own_echo_dropped.saturating_add(1);
            return;
        }

        // A sender asking again for a resource proof it did not hear. Answered from the
        // proof kept for this link, before the duplicate window: the sender repeats the
        // request verbatim, and every copy deserves an answer, up to a cap.
        if packet.context == link::CTX_CACHE_REQUEST {
            if let Some(proof) =
                self.resend_kept_proof(link_id, |proof| packet.payload[..] == proof.full_hash()[..])
            {
                actions.push(Action::Send {
                    interface,
                    packet: proof,
                });
            }
            return;
        }

        // The far end's packet heard a second time, directly and from a relay. Dropped
        // before the liveness stamp, as the echo is: the copy is no newer than the original.
        if is_deduplicated_link_context(packet.context) {
            let hash = packet.hash();
            if self.received_link_data.contains(&hash) {
                self.transport_counters.duplicate_dropped =
                    self.transport_counters.duplicate_dropped.saturating_add(1);
                return;
            }
            remember_hash(&mut self.received_link_data, hash);
        }

        // Keepalives are answered here, and count as liveness only from the peer's role.
        if packet.context == link::CTX_KEEPALIVE {
            if self.links[index].2.on_keepalive(&packet.payload, now) {
                actions.push(Action::Send {
                    interface,
                    packet: self.links[index]
                        .0
                        .keepalive_packet(link::KEEPALIVE_RESPONSE),
                });
            }
            return;
        }

        // Heard from: this is what keeps the slot. Recorded before dispatching, so a
        // resource transfer counts as liveness exactly as a keepalive does.
        self.links[index].2.on_inbound(now);

        // Resource contexts are a transfer's business, not the link's.
        if is_resource_context(packet.context) {
            self.on_resource(interface, link_id, index, packet, now, actions);
            return;
        }

        match self.links[index].0.receive(packet) {
            Some(Inbound::Data(payload)) => {
                actions.push(Action::Data { link_id, payload });
            }
            Some(Inbound::Close) => {
                self.drop_link(index, actions);
            }
            Some(Inbound::Rtt) => {
                if let Some(rtt) = link_liveness::read_rtt(&self.links[index].0, packet) {
                    self.links[index].2.on_rtt(rtt, now);
                }
            }
            // Requests and responses are not handled yet: dropped, not mishandled, and pinned
            // by a test.
            _ => {}
        }
    }

    /// Keepalives, stale teardown and the responder's handshake deadline, for every link.
    /// A link torn down here is reported as [`Action::LinkDown`] and counted as expired.
    pub(super) fn poll_liveness(
        &mut self,
        now: u64,
        interface: InterfaceId,
        actions: &mut Actions<ACTIONS>,
    ) {
        let seed = self.identity.to_secret_bytes();
        let mut index = 0;
        while index < self.links.len() {
            let (link, _, liveness) = &mut self.links[index];
            let packet = match liveness.poll(now) {
                None => {
                    index += 1;
                    continue;
                }
                Some(Due::Keepalive) => {
                    actions.push(Action::Send {
                        interface,
                        packet: link.keepalive_packet(link::KEEPALIVE_REQUEST),
                    });
                    index += 1;
                    continue;
                }
                Some(Due::Teardown) => {
                    let iv = derived_iv(&seed, link.id(), &mut self.iv_counter);
                    Some(link.close_packet(&iv))
                }
                Some(Due::HandshakeTimeout) => None,
            };
            if let Some(packet) = packet {
                actions.push(Action::Send { interface, packet });
            }
            self.expired_links = self.expired_links.saturating_add(1);
            // `drop_link` swaps the last link into `index`, which is examined next.
            self.drop_link(index, actions);
        }
    }

    /// Drop a link and everything riding on it.
    fn drop_link(&mut self, index: usize, actions: &mut Actions<ACTIONS>) {
        let link_id = self.links[index].0.id();
        self.links.swap_remove(index);
        // A transfer without its link can never finish, so it goes too.
        self.receivers.retain(|(id, _, _)| *id != link_id);
        self.senders.retain(|(id, _, _)| *id != link_id);
        self.resource_proofs.retain(|(id, _, _, _)| *id != link_id);
        actions.push(Action::LinkDown { link_id });
    }

    /// A responder ephemeral seed, derived from our identity and the link id.
    ///
    /// This layer has no RNG, and a responder answers requests it did not ask for, so its seed
    /// is derived. It stays unpredictable without our private key, and a retransmitted request
    /// reproduces the same proof.
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
}
