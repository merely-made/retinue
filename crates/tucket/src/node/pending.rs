//! Caller-timed text sends awaiting acknowledgement.

use alloc::{string::String, vec::Vec};

use super::CapacityError;

/// Retry behavior for one private text send.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextRetryPolicy {
    /// Total transmissions including the initial attempt, from 1 through 4.
    pub attempts: u8,
    /// Clear a learned path and flood the last attempt.
    pub flood_last: bool,
}

impl TextRetryPolicy {
    pub fn new(attempts: u8, flood_last: bool) -> Option<Self> {
        (1..=4).contains(&attempts).then_some(Self {
            attempts,
            flood_last,
        })
    }
}

impl Default for TextRetryPolicy {
    fn default() -> Self {
        Self {
            attempts: 4,
            flood_last: true,
        }
    }
}

/// Caller-driven state for one text awaiting an acknowledgement.
///
/// Tucket owns attempt numbering, route fallback, and matching delayed ACKs.
/// The caller owns the clock and decides when to ask for the next attempt.
#[derive(Clone, Debug)]
pub struct PendingText {
    pub(super) to: u8,
    pub(super) timestamp: u32,
    pub(super) text: String,
    pub(super) policy: TextRetryPolicy,
    pub(super) next_attempt: u8,
    pub(super) expected_acks: [[u8; 4]; 4],
    pub(super) expected_ack_count: u8,
    pub(super) complete: bool,
}

impl PendingText {
    pub fn attempts_sent(&self) -> u8 {
        self.next_attempt
    }

    pub fn attempts_remaining(&self) -> u8 {
        self.policy.attempts.saturating_sub(self.next_attempt)
    }

    pub fn is_complete(&self) -> bool {
        self.complete
    }

    /// Accept an ACK from any attempt already emitted. Delayed delivery of an
    /// earlier ACK still completes the send.
    pub fn acknowledge(&mut self, ack: [u8; 4]) -> bool {
        if self.expected_acks[..self.expected_ack_count as usize].contains(&ack) {
            self.complete = true;
            true
        } else {
            false
        }
    }
}

/// A bounded caller-owned collection of sends awaiting ACKs.
///
/// This type owns no `Node` state. Its capacity bounds only the caller's
/// outstanding texts, never the number of installed protocol instances.
#[derive(Clone, Debug)]
pub struct PendingTexts {
    entries: Vec<PendingText>,
    capacity: usize,
}

impl PendingTexts {
    pub fn new(capacity: usize) -> Result<Self, CapacityError> {
        if capacity == 0 {
            return Err(CapacityError::ZeroCapacity);
        }
        Ok(Self {
            entries: Vec::with_capacity(capacity),
            capacity,
        })
    }

    pub fn push(&mut self, pending: PendingText) -> Result<(), CapacityError> {
        if self.entries.len() == self.capacity {
            return Err(CapacityError::PendingFull);
        }
        self.entries.push(pending);
        Ok(())
    }

    pub fn as_mut_slice(&mut self) -> &mut [PendingText] {
        &mut self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    pub const fn capacity(&self) -> usize {
        self.capacity
    }
}

/// One concrete transmission produced from a [`PendingText`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextAttempt {
    pub frame: Vec<u8>,
    pub ack: [u8; 4],
    pub attempt: u8,
    pub flooded: bool,
}
