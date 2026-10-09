//! Timers and this node's own announce.

use super::tables::derived_iv;
use super::{
    Action, Actions, AppDataTooLarge, InterfaceId, MIN_LOGICAL_MTU, Node, RESOURCE_PROOF_CACHE_TTL,
    RESOURCE_RETRY_INTERVAL,
};
use crate::announce::{self, AnnounceBlob, RATCHET_LEN};
use crate::packet::Packet;

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Whether the node should attempt its own announce at `now`.
    ///
    /// This predicate is separate from blob availability. A shell may be due to announce
    /// while it is still waiting for a reservation-backed blob; in that case [`Self::poll`]
    /// runs maintenance and leaves this predicate true for the next poll.
    pub fn announce_due(&self, now: u64) -> bool {
        match self.last_announce {
            None => true,
            Some(last) => now.saturating_sub(last) >= self.announce_interval,
        }
    }

    /// Advance the node's own timers.
    ///
    /// The shell supplies an optional typed announce blob. Clock acquisition, durable
    /// reservation, and nonce policy stay outside this executor-neutral layer. If an announce
    /// is due but no blob is available, the announce is skipped and remains due on the next
    /// poll; maintenance still runs.
    pub fn poll(
        &mut self,
        now: u64,
        interface: InterfaceId,
        blob: Option<&AnnounceBlob>,
    ) -> Actions<ACTIONS> {
        let mut actions = Actions::new();

        self.expire_transport_state(now);

        // Ordinary expiry also releases resource buffers and tells the caller.
        // A resident caller can call expire_sessions first to retain its full
        // resource-loss report, then poll without emitting duplicate reports.
        let expired = self.expire_sessions(now);
        for link_id in expired.links {
            actions.push(Action::LinkDown { link_id });
        }
        for link_id in expired.pending_links {
            actions.push(Action::LinkRequestTimedOut { link_id });
        }
        self.poll_liveness(now, interface, &mut actions);

        if self.announce_due(now)
            && let Some(blob) = blob
        {
            self.last_announce = Some(now);
            match self.try_announce(blob, None) {
                Ok(packet) => {
                    self.announced_blob = Some(*blob);
                    actions.push(Action::Send { interface, packet });
                }
                Err(_) => {
                    self.refused_payloads = self.refused_payloads.saturating_add(1);
                }
            }
        }

        // Loss recovery: a transfer silent for a retry interval is redriven. A receiver
        // re-requests what it is missing, a sender re-offers an unanswered advertisement.
        let seed = self.identity.to_secret_bytes();
        let mut counter = self.iv_counter;
        for index in 0..self.receivers.len() {
            if now.saturating_sub(self.receivers[index].2) < RESOURCE_RETRY_INTERVAL {
                continue;
            }
            let link_id = self.receivers[index].0;
            let mut iv = || derived_iv(&seed, link_id, &mut counter);
            let replies = self.receivers[index].1.retransmit(&mut iv);
            self.receivers[index].2 = now;
            for reply in replies {
                actions.push(Action::Send {
                    interface,
                    packet: reply,
                });
            }
        }
        let mut index = 0;
        while index < self.senders.len() {
            if now.saturating_sub(self.senders[index].2) < RESOURCE_RETRY_INTERVAL {
                index += 1;
                continue;
            }
            let link_id = self.senders[index].0;
            let mut iv = || derived_iv(&seed, link_id, &mut counter);
            self.senders[index].2 = now;
            let sender = &mut self.senders[index].1;
            // Every part sent and no proof: ask the receiver's cache for it, as RNS does,
            // and cancel once those requests are spent.
            let packet = if !sender.awaiting_proof() {
                sender.advertisement(&iv())
            } else if let Some(request) = sender.cache_request() {
                request
            } else {
                let cancel = sender.cancel(&iv());
                self.senders.swap_remove(index);
                if let Some(packet) = cancel {
                    actions.push(Action::Send { interface, packet });
                }
                continue;
            };
            actions.push(Action::Send { interface, packet });
            index += 1;
        }
        self.iv_counter = counter;
        self.resource_proofs.retain(|(id, _, at, _)| {
            now.saturating_sub(*at) < RESOURCE_PROOF_CACHE_TTL
                && self.links.iter().any(|(link, _, _)| link.id() == *id)
        });
        // Last, so relayed announces take only the room this node's own work left.
        self.poll_rebroadcasts(now, &mut actions);

        actions
    }

    /// Forget that we announced, so the next [`Node::poll`] announces again.
    ///
    /// `poll` stamps the announce when it decides to send one, since it cannot know whether
    /// the shell got it on air. A shell whose send failed calls this, and bounds how often, so
    /// a dead radio does not become an announce loop.
    pub fn retry_announce(&mut self) {
        self.last_announce = None;
    }

    /// Build an announce within the logical carrier budget, including optional ratchet bytes.
    pub fn try_announce(
        &self,
        blob: &AnnounceBlob,
        ratchet: Option<&[u8; RATCHET_LEN]>,
    ) -> Result<Packet, AppDataTooLarge> {
        let base = MIN_LOGICAL_MTU as usize + if ratchet.is_some() { RATCHET_LEN } else { 0 };
        if base + self.app_data.len() > self.logical_mtu as usize {
            return Err(AppDataTooLarge);
        }
        Ok(self.announce(blob, ratchet))
    }

    /// Build this node's announce packet without enforcing the carrier budget.
    /// Use [`Self::try_announce`] for production egress; this builder also serves wire fixtures.
    pub fn announce(&self, blob: &AnnounceBlob, ratchet: Option<&[u8; RATCHET_LEN]>) -> Packet {
        announce::build(
            &self.identity,
            self.name_hash,
            blob,
            ratchet,
            &self.app_data,
        )
    }
}
