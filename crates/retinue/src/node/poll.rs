//! Timers and this node's own announce.

use super::tables::derived_iv;
use super::{
    Action, Actions, AppDataTooLarge, InterfaceId, LINK_IDLE_TIMEOUT, MIN_LOGICAL_MTU, Node,
    RESOURCE_PROOF_CACHE_TTL,
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

        // Loss recovery: each transfer's watchdog redrives it once overdue. A receiver
        // re-requests what it is missing, a sender re-offers an unanswered advertisement or
        // asks for a lost proof; either gives up, with a cancel, once its retries run out.
        let seed = self.identity.to_secret_bytes();
        let mut counter = self.iv_counter;
        let mut index = 0;
        while index < self.receivers.len() {
            let link_id = self.receivers[index].0;
            let mut iv = || derived_iv(&seed, link_id, &mut counter);
            let receiver = &mut self.receivers[index].1;
            for packet in receiver.poll(now, &mut iv) {
                actions.push(Action::Send { interface, packet });
            }
            if receiver.failure().is_some() {
                if let Some(carry) = receiver.carry()
                    && let Some((link, _, _)) =
                        self.links.iter_mut().find(|(l, _, _)| l.id() == link_id)
                {
                    link.set_resource_carry(carry);
                }
                self.receivers.swap_remove(index);
                continue;
            }
            index += 1;
        }
        let mut index = 0;
        while index < self.senders.len() {
            let link_id = self.senders[index].0;
            let mut iv = || derived_iv(&seed, link_id, &mut counter);
            let sender = &mut self.senders[index].1;
            if let Some(packet) = sender.poll(now, &mut iv) {
                actions.push(Action::Send { interface, packet });
            }
            if sender.is_canceled() {
                self.senders.swap_remove(index);
                continue;
            }
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

    /// The earliest tick at which [`Self::poll`] has timed work besides this node's own
    /// announce: a relayed announce ([`Self::next_rebroadcast`]), a resource watchdog
    /// ([`Self::resource_deadline`]), a link's keepalive, staleness, teardown or idle expiry,
    /// or an unanswered link request's deadline. A shell that polls on a coarse beat wakes
    /// for it; polling at it always moves it on. `None` while nothing is timed.
    pub fn next_deadline(&self) -> Option<u64> {
        let links = self.links.iter().flat_map(|(_, _, liveness)| {
            [
                liveness.next_due(),
                liveness.last_inbound().saturating_add(LINK_IDLE_TIMEOUT),
            ]
        });
        let requests = self.pending.iter().map(|(_, deadline, _)| *deadline);
        links
            .chain(requests)
            .chain(self.next_rebroadcast())
            .chain(self.resource_deadline())
            .min()
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
