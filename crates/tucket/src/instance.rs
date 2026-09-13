//! Retained, clocked Tucket protocol state for a resident protocol runtime.
//!
//! This layer owns only Tucket's contacts, routes, dedup history and bounded
//! caller-requested text operations. It never owns a radio action queue: a
//! frame returned by [`Instance::next_retry`] may already be in the caller's
//! physical queue, and pausing or interrupting this instance does not revoke it.

use alloc::vec::Vec;

use crate::node::{CapacityError, Event, Node, PendingText, TextAttempt, TextRetryPolicy};

/// Stable identifier assigned to one caller-requested text operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct OperationId(pub u32);

/// Bounds selected for one retained Tucket instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InstanceConfig {
    /// Maximum caller-owned text operations retained by this instance.
    pub pending: usize,
    /// Minimum monotonic milliseconds between attempts.
    pub retry_after: u64,
}

/// Caller-selected wire timestamp and monotonic operation deadlines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendTiming {
    pub timestamp: u32,
    pub expires_at: u64,
    pub allowed_until: u64,
}

impl InstanceConfig {
    pub const fn new(pending: usize, retry_after: u64) -> Option<Self> {
        if pending == 0 {
            None
        } else {
            Some(Self {
                pending,
                retry_after,
            })
        }
    }
}

/// Why a time or bounded-operation request was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InstanceError {
    Capacity(CapacityError),
    PendingFull,
    UnknownOperation,
    Paused,
    AlreadyPaused,
    NotPaused,
    /// The caller must collect expiry accounting with [`Instance::advance`]
    /// before scheduling an operation at or beyond its expiry boundary.
    AdvanceRequired,
    TimeRegression,
    TimeOverflow,
    InvalidDeadline,
    ReturnBudgetExceeded,
    OperationIdExhausted,
    LossNotPermitted,
}

impl From<CapacityError> for InstanceError {
    fn from(value: CapacityError) -> Self {
        Self::Capacity(value)
    }
}

/// Whether a protocol can leave the radio until `return_by`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PauseAssessment {
    Ready,
    Busy { retry_at: u64 },
    RequiresLoss { pending: Vec<OperationId> },
}

/// Explicit permission required to discard local text obligations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LossPermission {
    Deny,
    Allow,
}

/// Complete bounded accounting of text operations lost or expired locally.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LossReport {
    pub operations: Vec<OperationId>,
}

/// Result of accepting one inbound frame.
#[derive(Debug, Clone)]
pub struct FrameOutcome {
    pub events: Vec<Event>,
    pub outbound: Vec<Vec<u8>>,
    pub acknowledged: Vec<OperationId>,
    pub expired: LossReport,
}

/// Read-only scheduling state for one retained caller operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OperationState {
    pub id: OperationId,
    pub retry_at: u64,
    pub expires_at: u64,
    pub allowed_until: u64,
    pub attempts_remaining: u8,
}

struct Operation {
    id: OperationId,
    pending: PendingText,
    retry_at: u64,
    expires_at: u64,
    allowed_until: u64,
}

/// A retained Tucket instance. Calls take caller-supplied monotonic milliseconds.
pub struct Instance {
    node: Node,
    config: InstanceConfig,
    operations: Vec<Operation>,
    next_id: u32,
    last_now: Option<u64>,
    paused: bool,
}

impl Instance {
    pub fn new(node: Node, config: InstanceConfig) -> Self {
        Self {
            node,
            config,
            operations: Vec::with_capacity(config.pending),
            next_id: 1,
            last_now: None,
            paused: false,
        }
    }

    pub fn node(&self) -> &Node {
        &self.node
    }
    /// Mutable protocol access only while this instance is active. It is for
    /// application composition such as adverts, never for physical queue work.
    pub fn active_node_mut(&mut self) -> Result<&mut Node, InstanceError> {
        if self.paused {
            Err(InstanceError::Paused)
        } else {
            Ok(&mut self.node)
        }
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn pending_len(&self) -> usize {
        self.operations.len()
    }

    pub fn operations(&self) -> impl Iterator<Item = OperationState> + '_ {
        self.operations.iter().map(|op| OperationState {
            id: op.id,
            retry_at: op.retry_at,
            expires_at: op.expires_at,
            allowed_until: op.allowed_until,
            attempts_remaining: op.pending.attempts_remaining(),
        })
    }

