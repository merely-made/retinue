//! A resource transfer's policy and clock on an endpoint.

use std::time::Duration;

use tokio::time::Instant;

/// Runtime policy for an endpoint-driven resource transfer. Retransmission otherwise
/// follows RNS's watchdog, from the link's RTT and the measured rate.
#[derive(Clone, Copy, Debug)]
pub struct ResourceTransferConfig {
    /// The longest a transfer may hear nothing from its peer before it fails. A split
    /// Resource runs as long as its segments keep progressing.
    pub timeout: Duration,
    /// The shortest wait before any retransmission, and the RTT assumed until the link has
    /// measured one.
    pub retry_interval: Duration,
    /// Ceiling on the adaptive request window, in parts.
    pub request_window: usize,
}

impl Default for ResourceTransferConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry_interval: Duration::from_millis(500),
            request_window: crate::resource::WINDOW_MAX,
        }
    }
}

/// A transfer's clock: the millisecond ticks the state machines take, and the silence
/// limit after which the transfer fails.
pub(super) struct Pace {
    start: Instant,
    heard: Instant,
    idle: Duration,
}

impl Pace {
    pub(super) fn new(idle: Duration) -> Self {
        let start = Instant::now();
        Self {
            start,
            heard: start,
            idle,
        }
    }

    pub(super) fn now(&self) -> u64 {
        self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }

    pub(super) fn heard(&mut self) {
        self.heard = Instant::now();
    }

    /// Carry on the same clock with a new silence limit, counted from now.
    pub(super) fn resume(&mut self, idle: Duration) {
        self.idle = idle;
        self.heard();
    }

    /// When to wake next: the transfer's deadline or the silence limit, whichever is first.
    pub(super) fn wake(&self, deadline: Option<u64>) -> Instant {
        let idle = self.heard + self.idle;
        deadline
            .and_then(|at| self.start.checked_add(Duration::from_millis(at)))
            .map_or(idle, |at| at.min(idle))
    }

    pub(super) fn idle(&self) -> bool {
        self.heard.elapsed() >= self.idle
    }
}
