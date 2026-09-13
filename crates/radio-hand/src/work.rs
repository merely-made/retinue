//! Bounded physical work owned by one confirmed protocol activation.
//!
//! Protocol state lives elsewhere. This queue fences issued actions across
//! switches and refuses departure while a physical operation is in flight.
use heapless::{Deque, Vec};
use selvage::personality::PersonalityId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Activation {
    pub instance: PersonalityId,
    pub generation: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkId {
    pub activation: Activation,
    pub sequence: u64,
    /// Protocol-owned operation, if this transmission belongs to one.
    pub operation: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transmission {
    pub id: WorkId,
    pub deadline: u64,
    pub frame: Vec<u8, { selvage::MAX_RADIO_FRAME_LEN }>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkError {
    Inactive,
    StaleActivation,
    WrongCompletion,
    Full,
    FrameTooLong,
    Expired,
    Busy,
    LossNotPermitted,
    ClockRegression,
    Exhausted,
}

pub struct WorkQueue<const N: usize> {
    active: Option<Activation>,
    next_generation: u64,
    next_sequence: u64,
    last_now: u64,
    queued: Deque<Transmission, N>,
    inflight: Option<WorkId>,
}

impl<const N: usize> WorkQueue<N> {
    pub fn new(now: u64) -> Self {
        Self {
            active: None,
            next_generation: 1,
            next_sequence: 1,
            last_now: now,
            queued: Deque::new(),
            inflight: None,
        }
    }
    pub fn activation(&self) -> Option<Activation> {
        self.active
    }
    pub fn len(&self) -> usize {
        self.queued.len()
    }
    pub fn is_empty(&self) -> bool {
        self.queued.is_empty()
    }
    pub fn available(&self) -> usize {
        N - self.queued.len()
    }
    pub fn inflight(&self) -> Option<WorkId> {
        self.inflight
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.queued.iter().map(|w| w.deadline).min()
    }

    fn time(&self, now: u64) -> Result<(), WorkError> {
        if now < self.last_now {
            Err(WorkError::ClockRegression)
        } else {
            Ok(())
        }
    }

    pub fn contains_operation(&self, operation: u64) -> bool {
        self.queued
            .iter()
            .any(|w| w.id.operation == Some(operation))
    }
    pub fn first_deadline(&self) -> Option<u64> {
        self.queued.front().map(|w| w.deadline)
    }
    /// ACK/expiry may cancel a queued retry, never physical in-flight custody.
    pub fn cancel_operation(
        &mut self,
        now: u64,
        operation: u64,
    ) -> Result<Vec<WorkId, N>, WorkError> {
        self.time(now)?;
        let mut cancelled = Vec::new();
        for _ in 0..self.queued.len() {
            let work = self.queued.pop_front().expect("original count");
            if work.id.operation == Some(operation) && self.inflight != Some(work.id) {
                let _ = cancelled.push(work.id);
            } else {
                let _ = self.queued.push_back(work);
            }
        }
        self.last_now = now;
        Ok(cancelled)
    }

    /// Call only after the radio owner confirms this instance's profile and RX.
    pub fn activate(&mut self, now: u64, instance: PersonalityId) -> Result<Activation, WorkError> {
        self.time(now)?;
        if self.active.is_some() || !self.queued.is_empty() || self.inflight.is_some() {
            return Err(WorkError::Busy);
        }
        let next = self
            .next_generation
            .checked_add(1)
            .ok_or(WorkError::Exhausted)?;
        let token = Activation {
            instance,
            generation: self.next_generation,
        };
        self.next_generation = next;
        self.active = Some(token);
        self.last_now = now;
        Ok(token)
    }

    pub fn enqueue(
        &mut self,
        now: u64,
        token: Activation,
        operation: Option<u64>,
        deadline: u64,
        frame: &[u8],
    ) -> Result<WorkId, WorkError> {
        self.time(now)?;
        if self.active != Some(token) {
            return Err(WorkError::StaleActivation);
        }
        if now >= deadline {
            return Err(WorkError::Expired);
        }
        if self.queued.is_full() {
            return Err(WorkError::Full);
        }
        let bytes = Vec::from_slice(frame).map_err(|_| WorkError::FrameTooLong)?;
        let next = self
            .next_sequence
            .checked_add(1)
            .ok_or(WorkError::Exhausted)?;
        let id = WorkId {
            activation: token,
            sequence: self.next_sequence,
            operation,
        };
        let _ = self.queued.push_back(Transmission {
            id,
            deadline,
            frame: bytes,
        });
        self.next_sequence = next;
        self.last_now = now;
        Ok(id)
    }

    /// Claim one frame. The caller must recheck its deadline against a bounded
    /// physical TX duration. The queue retains it until matching completion.
    pub fn begin(
        &mut self,
        now: u64,
        token: Activation,
    ) -> Result<Option<Transmission>, WorkError> {
        self.time(now)?;
        if self.active != Some(token) {
            return Err(WorkError::StaleActivation);
        }
        if self.inflight.is_some() {
            return Err(WorkError::Busy);
        }
        let Some(work) = self.queued.front() else {
            return Ok(None);
        };
        if now >= work.deadline {
            return Err(WorkError::Expired);
        }
        let work = work.clone();
        self.inflight = Some(work.id);
        self.last_now = now;
        Ok(Some(work))
    }

    /// Completion means the hardware operation ended, not remote delivery.
    /// A late completion is still required to settle custody before switching.
    pub fn complete(&mut self, now: u64, id: WorkId) -> Result<(), WorkError> {
        self.time(now)?;
        if self.inflight != Some(id) {
            return Err(WorkError::WrongCompletion);
        }
        self.queued.pop_front();
        self.inflight = None;
        self.last_now = now;
        Ok(())
    }

    /// Expired queued actions are explicitly returned to their protocol owner.
    /// An in-flight operation cannot be silently revoked on a timer.
    pub fn expire(&mut self, now: u64) -> Result<Vec<WorkId, N>, WorkError> {
        self.time(now)?;
        let mut lost = Vec::new();
        let count = self.queued.len();
        for _ in 0..count {
            let work = self.queued.pop_front().expect("bounded original count");
            if now >= work.deadline && self.inflight != Some(work.id) {
                let _ = lost.push(work.id);
            } else {
                let _ = self.queued.push_back(work);
            }
        }
        self.last_now = now;
        Ok(lost)
    }

    /// Fence the outgoing activation. Permission never overrides physical
    /// in-flight custody. All discarded queued actions are reported.
    pub fn deactivate(&mut self, now: u64, allow_loss: bool) -> Result<Vec<WorkId, N>, WorkError> {
        self.time(now)?;
        if self.inflight.is_some() {
            return Err(WorkError::Busy);
        }
        if !self.queued.is_empty() && !allow_loss {
            return Err(WorkError::LossNotPermitted);
        }
        let mut lost = Vec::new();
        while let Some(work) = self.queued.pop_front() {
            let _ = lost.push(work.id);
        }
        self.active = None;
        self.last_now = now;
        Ok(lost)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_previous_activation_cannot_submit_or_complete_work() {
        let mut q = WorkQueue::<2>::new(0);
        let old = q.activate(0, PersonalityId(1)).unwrap();
        let id = q.enqueue(0, old, Some(9), 20, b"old").unwrap();
        assert_eq!(q.deactivate(1, false), Err(WorkError::LossNotPermitted));
        assert_eq!(q.deactivate(1, true).unwrap().as_slice(), &[id]);
        let new = q.activate(2, PersonalityId(1)).unwrap();
        assert_ne!(old, new);
        assert_eq!(
            q.enqueue(2, old, None, 30, b"stale"),
            Err(WorkError::StaleActivation)
        );
        let current = q.enqueue(2, new, None, 30, b"new").unwrap();
        q.begin(2, new).unwrap();
        assert_eq!(q.complete(3, id), Err(WorkError::WrongCompletion));
        assert_eq!(q.inflight(), Some(current));
        q.complete(3, current).unwrap();
    }
    #[test]
    fn timer_or_permission_cannot_revoke_inflight_custody() {
        let mut q = WorkQueue::<2>::new(0);
        let token = q.activate(0, PersonalityId(1)).unwrap();
        let first = q.enqueue(0, token, None, 5, b"one").unwrap();
        let second = q.enqueue(0, token, None, 5, b"two").unwrap();
        q.begin(0, token).unwrap();
        assert_eq!(q.expire(5).unwrap().as_slice(), &[second]);
        assert_eq!(q.deactivate(5, true), Err(WorkError::Busy));
        q.complete(6, first).unwrap();
        q.deactivate(6, false).unwrap();
    }
    #[test]
    fn capacity_deadline_and_clock_refusals_preserve_queue() {
        let mut q = WorkQueue::<1>::new(1);
        let token = q.activate(1, PersonalityId(1)).unwrap();
        assert_eq!(
            q.enqueue(0, token, None, 5, b"old"),
            Err(WorkError::ClockRegression)
        );
        assert_eq!(
            q.enqueue(1, token, None, 1, b"expired"),
            Err(WorkError::Expired)
        );
        assert_eq!(
            q.enqueue(1, token, None, 5, &[0; 256]),
            Err(WorkError::FrameTooLong)
        );
        let id = q.enqueue(1, token, None, 5, b"one").unwrap();
        assert_eq!(q.enqueue(2, token, None, 5, b"two"), Err(WorkError::Full));
        assert_eq!(q.expire(5).unwrap().as_slice(), &[id]);
    }
}
