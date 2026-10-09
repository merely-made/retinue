//! Retransmission timing for both halves of a transfer, after RNS's resource watchdog
//! (`Resource.py` 577-683). All times are the caller's millisecond ticks.

use super::window::Window;

/// Part timeouts a receiver retries before it gives up. `RNS.Resource.MAX_RETRIES`.
pub const MAX_RETRIES: u8 = 16;
/// Times a sender re-advertises unanswered before it gives up. `MAX_ADV_RETRIES`.
pub const MAX_ADV_RETRIES: u8 = 4;

const PART_TIMEOUT_FACTOR: u64 = 4;
const PART_TIMEOUT_FACTOR_AFTER_RTT: u64 = 2;
const PROOF_TIMEOUT_FACTOR: u64 = 3;
/// `RNS.Link.TRAFFIC_TIMEOUT_FACTOR`.
const TRAFFIC_TIMEOUT_FACTOR: u64 = 6;
const SENDER_GRACE: u64 = 10_000;
const PROCESSING_GRACE: u64 = 1_000;
const RETRY_GRACE: u64 = 250;
const PER_RETRY_DELAY: u64 = 500;

/// The link timing a transfer's watchdog works from, in milliseconds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// The link's round trip; the caller's best guess while it is unmeasured.
    pub rtt: u64,
    /// The shortest wait before any retransmission.
    pub floor: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            rtt: 1_000,
            floor: 0,
        }
    }
}

impl Timing {
    /// How long a receiver waits on `outstanding` parts of `part_size` bytes, after
    /// `retries_used` timeouts, before re-requesting (`Resource.py` 621-634).
    pub(super) fn part_wait(
        &self,
        window: &Window,
        part_size: usize,
        outstanding: usize,
        waiting_for_hmu: bool,
        retries_used: u8,
    ) -> u64 {
        let rate = window.expected_rate(self.rtt);
        let at_rate = |bytes: u64| bytes.saturating_mul(1_000) / rate;
        let part = part_size as u64;
        let wait = if window.responded() {
            let in_flight = at_rate(outstanding as u64 * part);
            let hmu = if waiting_for_hmu || outstanding == 0 {
                at_rate(part * 7 / 2)
            } else {
                0
            };
            PART_TIMEOUT_FACTOR_AFTER_RTT * in_flight + hmu
        } else {
            // RNS budgets three parts' bytes against its bit rate here; kept for parity.
            PART_TIMEOUT_FACTOR * at_rate(3 * part) / 8
        };
        self.floored(wait.saturating_add(RETRY_GRACE + u64::from(retries_used) * PER_RETRY_DELAY))
    }

    /// How long a sender waits for the first request before re-advertising
    /// (`Resource.py` 583-584).
    pub(super) fn advertisement_wait(&self) -> u64 {
        self.floored(
            self.rtt
                .saturating_mul(TRAFFIC_TIMEOUT_FACTOR)
                .saturating_add(PROCESSING_GRACE),
        )
    }

    /// How long a sender serving parts waits for another request before giving up
    /// (`Resource.py` 652-654). `rtt` is the transfer's own, advertisement to request.
    pub(super) fn request_wait(&self, rtt: u64) -> u64 {
        let retries = u64::from(MAX_RETRIES);
        let retry_delays = PER_RETRY_DELAY * retries * (retries + 1) / 2;
        self.floored(
            rtt.saturating_mul(TRAFFIC_TIMEOUT_FACTOR * retries)
                .saturating_add(SENDER_GRACE + retry_delays),
        )
    }

    /// How long a sender that sent every part waits for the proof before asking the
    /// receiver's cache (`Resource.py` 661-666).
    pub(super) fn proof_wait(&self, rtt: u64) -> u64 {
        self.floored(
            rtt.saturating_mul(PROOF_TIMEOUT_FACTOR)
                .saturating_add(SENDER_GRACE),
        )
    }

    fn floored(&self, wait: u64) -> u64 {
        wait.max(self.floor)
    }
}
