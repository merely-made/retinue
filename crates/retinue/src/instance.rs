//! Retained Node lifecycle. Time advances during absence; physical work stays
//! with the caller and must be fenced separately before suspension.
use crate::{
    announce::AnnounceBlob,
    hash::AddressHash,
    node::{
        Actions, InterruptionPermission, InterruptionReport, Node, PauseAssessment, PauseBlocked,
        SessionExpiryReport,
    },
    packet::Packet,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceError {
    Paused,
    AlreadyPaused,
    NotPaused,
    ClockRegression,
    PauseBlocked(PauseBlocked),
}

pub struct Instance<const P: usize = 8, const A: usize = 4, const L: usize = 1, const R: usize = 4>
{
    node: Node<P, A, L, R>,
    last_now: u64,
    paused: bool,
}
impl<const P: usize, const A: usize, const L: usize, const R: usize> Instance<P, A, L, R> {
    pub fn new(node: Node<P, A, L, R>, now: u64) -> Self {
        Self {
            node,
            last_now: now,
            paused: false,
        }
    }
    pub fn node(&self) -> &Node<P, A, L, R> {
        &self.node
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    fn time(&self, now: u64) -> Result<(), InstanceError> {
        if now < self.last_now {
            Err(InstanceError::ClockRegression)
        } else {
            Ok(())
        }
    }
    fn active(&self, now: u64) -> Result<(), InstanceError> {
        self.time(now)?;
        if self.paused {
            Err(InstanceError::Paused)
        } else {
            Ok(())
        }
    }
    pub fn assess_pause(&self, now: u64, return_by: u64) -> Result<PauseAssessment, InstanceError> {
        self.active(now)?;
        let assessment = self.node.pause_assessment();
        assessment
            .can_pause_through(now, return_by)
            .map_err(InstanceError::PauseBlocked)?;
        Ok(assessment)
    }
    /// Explicitly ending sessions creates close packets in the loss report;
    /// caller must transmit or account for them before changing the radio.
    pub fn pause(
        &mut self,
        now: u64,
        return_by: u64,
        permission: InterruptionPermission,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<Option<InterruptionReport<L, R>>, InstanceError> {
        self.time(now)?;
        if self.paused {
            return Err(InstanceError::AlreadyPaused);
        }
        if return_by < now {
            return Err(InstanceError::PauseBlocked(
                PauseBlocked::ReturnBoundBeforeNow { now, return_by },
            ));
        }
        let loss = match self
            .node
            .pause_assessment()
            .can_pause_through(now, return_by)
        {
            Ok(()) => None,
            Err(reason) => {
                // These two failures say that our clock cannot safely interpret
                // retained link state. Explicit session loss cannot turn an
                // untrustworthy clock into permission to mutate it.
                if matches!(
                    reason,
                    PauseBlocked::ClockBeforeLinkActivity { .. } | PauseBlocked::LinkExpiryOverflow
                ) {
                    return Err(InstanceError::PauseBlocked(reason));
                }
                if permission != InterruptionPermission::AllowSessionLoss {
                    return Err(InstanceError::PauseBlocked(reason));
                }
                Some(
                    self.node
                        .force_interrupt(permission, iv)
                        .expect("explicit permission"),
                )
            }
        };
        self.paused = true;
        self.last_now = now;
        Ok(loss)
    }
    pub fn advance(&mut self, now: u64) -> Result<SessionExpiryReport<L>, InstanceError> {
        self.time(now)?;
        let report = self.node.expire_sessions(now);
        self.last_now = now;
        Ok(report)
    }
    pub fn resume(&mut self, now: u64) -> Result<SessionExpiryReport<L>, InstanceError> {
        self.time(now)?;
        if !self.paused {
            return Err(InstanceError::NotPaused);
        }
        let report = self.node.expire_sessions(now);
        self.last_now = now;
        self.paused = false;
        Ok(report)
    }
    pub fn ingest(&mut self, now: u64, packet: &Packet) -> Result<Actions<A>, InstanceError> {
        self.active(now)?;
        let result = self.node.ingest(0, packet, now);
        self.last_now = now;
        Ok(result)
    }
    pub fn poll(
        &mut self,
        now: u64,
        blob: Option<&AnnounceBlob>,
    ) -> Result<Actions<A>, InstanceError> {
        self.active(now)?;
        let result = self.node.poll(now, 0, blob);
        self.last_now = now;
        Ok(result)
    }
    pub fn open_link(
        &mut self,
        now: u64,
        to: AddressHash,
        seed: &[u8; 64],
    ) -> Result<Option<Actions<A>>, InstanceError> {
        self.active(now)?;
        let result = self.node.open_link(to, 0, seed);
        self.last_now = now;
        Ok(result)
    }
    pub fn send(
        &mut self,
        now: u64,
        id: AddressHash,
        data: &[u8],
        iv: &[u8; 16],
    ) -> Result<Option<Actions<A>>, InstanceError> {
        self.active(now)?;
        let result = self.node.send(id, 0, data, iv);
        self.last_now = now;
        Ok(result)
    }
    pub fn publish(
        &mut self,
        now: u64,
        id: AddressHash,
        data: &[u8],
        salt: [u8; 4],
        iv: &[u8; 16],
    ) -> Result<Option<Actions<A>>, InstanceError> {
        self.active(now)?;
        let result = self.node.publish(id, 0, data, salt, iv, now);
        self.last_now = now;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{announce::RAND_HASH_LEN, destination::DestinationName, identity::PrivateIdentity};

    fn sent<const A: usize>(actions: &Actions<A>) -> Packet {
        actions
            .iter()
            .find_map(|action| match action {
                crate::node::Action::Send { packet, .. } => Some(packet.clone()),
                _ => None,
            })
            .expect("link setup emits a packet")
    }

    fn linked_at(now: u64) -> (Node<32, 8, 4>, AddressHash) {
        let mut a: Node<32, 8, 4> = Node::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["instance-a"]).name_hash(),
        );
        let mut b: Node<32, 8, 4> = Node::new(
            PrivateIdentity::from_secret_bytes(&[0x22; 64]),
            DestinationName::new("retinue", ["instance-b"]).name_hash(),
        );
        a.ingest(
            0,
            &b.announce(&AnnounceBlob::from_wire([2; RAND_HASH_LEN]), None),
            now,
        );
        let request = sent(&a.open_link(b.destination(), 0, &[0x31; 64]).unwrap());
        let proof = sent(&b.ingest(0, &request, now));
        let actions = a.ingest(0, &proof, now);
        let id = actions
            .iter()
            .find_map(|action| match action {
                crate::node::Action::LinkUp { link_id } => Some(*link_id),
                _ => None,
            })
            .expect("link comes up");
        (a, id)
    }

    #[test]
    fn pause_retains_live_links_and_resume_does_not_reset_their_clock() {
        let (node, id) = linked_at(100);
        let mut instance = Instance::new(node, 100);
        let return_by = 100 + crate::node::LINK_IDLE_TIMEOUT - 1;

        assert!(matches!(
            instance.pause(
                100,
                return_by,
                InterruptionPermission::PreserveSessions,
                || [0; 16]
            ),
            Ok(None)
        ));
        assert!(instance.is_paused());
        assert_eq!(instance.node().link_count(), 1);
        assert!(matches!(
            instance.poll(101, None),
            Err(InstanceError::Paused)
        ));
        assert!(instance.advance(return_by).unwrap().links.is_empty());
        assert!(instance.node().has_link(id));
        assert!(instance.resume(return_by).unwrap().links.is_empty());
        assert!(!instance.is_paused());
        assert_eq!(
            instance.node().pause_assessment().earliest_link_expiry,
            Some(100 + crate::node::LINK_IDLE_TIMEOUT)
        );
    }

    #[test]
    fn invalid_lifecycle_requests_do_not_advance_the_clock() {
        let node: Node<32, 8, 4> = Node::new(
            PrivateIdentity::from_secret_bytes(&[0x33; 64]),
            DestinationName::new("retinue", ["lifecycle"]).name_hash(),
        );
        let mut instance = Instance::new(node, 0);
        assert!(matches!(instance.resume(10), Err(InstanceError::NotPaused)));
        assert!(instance.advance(0).is_ok());
        assert!(matches!(
            instance.pause(0, 0, InterruptionPermission::PreserveSessions, || [0; 16]),
            Ok(None)
        ));
        assert!(matches!(
            instance.pause(10, 10, InterruptionPermission::PreserveSessions, || [0; 16]),
            Err(InstanceError::AlreadyPaused)
        ));
        assert!(instance.resume(0).is_ok());
    }

    #[test]
    fn forced_loss_cannot_bypass_an_untrusted_link_clock_or_expiry_overflow() {
        let (node, id) = linked_at(1_000);
        let mut clock_before_activity = Instance::new(node, 0);
        assert!(matches!(
            clock_before_activity.pause(
                0,
                100,
                InterruptionPermission::AllowSessionLoss,
                || panic!("must not force loss")
            ),
            Err(InstanceError::PauseBlocked(
                PauseBlocked::ClockBeforeLinkActivity {
                    now: 0,
                    latest_activity: 1_000
                }
            ))
        ));
        assert!(clock_before_activity.node().has_link(id));
        assert!(!clock_before_activity.is_paused());

        let (node, id) = linked_at(u64::MAX);
        let mut overflow = Instance::new(node, u64::MAX);
        assert!(matches!(
            overflow.pause(
                u64::MAX,
                u64::MAX,
                InterruptionPermission::AllowSessionLoss,
                || panic!("must not force loss")
            ),
            Err(InstanceError::PauseBlocked(
                PauseBlocked::LinkExpiryOverflow
            ))
        ));
        assert!(overflow.node().has_link(id));
        assert!(!overflow.is_paused());
    }
}
