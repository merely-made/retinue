//! Link liveness: the measured RTT, keepalives, staleness and the responder's handshake
//! deadline.
//!
//! Adapted from RNS 1.5.7 `Link.py` (80-106, 204, 419-438, 516-538, 722-802 and 1130-1135).
//! The initiator measures the request-to-proof time and reports it in the RTT packet; the
//! responder takes the larger of that and its own proof-to-RTT time. Both derive a
//! keepalive interval from the RTT: `rtt × 360 / 1.75`, held to 5..360 s. While either
//! direction has been quiet for an interval, the initiator sends a keepalive request at
//! most once per interval; the responder answers one only when it has itself sent nothing
//! for an interval. With nothing heard for two intervals the link goes stale, and
//! `rtt × 4 + 5 s` later it is torn down with a LINKCLOSE unless the peer was heard in the
//! meantime. A responder that never receives the RTT packet drops the link silently at
//! `6 s × (hops + 1) + 360 s`, as RNS does.
//!
//! [`Liveness`] keeps no clock: every call takes the caller's monotonic `now` in
//! milliseconds, so [`Node`](crate::node::Node) drives it from `poll` and the endpoint from
//! a timer. Two choices are retinue's own. Keepalive bytes are matched to the role, so an
//! initiator ignores a request and a responder ignores a response: each can only be our own
//! keepalive heard back from a relay (RNS ignores the first; retinue's own-echo rule covers
//! the second). And neither the endpoint nor `Node` tracks outbound data, only keepalives, so
//! either may send or answer one while data is flowing: never fewer than RNS would.

use crate::link::{KEEPALIVE_REQUEST, KEEPALIVE_RESPONSE, Link};
use crate::packet::Packet;

/// The shortest keepalive interval, in milliseconds (RNS `Link.KEEPALIVE_MIN`).
pub const KEEPALIVE_MIN: u64 = 5_000;
/// The longest keepalive interval, in milliseconds, and the interval before an RTT is known
/// (RNS `Link.KEEPALIVE_MAX`).
pub const KEEPALIVE_MAX: u64 = 360_000;
/// The RTT, in milliseconds, at which the keepalive interval reaches its maximum.
const KEEPALIVE_MAX_RTT: u64 = 1_750;
/// Intervals without inbound traffic before a link is stale.
const STALE_FACTOR: u64 = 2;
/// RTTs a stale link waits for the peer, on top of [`STALE_GRACE`].
const KEEPALIVE_TIMEOUT_FACTOR: u64 = 4;
/// The fixed part of a stale link's wait before teardown, in milliseconds.
pub const STALE_GRACE: u64 = 5_000;

/// The keepalive interval for an RTT, both in milliseconds.
pub const fn keepalive_interval(rtt: u64) -> u64 {
    let interval = rtt.saturating_mul(KEEPALIVE_MAX) / KEEPALIVE_MAX_RTT;
    if interval < KEEPALIVE_MIN {
        KEEPALIVE_MIN
    } else if interval > KEEPALIVE_MAX {
        KEEPALIVE_MAX
    } else {
        interval
    }
}

/// How long a responder waits for the initiator's RTT packet, in milliseconds, for a request
/// that arrived over `hops` relays.
pub const fn handshake_timeout(hops: u8) -> u64 {
    crate::node::LINK_ESTABLISHMENT_TIMEOUT_PER_HOP * (hops as u64 + 1) + KEEPALIVE_MAX
}

/// What [`Liveness::poll`] wants done.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Due {
    /// Send a keepalive request ([`KEEPALIVE_REQUEST`]).
    Keepalive,
    /// The link went stale and its grace ran out: send a LINKCLOSE and drop it.
    Teardown,
    /// The initiator never sent its RTT packet: drop the link without a LINKCLOSE.
    HandshakeTimeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    /// A responder waiting for the RTT packet of a request that arrived at `since`.
    Handshake {
        since: u64,
        deadline: u64,
    },
    Active,
    Stale {
        close_at: u64,
    },
}

/// One established link's liveness timers. See the module documentation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Liveness {
    initiator: bool,
    state: State,
    rtt: Option<u64>,
    keepalive: u64,
    last_inbound: u64,
    last_outbound: u64,
    last_keepalive: u64,
}

impl Liveness {
    /// An initiator's link, active from `now` with the RTT it measured. The RTT packet is
    /// taken as sent at `now`.
    pub fn initiator(rtt: u64, now: u64) -> Self {
        Self {
            initiator: true,
            state: State::Active,
            rtt: Some(rtt),
            keepalive: keepalive_interval(rtt),
            last_inbound: now,
            last_outbound: now,
            last_keepalive: now,
        }
    }