    /// Earliest retry or expiry the runtime should schedule. A final attempt
    /// awaiting its ACK contributes only its expiry deadline.
    pub fn next_deadline(&self) -> Option<u64> {
        self.operations
            .iter()
            .flat_map(|op| {
                let retry = (op.pending.attempts_remaining() > 0).then_some(op.retry_at);
                [retry, Some(op.expires_at)].into_iter().flatten()
            })
            .min()
    }

    fn validate_now(&self, now: u64) -> Result<(), InstanceError> {
        if self.last_now.is_some_and(|last| now < last) {
            Err(InstanceError::TimeRegression)
        } else {
            Ok(())
        }
    }

    fn accept_now(&mut self, now: u64) {
        self.last_now = Some(now);
    }

    /// Create a bounded text operation. `expires_at` is the total lifetime;
    /// `allowed_until` is the caller's latest permitted transmission time.
    pub fn begin_send(
        &mut self,
        now: u64,
        to: u8,
        text: impl AsRef<str>,
        policy: TextRetryPolicy,
        timing: SendTiming,
    ) -> Result<OperationId, InstanceError> {
        self.validate_now(now)?;
        if self.paused {
            return Err(InstanceError::Paused);
        }
        if timing.expires_at <= now
            || timing.allowed_until < now
            || timing.allowed_until > timing.expires_at
        {
            return Err(InstanceError::InvalidDeadline);
        }
        if self.operations.len() == self.config.pending {
            return Err(InstanceError::PendingFull);
        }
        let id = OperationId(self.next_id);
        let next_id = self
            .next_id
            .checked_add(1)
            .ok_or(InstanceError::OperationIdExhausted)?;
        let pending = self
            .node
            .try_begin_text(to, timing.timestamp, text, policy)?;
        self.operations.push(Operation {
            id,
            pending,
            retry_at: now,
            expires_at: timing.expires_at,
            allowed_until: timing.allowed_until,
        });
        self.next_id = next_id;
        self.accept_now(now);
        Ok(id)
    }

    /// Produce the due attempt for `id`. This only issues a frame; radio queue
    /// ownership and TX completion remain with the caller.
    pub fn next_retry(
        &mut self,
        now: u64,
        id: OperationId,
        return_by: u64,
    ) -> Result<Option<TextAttempt>, InstanceError> {
        self.validate_now(now)?;
        if self.paused {
            return Err(InstanceError::Paused);
        }
        if return_by < now {
            return Err(InstanceError::InvalidDeadline);
        }
        let pos = self
            .operations
            .iter()
            .position(|op| op.id == id)
            .ok_or(InstanceError::UnknownOperation)?;
        let op = &self.operations[pos];
        if now >= op.expires_at {
            return Err(InstanceError::AdvanceRequired);
        }
        if return_by > op.allowed_until || return_by >= op.expires_at {
            return Err(InstanceError::ReturnBudgetExceeded);
        }
        if now > op.allowed_until {
            return Err(InstanceError::ReturnBudgetExceeded);
        }
        if now < op.retry_at {
            self.accept_now(now);
            return Ok(None);
        }
        let next_retry = now
            .checked_add(self.config.retry_after)
            .ok_or(InstanceError::TimeOverflow)?;
        let attempt = self
            .node
            .next_text_attempt(&mut self.operations[pos].pending);
        self.operations[pos].retry_at = next_retry;
        if self.operations[pos].pending.is_complete() {
            self.operations.swap_remove(pos);
        }
        self.accept_now(now);
        Ok(attempt)
    }

