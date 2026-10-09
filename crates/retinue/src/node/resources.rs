//! Resource transfers riding on links.

use super::tables::derived_iv;
use super::{Action, Actions, InterfaceId, Node, RESOURCE_FALLBACK_RTT};
use crate::hash::AddressHash;
use crate::link;
use crate::link_liveness::Liveness;
use crate::packet::Packet;
use crate::resource::WINDOW_MAX;
use crate::resource_transfer::{ResourceReceiver, ResourceSender, Timing};

/// A link's transfer timing: its measured RTT, or the fallback until there is one.
fn timing(liveness: &Liveness) -> Timing {
    Timing {
        rtt: liveness.rtt().unwrap_or(RESOURCE_FALLBACK_RTT),
        floor: 0,
    }
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Publish a resource on an established link.
    ///
    /// `random_hash` and `iv` are caller-supplied, per the same no-RNG discipline as
    /// everything else here. Returns `None` if the link is unknown or a transfer is already
    /// running on it: one at a time, because a board cannot hold two.
    pub fn publish(
        &mut self,
        link_id: AddressHash,
        interface: InterfaceId,
        data: &[u8],
        random_hash: [u8; crate::resource::RANDOM_HASH_LEN],
        iv: &[u8; crate::token::IV_LEN],
        now: u64,
    ) -> Option<Actions<ACTIONS>> {
        self.start_sender(link_id, interface, data, None, random_hash, iv, now)
    }

    /// [`publish`](Self::publish) with `metadata`, one already-packed msgpack value that
    /// reaches the receiver beside the data (RNS `Resource.py` 261-272). The metadata counts
    /// toward the outbound size limit.
    #[allow(clippy::too_many_arguments)]
    pub fn publish_with_metadata(
        &mut self,
        link_id: AddressHash,
        interface: InterfaceId,
        data: &[u8],
        metadata: &[u8],
        random_hash: [u8; crate::resource::RANDOM_HASH_LEN],
        iv: &[u8; crate::token::IV_LEN],
        now: u64,
    ) -> Option<Actions<ACTIONS>> {
        self.start_sender(
            link_id,
            interface,
            data,
            Some(metadata),
            random_hash,
            iv,
            now,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn start_sender(
        &mut self,
        link_id: AddressHash,
        interface: InterfaceId,
        data: &[u8],
        metadata: Option<&[u8]>,
        random_hash: [u8; crate::resource::RANDOM_HASH_LEN],
        iv: &[u8; crate::token::IV_LEN],
        now: u64,
    ) -> Option<Actions<ACTIONS>> {
        // Metadata travels framed by a three-byte length in front of the data.
        let framed = data.len() + metadata.map_or(0, |m| 3 + m.len());
        if framed > self.payload_limits.max_outbound_resource {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return None;
        }
        if self.senders.iter().any(|(id, _, _)| *id == link_id) || self.senders.is_full() {
            return None;
        }
        let (link, _, liveness) = self.links.iter().find(|(l, _, _)| l.id() == link_id)?;

        let sender = match metadata {
            None => ResourceSender::publish(link.clone(), data, random_hash, iv),
            Some(metadata) => {
                ResourceSender::publish_with_metadata(link.clone(), data, metadata, random_hash, iv)
                    .ok()?
            }
        };
        let mut sender = sender.with_timing(timing(liveness));
        let advertisement = sender.advertise(now, iv);
        let _ = self.senders.push((link_id, sender, now));

        let mut actions = Actions::new();
        actions.push(Action::Send {
            interface,
            packet: advertisement,
        });
        Some(actions)
    }

    /// The tick at which [`Node::poll`] next has resource work: a part, request,
    /// advertisement or proof overdue. `None` while no transfer runs.
    pub fn resource_deadline(&self) -> Option<u64> {
        let receivers = self.receivers.iter().filter_map(|(_, r, _)| r.deadline());
        let senders = self.senders.iter().filter_map(|(_, s, _)| s.deadline());
        receivers.chain(senders).min()
    }

    /// Whether a resource is being received or sent on this link.
    pub fn transfer_active(&self, link_id: AddressHash) -> bool {
        self.receivers.iter().any(|(id, _, _)| *id == link_id)
            || self.senders.iter().any(|(id, _, _)| *id == link_id)
    }

    /// A packet belonging to a resource transfer on this link.
    pub(super) fn on_resource(
        &mut self,
        interface: InterfaceId,
        link_id: AddressHash,
        link_index: usize,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // Derived IVs, as this layer holds no RNG. The counter is written back to node state
        // so the sequence never restarts under a link key.
        let seed = self.identity.to_secret_bytes();
        let mut counter = self.iv_counter;
        let mut iv = || derived_iv(&seed, link_id, &mut counter);

        // An outbound transfer's replies come back on the same link, so try the sender
        // first: only one direction can own a given context on a given link at a time.
        if let Some(pos) = self.senders.iter().position(|(id, _, _)| *id == link_id) {
            let replies = self.senders[pos].1.on_packet(packet, now, &mut iv);
            self.iv_counter = counter;
            self.senders[pos].2 = now;
            let finished = self.senders[pos].1.is_done() || self.senders[pos].1.is_canceled();
            for reply in replies {
                actions.push(Action::Send {
                    interface,
                    packet: reply,
                });
            }
            if finished {
                self.senders.swap_remove(pos);
            }
            return;
        }

        let existing = self.receivers.iter().position(|(id, _, _)| *id == link_id);
        let is_new = existing.is_none();
        let pos = match existing {
            Some(pos) => pos,
            None => {
                // A re-advertisement of a resource already proved: an older retinue sender
                // lost the proof and offers again instead of a cache request. Answer with the
                // kept proof rather than receiving and delivering it twice.
                if packet.context == link::CTX_RESOURCE_ADV
                    && self
                        .resource_proofs
                        .iter()
                        .any(|(id, _, _, _)| *id == link_id)
                    && let Some(advertised) = self.links[link_index]
                        .0
                        .decrypt(packet)
                        .ok()
                        .and_then(|plain| crate::resource::Advertisement::parse(&plain).ok())
                    && let Some(proof) = self.resend_kept_proof(link_id, |proof| {
                        crate::resource::parse_proof(&proof.payload)
                            .is_some_and(|(hash, _)| hash[..] == advertised.resource_hash[..])
                    })
                {
                    actions.push(Action::Send {
                        interface,
                        packet: proof,
                    });
                    return;
                }
                if self.receivers.is_full() {
                    self.refused_offers = self.refused_offers.saturating_add(1);
                    // Tell the sender, as RNS rejects an offer it will not take, rather than
                    // leave it re-advertising into a full table.
                    if packet.context == link::CTX_RESOURCE_ADV
                        && let Some(reject) = crate::resource_transfer::reject(
                            &self.links[link_index].0,
                            packet,
                            &iv(),
                        )
                    {
                        self.iv_counter = counter;
                        actions.push(Action::Send {
                            interface,
                            packet: reject,
                        });
                    }
                    return;
                }
                let (link, _, liveness) = &self.links[link_index];
                let receiver = ResourceReceiver::with_limits(
                    link.clone(),
                    WINDOW_MAX,
                    self.payload_limits.max_resource_parts,
                )
                .with_timing(timing(liveness));
                let _ = self.receivers.push((link_id, receiver, now));
                self.receivers.len() - 1
            }
        };

        let replies = self.receivers[pos].1.on_packet(packet, now, &mut iv);
        self.iv_counter = counter;
        self.receivers[pos].2 = now;

        // A receiver created for this packet that then said nothing did not accept the
        // transfer (an advertisement past the part ceiling). Keeping it would hold a slot.
        if is_new && replies.is_empty() && self.receivers[pos].1.data().is_none() {
            self.receivers.swap_remove(pos);
            self.refused_offers = self.refused_offers.saturating_add(1);
            return;
        }

        for reply in replies {
            actions.push(Action::Send {
                interface,
                packet: reply,
            });
        }

        if self.receivers[pos].1.is_complete() {
            let carry = self.receivers[pos].1.carry();
            self.links[link_index].0.set_resource_carry(carry);
            // Keep the proof a while, for the sender's cache request if it was lost.
            if let Some(proof) = self.receivers[pos].1.proof_packet() {
                self.resource_proofs.retain(|(id, _, _, _)| *id != link_id);
                let _ = self.resource_proofs.push((link_id, proof, now, 0));
            }
            if let Some((data, metadata)) = self.receivers[pos].1.take_payload() {
                actions.push(Action::Resource {
                    link_id,
                    data,
                    metadata,
                });
            }
            self.receivers.swap_remove(pos);
        } else if self.receivers[pos].1.failure().is_some() {
            // An offer refused at its advertisement, or a body that failed to recover: its
            // cancel went out above, and nothing further is held for it.
            self.receivers.swap_remove(pos);
            self.refused_offers = self.refused_offers.saturating_add(1);
        } else if self.receivers[pos].1.is_canceled() {
            self.receivers.swap_remove(pos);
        }
    }

    /// The proof kept for `link_id`, if `matches` it and it has not yet been re-sent
    /// [`PROOF_CACHE_ANSWERS`] times. The cap bounds what anyone who heard the cleartext
    /// proof can make this node transmit by asking for it again.
    ///
    /// [`PROOF_CACHE_ANSWERS`]: crate::resource_transfer::PROOF_CACHE_ANSWERS
    pub(super) fn resend_kept_proof(
        &mut self,
        link_id: AddressHash,
        matches: impl Fn(&Packet) -> bool,
    ) -> Option<Packet> {
        let (_, proof, _, answers) = self
            .resource_proofs
            .iter_mut()
            .find(|(id, proof, _, _)| *id == link_id && matches(proof))?;
        if *answers >= crate::resource_transfer::PROOF_CACHE_ANSWERS {
            return None;
        }
        *answers += 1;
        Some(proof.clone())
    }
}