    /// A responder's link, proved at `now` for a request that arrived over `hops` relays. It
    /// waits for the initiator's RTT packet ([`Self::on_rtt`]).
    pub fn responder(now: u64, hops: u8) -> Self {
        Self {
            initiator: false,
            state: State::Handshake {
                since: now,
                deadline: now.saturating_add(handshake_timeout(hops)),
            },
            rtt: None,
            keepalive: KEEPALIVE_MAX,
            last_inbound: now,
            last_outbound: now,
            last_keepalive: now,
        }
    }

    /// The link RTT in milliseconds, once known: the initiator's measurement, or the larger
    /// of both sides' on a responder.
    pub fn rtt(&self) -> Option<u64> {
        self.rtt
    }

    /// The current keepalive interval, in milliseconds.
    pub fn keepalive(&self) -> u64 {
        self.keepalive
    }

    /// When the peer was last heard from.
    pub fn last_inbound(&self) -> u64 {
        self.last_inbound
    }

    /// Whether the link is stale: quiet for two intervals and awaiting teardown.
    pub fn is_stale(&self) -> bool {
        matches!(self.state, State::Stale { .. })
    }

    /// The time at which [`Self::poll`] drops the link if nothing more is heard, when that
    /// is already fixed: a responder's handshake deadline, or a stale link's teardown.
    pub fn teardown_at(&self) -> Option<u64> {
        match self.state {
            State::Handshake { deadline, .. } => Some(deadline),
            State::Stale { close_at } => Some(close_at),
            State::Active => None,
        }
    }

    /// The peer was heard from: any packet on the link other than a keepalive or the RTT
    /// packet, which have their own entry points.
    pub fn on_inbound(&mut self, now: u64) {
        self.last_inbound = self.last_inbound.max(now);
        if self.is_stale() {
            self.state = State::Active;
        }
    }

    /// Something was sent on the link.
    pub fn on_outbound(&mut self, now: u64) {
        self.last_outbound = self.last_outbound.max(now);
    }

    /// The initiator's RTT packet, carrying its measurement `reported` in milliseconds.
    /// A responder takes the larger of that and its own measurement, and becomes active.
    /// An initiator ignores it: it can only be its own, heard back.
    pub fn on_rtt(&mut self, reported: u64, now: u64) {
        if self.initiator {
            return;
        }
        let measured = match self.state {
            State::Handshake { since, .. } => now.saturating_sub(since),
            _ => 0,
        };
        let rtt = measured.max(reported);
        self.rtt = Some(rtt);
        self.keepalive = keepalive_interval(rtt);
        self.state = State::Active;
        self.last_inbound = self.last_inbound.max(now);
    }

    /// A keepalive packet with this payload arrived. Returns whether to answer it with a
    /// [`KEEPALIVE_RESPONSE`]; when it does, the answer is recorded as sent at `now`.
    pub fn on_keepalive(&mut self, payload: &[u8], now: u64) -> bool {
        match (self.initiator, payload) {
            (true, [KEEPALIVE_RESPONSE]) => {
                self.on_inbound(now);
                false
            }
            (false, [KEEPALIVE_REQUEST]) => {
                self.on_inbound(now);
                if now >= self.last_outbound.saturating_add(self.keepalive) {
                    self.last_outbound = now;
                    self.last_keepalive = now;
                    true
                } else {
                    false
                }
            }
            _ => false,
        }
    }

    /// Advance the timers to `now`. A [`Due::Keepalive`] is recorded as sent at `now`.
    pub fn poll(&mut self, now: u64) -> Option<Due> {
        match self.state {
            State::Handshake { deadline, .. } => (now >= deadline).then_some(Due::HandshakeTimeout),
            State::Stale { close_at } => (now >= close_at).then_some(Due::Teardown),
            State::Active => {
                let interval = self.keepalive;
                let quiet_in = now >= self.last_inbound.saturating_add(interval);
                let quiet_out = now >= self.last_outbound.saturating_add(interval);
                if !quiet_in && !quiet_out {
                    return None;
                }
                let mut due = None;
                if self.initiator && now >= self.last_keepalive.saturating_add(interval) {
                    self.last_keepalive = now;
                    self.last_outbound = now;
                    due = Some(Due::Keepalive);
                }
                if now >= self.last_inbound.saturating_add(interval * STALE_FACTOR) {
                    let grace = self
                        .rtt
                        .unwrap_or(0)
                        .saturating_mul(KEEPALIVE_TIMEOUT_FACTOR)
                        .saturating_add(STALE_GRACE);
                    self.state = State::Stale {
                        close_at: now.saturating_add(grace),
                    };
                }
                due
            }
        }
    }
}

/// The RTT an RTT packet reports, in milliseconds, or `None` if it does not decrypt to a
/// MessagePack number of seconds. RNS packs a float64; float32 and unsigned integers are
/// accepted too.
pub fn read_rtt(link: &Link, packet: &Packet) -> Option<u64> {
    let plain = link.decrypt(packet).ok()?;
    let seconds = match plain.as_slice() {
        [0xcb, rest @ ..] => f64::from_be_bytes(rest.try_into().ok()?),
        [0xca, rest @ ..] => f64::from(f32::from_be_bytes(rest.try_into().ok()?)),
        [0xcc, b] => f64::from(*b),
        [0xcd, rest @ ..] => f64::from(u16::from_be_bytes(rest.try_into().ok()?)),
        [b @ 0x00..=0x7f] => f64::from(*b),
        _ => return None,
    };
    (seconds.is_finite() && seconds >= 0.0).then_some((seconds * 1_000.0) as u64)
}

