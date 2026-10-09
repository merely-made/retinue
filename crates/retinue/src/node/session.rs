//! Session expiry, pause assessment and forced interruption.

use heapless::Vec as BoundedVec;

use super::{
    InterruptionPermission, InterruptionReport, LINK_IDLE_TIMEOUT, Node, PauseAssessment,
    SessionExpiryReport, SessionLossNotPermitted,
};
use crate::hash::AddressHash;

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// Explicitly end all local sessions and transfer obligations.
    ///
    /// Before calling, the owner must admit session loss, finish any in-flight
    /// hardware operation, and cancel or account for every previously returned
    /// action. Such actions are caller-owned and cannot be revoked here; replaying
    /// them after interruption can create new remote work. Stop ingesting during
    /// the switch. New incoming requests after return can establish new sessions.
    ///
    /// Returns every affected ID and encrypted close packet without squeezing
    /// loss notifications into `ACTIONS`. Generate a fresh IV for each link using
    /// the caller's normal entropy source. Denied permission never calls it.
    /// Identity, peers, route/freshness history and the resource IV counter are
    /// retained. This operation neither freezes time nor reports a radio change.
    pub fn force_interrupt(
        &mut self,
        permission: InterruptionPermission,
        mut iv: impl FnMut() -> [u8; crate::token::IV_LEN],
    ) -> Result<InterruptionReport<LINKS, ROUTES>, SessionLossNotPermitted> {
        if permission != InterruptionPermission::AllowSessionLoss {
            return Err(SessionLossNotPermitted);
        }
        let mut report = InterruptionReport {
            closed_links: BoundedVec::new(),
            close_packets: BoundedVec::new(),
            pending_links: BoundedVec::new(),
            inbound_resources: BoundedVec::new(),
            outbound_resources: BoundedVec::new(),
            transit_bridges: BoundedVec::new(),
        };
        // Each output has exactly the capacity of its source table. Prepare the
        // complete report before mutation, so no ordinary Actions overflow can
        // hide a discarded session or transfer.
        for (link, _, _) in &self.links {
            report.closed_links.push(link.id()).expect("link bound");
            report
                .close_packets
                .push(link.close_packet(&iv()))
                .expect("link bound");
        }
        for (pending, _, _) in &self.pending {
            report
                .pending_links
                .push(pending.link_id())
                .expect("pending bound");
        }
        for (id, _, _) in &self.receivers {
            report.inbound_resources.push(*id).expect("receiver bound");
        }
        for (id, _, _) in &self.senders {
            report.outbound_resources.push(*id).expect("sender bound");
        }
        for bridge in &self.bridges {
            report
                .transit_bridges
                .push(bridge.link_id)
                .expect("bridge bound");
        }
        self.links.clear();
        self.pending.clear();
        self.receivers.clear();
        self.senders.clear();
        self.bridges.clear();
        Ok(report)
    }

    /// Inspect local work before asking a radio owner to pause this node.
    ///
    /// This neither polls nor expires state. In particular, established links
    /// retain their original `last_seen` timestamps while the caller is away;
    /// call [`PauseAssessment::can_pause_through`] with the caller's monotonic
    /// `now` and proposed return bound before admission, then call [`Self::poll`]
    /// normally after return. Remote peers may still expire independently.
    pub fn pause_assessment(&self) -> PauseAssessment {
        let mut earliest_link_expiry: Option<u64> = None;
        let mut latest_link_activity: Option<u64> = None;
        let mut link_expiry_overflow = false;
        for (_, _, liveness) in &self.links {
            let last_seen = liveness.last_inbound();
            latest_link_activity = Some(match latest_link_activity {
                Some(current) => current.max(last_seen),
                None => last_seen,
            });
            match last_seen.checked_add(LINK_IDLE_TIMEOUT) {
                Some(idle) => {
                    // A stale link's teardown, or a responder's handshake deadline, can
                    // come before the idle expiry.
                    let expiry = liveness.teardown_at().map_or(idle, |at| at.min(idle));
                    earliest_link_expiry = Some(match earliest_link_expiry {
                        Some(current) => current.min(expiry),
                        None => expiry,
                    });
                }
                None => link_expiry_overflow = true,
            }
        }
        PauseAssessment {
            pending_handshakes: self.pending.len(),
            inbound_resources: self.receivers.len(),
            outbound_resources: self.senders.len(),
            transit_bridges: self.bridges.len(),
            earliest_link_expiry,
            latest_link_activity,
            link_expiry_overflow,
        }
    }

    /// Reconcile session expiry after an absence without generating radio work.
    /// Associated resource state is removed with its link, including orphaned
    /// entries left by earlier maintenance. A link request unanswered at its
    /// deadline is dropped and reported in `pending_links`.
    pub fn expire_sessions(&mut self, now: u64) -> SessionExpiryReport<LINKS> {
        let mut report = SessionExpiryReport {
            pending_links: self.expire_link_requests(now),
            ..Default::default()
        };
        self.links.retain(|(link, _, liveness)| {
            let expired = now.saturating_sub(liveness.last_inbound()) >= LINK_IDLE_TIMEOUT;
            if expired {
                let _ = report.links.push(link.id());
            }
            !expired
        });
        self.expired_links = self.expired_links.saturating_add(report.links.len() as u16);
        self.receivers.retain(|(id, _, _)| {
            let keep = self.links.iter().any(|(link, _, _)| link.id() == *id);
            if !keep {
                let _ = report.inbound_resources.push(*id);
            }
            keep
        });
        self.senders.retain(|(id, _, _)| {
            let keep = self.links.iter().any(|(link, _, _)| link.id() == *id);
            if !keep {
                let _ = report.outbound_resources.push(*id);
            }
            keep
        });
        report
    }

    /// Drop link requests unanswered at their deadline, returning their link ids.
    pub(super) fn expire_link_requests(&mut self, now: u64) -> BoundedVec<AddressHash, LINKS> {
        let mut expired_ids = BoundedVec::new();
        self.pending.retain(|(attempt, deadline, _)| {
            let expired = now >= *deadline;
            if expired {
                let _ = expired_ids.push(attempt.link_id());
            }
            !expired
        });
        self.expired_link_requests = self
            .expired_link_requests
            .saturating_add(expired_ids.len() as u16);
        expired_ids
    }
}
