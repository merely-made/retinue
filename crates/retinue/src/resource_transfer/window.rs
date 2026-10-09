//! The receiver's adaptive request window and rate estimate (`Resource.py` 58-100,
//! 862-935). Sans-io: every input carries the caller's millisecond tick.

use crate::resource::WINDOW_MAX;

/// The window a transfer starts at. `RNS.Resource.WINDOW`.
pub const WINDOW_INITIAL: usize = 4;
/// The smallest window. `RNS.Resource.WINDOW_MIN`.
const WINDOW_MIN: usize = 2;
/// The window ceiling until the link proves fast. `RNS.Resource.WINDOW_MAX_SLOW`.
pub const WINDOW_MAX_SLOW: usize = 10;
/// The window ceiling on a very slow link. `RNS.Resource.WINDOW_MAX_VERY_SLOW`.
pub const WINDOW_MAX_VERY_SLOW: usize = 4;
/// The least spread kept between the window's floor and ceiling. `WINDOW_FLEXIBILITY`.
const WINDOW_FLEXIBILITY: usize = 4;
/// Fast rounds before the window may open to [`WINDOW_MAX`]. `FAST_RATE_THRESHOLD`.
pub const FAST_RATE_THRESHOLD: u8 = (WINDOW_MAX_SLOW - WINDOW_INITIAL - 2) as u8;
/// Slow rounds before the window is capped at [`WINDOW_MAX_VERY_SLOW`].
const VERY_SLOW_RATE_THRESHOLD: u8 = 2;
/// A round faster than this, in bytes per second, counts as fast: 50 kbit/s. `RATE_FAST`.
pub const RATE_FAST: u64 = 50_000 / 8;
/// A round slower than this, in bytes per second, counts as very slow: 2 kbit/s.
pub const RATE_VERY_SLOW: u64 = 2_000 / 8;
/// What RNS charges for forming a link, the request, proof and RTT packets at MTU
/// signalling (`Link.establishment_cost`). Divided by the RTT, it is the rate guessed before
/// any part has been timed.
const HANDSHAKE_BYTES: u64 = 287;

/// What one incoming transfer leaves its link for the next (`Link.py` 1257-1266).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowCarry {
    /// The window the last transfer ended at.
    pub window: usize,
    /// Its last expected rate, in bytes per second.
    pub rate: Option<u64>,
}

/// Bytes over `ms` milliseconds, as bytes per second. A tick too short to see counts as one.
fn per_second(bytes: u64, ms: u64) -> u64 {
    bytes.saturating_mul(1_000) / ms.max(1)
}

#[derive(Clone, Debug)]
pub(super) struct Window {
    size: usize,
    min: usize,
    max: usize,
    /// The caller's configured window: the window never exceeds it.
    ceiling: usize,
    fast_rounds: u8,
    very_slow_rounds: u8,
    /// When the outstanding request went out and its encoded size.
    requested_at: Option<u64>,
    request_bytes: usize,
    /// Whether the outstanding request's first part has arrived.
    answered: bool,
    /// Whether any request of this transfer has been answered.
    responded: bool,
    received: u64,
    received_at_request: u64,
    /// The transfer's own RTT, smoothed from the link's.
    rtt: Option<u64>,
    /// The last full round's delivery rate.
    rate: Option<u64>,
    carried_rate: Option<u64>,
}

impl Window {
    pub(super) fn new(ceiling: usize, carry: Option<WindowCarry>) -> Self {
        let ceiling = ceiling.clamp(1, WINDOW_MAX);
        let carry = carry.unwrap_or_default();
        Self {
            size: if carry.window > 0 {
                carry.window
            } else {
                WINDOW_INITIAL
            },
            min: WINDOW_MIN,
            max: WINDOW_MAX_SLOW,
            ceiling,
            fast_rounds: 0,
            very_slow_rounds: 0,
            requested_at: None,
            request_bytes: 0,
            answered: false,
            responded: false,
            received: 0,
            received_at_request: 0,
            rtt: None,
            rate: None,
            carried_rate: carry.rate,
        }
    }

