//! Send-window and retransmit-timeout policy, from RNS 1.5.7 `Channel.py` 200-245.
//!
//! The window bounds unproved envelopes in flight. It grows by one per proof up to
//! `window_max`, which is promoted a tier after `FAST_RATE_THRESHOLD` proofs below that
//! tier's RTT; each timeout shrinks it by one. This is local send-rate policy, never on the
//! wire, so matching RNS's tiers is a tuning choice.

/// The starting send window of a dynamic channel (RNS `WINDOW`).
pub const WINDOW_INITIAL: u32 = 2;
/// The window never shrinks below this (RNS `WINDOW_MIN`), unless the link starts slower
/// than the slow RTT tier, when RNS pins the whole window to one.
pub const WINDOW_MIN: u32 = 2;
/// The window never grows above this (RNS `WINDOW_MAX`, the fast-tier ceiling).
pub const WINDOW_MAX: u32 = 48;
/// The smallest gap a timeout leaves between `window_max` and `window_min` (RNS
/// `WINDOW_FLEXIBILITY`): a timeout lowers `window_max` by one only while it is more than
/// this far above the floor. It is not a step size; the window itself drops by one.
pub const WINDOW_FLEXIBILITY: u32 = 4;

pub(super) const WINDOW_MAX_SLOW: u32 = 5;
pub(super) const WINDOW_MAX_MEDIUM: u32 = 12;
pub(super) const WINDOW_MAX_FAST: u32 = 48;
pub(super) const WINDOW_MIN_LIMIT_MEDIUM: u32 = 5;
pub(super) const WINDOW_MIN_LIMIT_FAST: u32 = 16;
// RTT tier thresholds, in the caller's tick unit. RNS's are seconds; these read a tick
// as a millisecond (RNS RTT_FAST/MEDIUM/SLOW = 0.18 / 0.75 / 1.45 s).
pub(super) const RTT_FAST: u64 = 180;
pub(super) const RTT_MEDIUM: u64 = 750;
pub(super) const RTT_SLOW: u64 = 1450;
/// Proofs measured below a tier's RTT before `window_max` is promoted to that tier (RNS
/// `FAST_RATE_THRESHOLD`).
pub(super) const FAST_RATE_THRESHOLD: u32 = 10;

/// Ticks without a proof before a fixed channel
/// ([`Channel::with_params`](super::Channel::with_params)) retransmits. A tick is the unit
/// passed to [`Channel::poll_transmit`](super::Channel::poll_transmit) (milliseconds over a
/// real clock; a counter in tests).
pub const DEFAULT_RETX_TIMEOUT: u64 = 4;

/// How many times one envelope goes on the wire before the channel gives up (RNS
/// `Channel._max_tries`). The first send is a try, so this is the transmission count.
pub const DEFAULT_MAX_TRIES: u8 = 5;

/// The floor of the per-envelope timeout's RTT term, in ticks read as milliseconds (RNS
/// `max(rtt * 2.5, 0.025)`).
const RETX_RTT_FLOOR: u64 = 25;

/// How long an envelope on its `tries`-th transmission waits for its proof, with
/// `outstanding` envelopes in flight, given an RTT estimate. RNS `_get_packet_timeout_time`:
/// `1.5^(tries-1) * max(2.5 * rtt, 25 ms) * (outstanding + 1.5)`, in integer ticks. The
/// backoff exponent stops at the default try count, so a raised limit cannot overflow it.
pub(super) fn retx_timeout(rtt: u64, tries: u8, outstanding: usize) -> u64 {
    let backoff = u32::from(tries.clamp(1, DEFAULT_MAX_TRIES) - 1);
    let base = (rtt.saturating_mul(5) / 2).max(RETX_RTT_FLOOR);
    base.saturating_mul(2 * outstanding as u64 + 3)
        .saturating_mul(3u64.pow(backoff))
        / (2u64 << backoff)
}