    /// Feed an RX frame while active and match any delayed ACK against all
    /// retained operations. A paused instance refuses RX ownership.
    pub fn on_frame(&mut self, now: u64, frame: &[u8]) -> Result<FrameOutcome, InstanceError> {
        self.validate_now(now)?;
        if self.paused {
            return Err(InstanceError::Paused);
        }
        let expired = self.advance(now)?;
        let (events, outbound) = self.node.on_frame(frame);
        let mut acknowledged = Vec::with_capacity(self.operations.len());
        for event in &events {
            if let Event::Ack(ack) = event {
                let mut i = 0;
                while i < self.operations.len() {
                    if self.operations[i].pending.acknowledge(*ack) {
                        acknowledged.push(self.operations[i].id);
                        self.operations.swap_remove(i);
                    } else {
                        i += 1;
                    }
                }
            }
        }
        self.accept_now(now);
        Ok(FrameOutcome {
            events,
            outbound,
            acknowledged,
            expired,
        })
    }

    /// Assess departure without mutating state. Operations that would require
    /// an attempt before return are busy; ones whose caller deadline would pass
    /// require explicit loss.
    pub fn assess_pause(&self, now: u64, return_by: u64) -> Result<PauseAssessment, InstanceError> {
        self.validate_now(now)?;
        if return_by < now {
            return Err(InstanceError::InvalidDeadline);
        }
        let mut loss = Vec::new();
        let mut busy = None;
        for op in &self.operations {
            if op.expires_at <= return_by {
                loss.push(op.id);
            } else if op.pending.attempts_remaining() > 0 {
                if op.allowed_until < return_by {
                    loss.push(op.id);
                } else if op.retry_at <= return_by {
                    busy = Some(busy.map_or(op.retry_at, |time: u64| time.min(op.retry_at)));
                }
            }
        }
        if !loss.is_empty() {
            Ok(PauseAssessment::RequiresLoss { pending: loss })
        } else if let Some(retry_at) = busy {
            Ok(PauseAssessment::Busy { retry_at })
        } else {
            Ok(PauseAssessment::Ready)
        }
    }

    /// Pause after a ready assessment. Contacts, routes and dedup stay resident.
    pub fn pause(&mut self, now: u64, return_by: u64) -> Result<(), InstanceError> {
        self.validate_now(now)?;
        if self.paused {
            return Err(InstanceError::AlreadyPaused);
        }
        if return_by < now {
            return Err(InstanceError::InvalidDeadline);
        }
        match self.assess_pause(now, return_by)? {
            PauseAssessment::Ready => {
                self.paused = true;
                self.accept_now(now);
                Ok(())
            }
            PauseAssessment::Busy { .. } | PauseAssessment::RequiresLoss { .. } => {
                Err(InstanceError::ReturnBudgetExceeded)
            }
        }
    }

    /// Resume without emitting reconnect traffic. Elapsed time is applied by
    /// dropping expired local obligations and reporting every operation lost.
    pub fn resume(&mut self, now: u64) -> Result<LossReport, InstanceError> {
        if !self.paused {
            return Err(InstanceError::NotPaused);
        }
        let report = self.advance(now)?;
        self.paused = false;
        Ok(report)
    }

    /// Apply elapsed monotonic time in either active or paused state. Expiry is
    /// explicit so the caller receives every locally lost operation exactly
    /// once before it can accept a late ACK.
    pub fn advance(&mut self, now: u64) -> Result<LossReport, InstanceError> {
        self.validate_now(now)?;
        let mut report = LossReport {
            operations: Vec::with_capacity(self.operations.len()),
        };
        let mut i = 0;
        while i < self.operations.len() {
            if now >= self.operations[i].expires_at {
                report.operations.push(self.operations[i].id);
                self.operations.swap_remove(i);
            } else {
                i += 1;
            }
        }
        self.accept_now(now);
        Ok(report)
    }