    /// Parts to request and match past the first missing one.
    pub(super) fn size(&self) -> usize {
        self.size.min(self.ceiling)
    }

    /// The ceiling the window may currently grow to.
    pub(super) fn max(&self) -> usize {
        self.max.min(self.ceiling)
    }

    pub(super) fn responded(&self) -> bool {
        self.responded
    }

    pub(super) fn on_request(&mut self, now: u64, bytes: usize) {
        self.requested_at = Some(now);
        self.request_bytes = bytes;
        self.answered = false;
        self.received_at_request = self.received;
    }

    /// A part packet of `packet_len` bytes, `payload_len` of them part data. The first after
    /// a request times the round trip and its rate (`Resource.py` 849-870).
    pub(super) fn on_part(
        &mut self,
        now: u64,
        link_rtt: u64,
        packet_len: usize,
        payload_len: usize,
    ) {
        self.received += payload_len as u64;
        let Some(sent) = self.requested_at.filter(|_| !self.answered) else {
            return;
        };
        self.answered = true;
        self.responded = true;
        let measured = now.saturating_sub(sent);
        // RNS starts from the link's RTT and moves at most 5% a round toward each sample.
        self.rtt = Some(match self.rtt {
            None => link_rtt,
            Some(rtt) if measured < rtt => measured.max(rtt - rtt / 20),
            Some(rtt) => measured.min(rtt + rtt / 20),
        });
        let rate = per_second((packet_len + self.request_bytes) as u64, measured);
        self.count_fast(rate);
    }

    /// Every requested part arrived: open the window a step and rate the round
    /// (`Resource.py` 905-935).
    pub(super) fn on_round(&mut self, now: u64) {
        if self.size < self.max {
            self.size += 1;
            if self.size - self.min > WINDOW_FLEXIBILITY - 1 {
                self.min += 1;
            }
        }
        let Some(sent) = self.requested_at else {
            return;
        };
        let rate = per_second(
            self.received - self.received_at_request,
            now.saturating_sub(sent),
        );
        self.rate = Some(rate);
        self.received_at_request = self.received;
        self.count_fast(rate);
        if self.fast_rounds == 0
            && rate < RATE_VERY_SLOW
            && self.very_slow_rounds < VERY_SLOW_RATE_THRESHOLD
        {
            self.very_slow_rounds += 1;
            if self.very_slow_rounds == VERY_SLOW_RATE_THRESHOLD {
                self.max = WINDOW_MAX_VERY_SLOW;
            }
        }
    }

    fn count_fast(&mut self, rate: u64) {
        if rate > RATE_FAST && self.fast_rounds < FAST_RATE_THRESHOLD {
            self.fast_rounds += 1;
            if self.fast_rounds == FAST_RATE_THRESHOLD {
                self.max = WINDOW_MAX;
            }
        }
    }

    /// A part timeout: close the window a step, and its ceiling with it (`Resource.py`
    /// 640-646).
    pub(super) fn on_timeout(&mut self) {
        if self.size > self.min {
            self.size -= 1;
            if self.max > self.min {
                self.max -= 1;
                if self.max.saturating_sub(self.size) > WINDOW_FLEXIBILITY - 1 {
                    self.max -= 1;
                }
            }
        }
    }

    /// The expected in-flight rate, in bytes per second: the last round's, else the one
    /// carried from the last transfer, else the handshake's (`Resource.update_eifr`).
    pub(super) fn expected_rate(&self, link_rtt: u64) -> u64 {
        self.rate
            .or(self.carried_rate)
            .unwrap_or_else(|| per_second(HANDSHAKE_BYTES, self.rtt.unwrap_or(link_rtt)))
            .max(1)
    }

    pub(super) fn carry(&self, link_rtt: u64) -> WindowCarry {
        WindowCarry {
            window: self.size(),
            rate: Some(self.expected_rate(link_rtt)),
        }
    }
}