/// An RTT in milliseconds as the seconds an RTT packet carries.
pub fn rtt_seconds(rtt: u64) -> f32 {
    rtt as f32 / 1_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keepalive_interval_follows_rns_and_clamps() {
        assert_eq!(keepalive_interval(0), KEEPALIVE_MIN);
        assert_eq!(keepalive_interval(50), 10_285);
        assert_eq!(keepalive_interval(1_750), KEEPALIVE_MAX);
        assert_eq!(keepalive_interval(u64::MAX), KEEPALIVE_MAX);
        assert_eq!(handshake_timeout(0), 366_000);
        assert_eq!(handshake_timeout(2), 378_000);
    }

    #[test]
    fn an_idle_initiator_sends_one_keepalive_per_interval() {
        let mut l = Liveness::initiator(0, 1_000);
        assert_eq!(l.poll(5_999), None);
        assert_eq!(l.poll(6_000), Some(Due::Keepalive));
        assert_eq!(l.poll(6_500), None);
        // The answer keeps the link fresh; the next request waits a whole interval.
        assert!(!l.on_keepalive(&[KEEPALIVE_RESPONSE], 6_010));
        assert_eq!(l.poll(10_999), None);
        assert_eq!(l.poll(11_000), Some(Due::Keepalive));
    }

    #[test]
    fn a_silent_peer_goes_stale_then_is_torn_down_after_the_grace() {
        let mut l = Liveness::initiator(100, 0);
        let k = l.keepalive();
        assert_eq!(k, 20_571);
        assert_eq!(l.poll(k), Some(Due::Keepalive));
        assert_eq!(l.poll(2 * k), Some(Due::Keepalive));
        assert!(l.is_stale());
        // rtt × 4 + 5 s.
        assert_eq!(l.teardown_at(), Some(2 * k + 5_400));
        assert_eq!(l.poll(2 * k + 5_399), None);
        assert_eq!(l.poll(2 * k + 5_400), Some(Due::Teardown));
    }

    #[test]
    fn traffic_during_the_grace_revives_a_stale_link() {
        let mut l = Liveness::initiator(0, 0);
        let _ = l.poll(10_000);
        assert!(l.is_stale());
        l.on_inbound(12_000);
        assert!(!l.is_stale());
        assert_eq!(l.teardown_at(), None);
        assert_ne!(l.poll(15_000), Some(Due::Teardown));
        assert!(!l.is_stale());
    }

    #[test]
    fn a_responder_answers_requests_only_after_an_interval_of_its_own_silence() {
        let mut l = Liveness::responder(0, 0);
        l.on_rtt(0, 10);
        assert_eq!(l.keepalive(), KEEPALIVE_MIN);
        assert!(!l.on_keepalive(&[KEEPALIVE_REQUEST], 4_000));
        assert!(l.on_keepalive(&[KEEPALIVE_REQUEST], 5_000));
        assert!(!l.on_keepalive(&[KEEPALIVE_REQUEST], 9_000));
        assert!(l.on_keepalive(&[KEEPALIVE_REQUEST], 10_000));
        // A responder never volunteers one.
        assert_eq!(l.poll(15_000), None);
    }

    #[test]
    fn each_role_ignores_the_keepalive_it_sends_itself() {
        let mut initiator = Liveness::initiator(0, 0);
        assert!(!initiator.on_keepalive(&[KEEPALIVE_REQUEST], 9_000));
        assert_eq!(initiator.last_inbound(), 0);
        let mut responder = Liveness::responder(0, 0);
        responder.on_rtt(0, 0);
        assert!(!responder.on_keepalive(&[KEEPALIVE_RESPONSE], 9_000));
        assert_eq!(responder.last_inbound(), 0);
        assert!(!responder.on_keepalive(&[KEEPALIVE_REQUEST, 0], 9_000));
    }

    #[test]
    fn a_responder_takes_the_larger_rtt_and_times_out_without_one() {
        let mut l = Liveness::responder(1_000, 0);
        l.on_rtt(30, 1_200);
        assert_eq!(l.rtt(), Some(200));
        let mut l = Liveness::responder(1_000, 0);
        l.on_rtt(900, 1_200);
        assert_eq!(l.rtt(), Some(900));

        let mut silent = Liveness::responder(1_000, 1);
        assert_eq!(silent.teardown_at(), Some(1_000 + 372_000));
        assert_eq!(silent.poll(372_999), None);
        assert_eq!(silent.poll(373_000), Some(Due::HandshakeTimeout));
    }
}