    /// Explicitly discard every retained operation. Denial is pure and leaves
    /// contacts, routes, dedup, time and operation state unchanged.
    pub fn interrupt(&mut self, permission: LossPermission) -> Result<LossReport, InstanceError> {
        if permission != LossPermission::Allow {
            return Err(InstanceError::LossNotPermitted);
        }
        let mut report = LossReport {
            operations: Vec::with_capacity(self.operations.len()),
        };
        for operation in self.operations.drain(..) {
            report.operations.push(operation.id);
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::LocalIdentity;

    fn instance(seed: u8) -> Instance {
        Instance::new(
            Node::new(LocalIdentity::from_seed([seed; 32]), false),
            InstanceConfig::new(2, 10).unwrap(),
        )
    }

    fn introduce(a: &mut Instance, b: &mut Instance) {
        let a_adv = a.active_node_mut().unwrap().advert_frame(1, b"a");
        let b_adv = b.active_node_mut().unwrap().advert_frame(2, b"b");
        b.on_frame(0, &a_adv).unwrap();
        a.on_frame(0, &b_adv).unwrap();
    }

    #[test]
    fn two_nodes_ack_a_retained_send() {
        let mut a = instance(1);
        let mut b = instance(2);
        introduce(&mut a, &mut b);
        let id = a
            .begin_send(
                1,
                b.node().my_hash(),
                "hello",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: 100,
                    allowed_until: 100,
                },
            )
            .unwrap();
        let attempt = a.next_retry(1, id, 2).unwrap().unwrap();
        let received = b.on_frame(1, &attempt.frame).unwrap();
        let ack = match received.events.as_slice() {
            [Event::Message { ack, .. }] => *ack,
            _ => panic!("text"),
        };
        let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
        assert_eq!(a.on_frame(2, &ack_frame).unwrap().acknowledged, [id]);
        assert_eq!(a.pending_len(), 0);
    }

    #[test]
    fn delayed_ack_from_prior_attempt_matches() {
        let mut a = instance(3);
        let mut b = instance(4);
        introduce(&mut a, &mut b);
        let id = a
            .begin_send(
                1,
                b.node().my_hash(),
                "retry",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: 100,
                    allowed_until: 100,
                },
            )
            .unwrap();
        let first = a.next_retry(1, id, 2).unwrap().unwrap();
        let _second = a.next_retry(11, id, 12).unwrap().unwrap();
        let received = b.on_frame(12, &first.frame).unwrap();
        let ack = match received.events.as_slice() {
            [Event::Message { ack, .. }] => *ack,
            _ => panic!("text"),
        };
        let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
        assert_eq!(a.on_frame(13, &ack_frame).unwrap().acknowledged, [id]);
    }

    #[test]
    fn final_attempt_waits_for_ack_but_expiry_wins_at_its_exact_boundary() {
        let mut a = instance(12);
        let mut b = instance(13);
        introduce(&mut a, &mut b);
        let one_try = TextRetryPolicy::new(1, false).unwrap();
        let id = a
            .begin_send(
                1,
                b.node().my_hash(),
                "final",
                one_try,
                SendTiming {
                    timestamp: 3,
                    expires_at: 50,
                    allowed_until: 49,
                },
            )
            .unwrap();
        let sent = a.next_retry(1, id, 2).unwrap().unwrap();
        let received = b.on_frame(2, &sent.frame).unwrap();
        let ack = match received.events.as_slice() {
            [Event::Message { ack, .. }] => *ack,
            _ => panic!("text"),
        };
        assert!(a.next_retry(11, id, 12).unwrap().is_none());
        assert_eq!(a.operations().collect::<Vec<_>>()[0].id, id);
        assert_eq!(a.next_deadline(), Some(50));
        assert_eq!(a.assess_pause(12, 49), Ok(PauseAssessment::Ready));
        let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
        assert_eq!(a.on_frame(12, &ack_frame).unwrap().acknowledged, [id]);

        let expired = a
            .begin_send(
                20,
                b.node().my_hash(),
                "late",
                one_try,
                SendTiming {
                    timestamp: 4,
                    expires_at: 30,
                    allowed_until: 29,
                },
            )
            .unwrap();
        let sent = a.next_retry(20, expired, 21).unwrap().unwrap();
        let received = b.on_frame(21, &sent.frame).unwrap();
        let ack = match received.events.as_slice() {
            [Event::Message { ack, .. }] => *ack,
            _ => panic!("text"),
        };
        let ack_frame = b.active_node_mut().unwrap().ack_frame(ack);
        let outcome = a.on_frame(30, &ack_frame).unwrap();
        assert_eq!(outcome.expired.operations, [expired]);
        assert!(outcome.acknowledged.is_empty());
    }

    #[test]
    fn pause_requires_loss_then_resume_expires_without_sending() {
        let mut a = instance(5);
        let mut b = instance(6);
        introduce(&mut a, &mut b);
        let id = a
            .begin_send(
                1,
                b.node().my_hash(),
                "hold",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: 20,
                    allowed_until: 20,
                },
            )
            .unwrap();
        assert!(
            matches!(a.assess_pause(2, 21), Ok(PauseAssessment::RequiresLoss { pending }) if pending == [id])
        );
        assert_eq!(a.pause(2, 21), Err(InstanceError::ReturnBudgetExceeded));
        let _ = a.next_retry(1, id, 2).unwrap().unwrap();
        a.pause(2, 3).unwrap();
        assert!(matches!(a.on_frame(3, b"bad"), Err(InstanceError::Paused)));
        assert_eq!(a.resume(21).unwrap().operations, [id]);
    }

    #[test]
    fn denial_is_pure_and_time_regression_refuses() {
        let mut a = instance(7);
        let mut b = instance(8);
        introduce(&mut a, &mut b);
        let id = a
            .begin_send(
                1,
                b.node().my_hash(),
                "hold",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: 100,
                    allowed_until: 100,
                },
            )
            .unwrap();
        assert_eq!(
            a.interrupt(LossPermission::Deny),
            Err(InstanceError::LossNotPermitted)
        );
        assert_eq!(a.pending_len(), 1);
        assert_eq!(a.next_retry(0, id, 1), Err(InstanceError::TimeRegression));
        assert_eq!(a.interrupt(LossPermission::Allow).unwrap().operations, [id]);
    }

    #[test]
    fn pending_capacity_invalid_return_and_retry_overflow_refuse_before_mutation() {
        let mut a = Instance::new(
            Node::new(LocalIdentity::from_seed([9; 32]), false),
            InstanceConfig::new(1, 10).unwrap(),
        );
        let mut b = instance(10);
        introduce(&mut a, &mut b);
        let id = a
            .begin_send(
                1,
                b.node().my_hash(),
                "one",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: 100,
                    allowed_until: 100,
                },
            )
            .unwrap();
        assert_eq!(a.assess_pause(2, 1), Err(InstanceError::InvalidDeadline));
        assert_eq!(
            a.begin_send(
                2,
                b.node().my_hash(),
                "two",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: 100,
                    allowed_until: 100
                },
            ),
            Err(InstanceError::PendingFull)
        );
        assert_eq!(a.pending_len(), 1);

        let mut overflow = Instance::new(
            Node::new(LocalIdentity::from_seed([11; 32]), false),
            InstanceConfig::new(1, 2).unwrap(),
        );
        overflow
            .active_node_mut()
            .unwrap()
            .try_add_contact(b.node().identity().clone())
            .unwrap();
        let overflow_id = overflow
            .begin_send(
                u64::MAX - 1,
                b.node().my_hash(),
                "x",
                TextRetryPolicy::default(),
                SendTiming {
                    timestamp: 3,
                    expires_at: u64::MAX,
                    allowed_until: u64::MAX - 1,
                },
            )
            .unwrap();
        assert_eq!(
            overflow.next_retry(u64::MAX - 1, overflow_id, u64::MAX - 1),
            Err(InstanceError::TimeOverflow)
        );
        assert_eq!(overflow.pending_len(), 1);
        assert_eq!(id, OperationId(1));
    }
}
