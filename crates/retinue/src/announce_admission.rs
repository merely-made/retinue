//! Bounded, attributed announce-ingress pressure state.
//!
//! This is deliberately separate from packet routing. It records rate facts and returns a
//! verdict; the endpoint owns verified packets, deferred storage, and relay. The shape is
//! adapted from Prns's interface and destination announce-limit state machines, but this is
//! Retinue code with Retinue's bounded host tables and public diagnostics.

use std::collections::HashMap;
use std::time::Duration;

use crate::hash::AddressHash;

/// Interface-local burst and held-announce policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceIngressPolicy {
    /// Whether unknown-destination announces may be held during an interface burst.
    pub enabled: bool,
    /// Number of interface rows retained. Least-recently-observed rows are evicted first.
    pub interface_capacity: usize,
    /// Number of destination rate rows retained. Least-recently-allowed rows are evicted first.
    pub destination_capacity: usize,
    /// Maximum verified announcements waiting for an interface burst to subside.
    pub held_capacity: usize,
    /// An interface is new, and therefore judged at the stricter rate, for this long.
    pub new_interface_age: Duration,
    /// Burst threshold for a new interface.
    pub new_interface_hz: u64,
    /// Burst threshold once an interface has aged past [`Self::new_interface_age`].
    pub established_interface_hz: u64,
    /// Duration of the rate observation window.
    pub frequency_window: Duration,
    /// Minimum time a burst remains latched.
    pub burst_hold: Duration,
    /// Initial delay before a held announce may be released.
    pub burst_penalty: Duration,
    /// Minimum separation between held-announcement releases on one interface.
    pub held_release_interval: Duration,
    /// Minimum normal interval between announcements for one destination.
    pub destination_target: Duration,
    /// Number of fast destination announcements allowed before a block latches.
    pub destination_grace: u16,
    /// Extra block time after a destination exceeds its grace.
    pub destination_penalty: Duration,
}

impl Default for AnnounceIngressPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            interface_capacity: 256,
            destination_capacity: 4_096,
            held_capacity: 256,
            new_interface_age: Duration::from_secs(2 * 60 * 60),
            new_interface_hz: 3,
            established_interface_hz: 10,
            frequency_window: Duration::from_secs(10),
            burst_hold: Duration::from_secs(15),
            burst_penalty: Duration::from_secs(15),
            held_release_interval: Duration::from_secs(5),
            // Preserve Retinue's former one-second destination floor. The state now retains
            // an explicit violation count and unblock deadline rather than a timestamp-only
            // budget; callers may choose a non-zero grace or penalty for a stricter mesh.
            destination_target: Duration::from_secs(1),
            destination_grace: 0,
            destination_penalty: Duration::ZERO,
        }
    }
}

/// The endpoint-visible accounting for one incoming interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnnounceIngressCounters {
    /// Verified announce packets observed from this interface.
    pub observed: u64,
    /// Unknown-route announces retained while this interface was bursting.
    pub held: u64,
    /// Held announces released back to the router.
    pub released: u64,
    /// Verified announces dropped because the bounded held queue was full.
    pub held_dropped: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterfaceVerdict {
    Process,
    Hold { release_at_ms: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DestinationVerdict {
    Relay,
    BlockRelay,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Burst {
    Calm,
    Latched { since_ms: u64 },
}

#[derive(Clone, Copy, Debug)]
struct InterfaceState {
    attached_at_ms: u64,
    window_started_at_ms: u64,
    window_count: u16,
    burst: Burst,
    next_held_release_at_ms: u64,
    counters: AnnounceIngressCounters,
}

#[derive(Clone, Copy, Debug)]
struct DestinationState {
    last_allowed_at_ms: u64,
    blocked_until_ms: u64,
    rate_violations: u16,
}

/// A bounded, deterministic admission ledger. Times are monotonic milliseconds relative to
/// the endpoint's creation so tests can exercise burst and release behavior without sleeping.
#[derive(Debug)]
pub(crate) struct AnnounceAdmission {
    policy: AnnounceIngressPolicy,
    interfaces: HashMap<u32, InterfaceState>,
    destinations: HashMap<AddressHash, DestinationState>,
}

impl AnnounceAdmission {
    pub(crate) fn new(policy: AnnounceIngressPolicy) -> Self {
        Self {
            policy,
            interfaces: HashMap::new(),
            destinations: HashMap::new(),
        }
    }

    pub(crate) fn set_policy(&mut self, policy: AnnounceIngressPolicy) {
        self.policy = policy;
        self.trim_interfaces();
        self.trim_destinations();
    }

    pub(crate) fn policy(&self) -> AnnounceIngressPolicy {
        self.policy
    }

    pub(crate) fn attach_interface(&mut self, interface: u32, now_ms: u64) {
        self.interface_mut(interface, now_ms);
    }

    pub(crate) fn forget_interface(&mut self, interface: u32) {
        self.interfaces.remove(&interface);
    }

    /// Count a verified announce and decide whether an as-yet-unknown route should wait.
    pub(crate) fn observe_interface(
        &mut self,
        interface: u32,
        route_is_known: bool,
        now_ms: u64,
    ) -> InterfaceVerdict {
        let policy = self.policy;
        let row = self.interface_mut(interface, now_ms);
        row.counters.observed = row.counters.observed.saturating_add(1);
        if row.window_count == 0
            || now_ms.saturating_sub(row.window_started_at_ms)
                >= duration_ms(policy.frequency_window)
        {
            row.window_started_at_ms = now_ms;
            row.window_count = 1;
        } else {
            row.window_count = row.window_count.saturating_add(1);
        }

        if !policy.enabled || route_is_known {
            return InterfaceVerdict::Process;
        }

        let rate = rate_reading(row, now_ms, policy);
        match row.burst {
            Burst::Calm if rate == RateReading::Over => {
                row.burst = Burst::Latched { since_ms: now_ms };
                row.next_held_release_at_ms =
                    now_ms.saturating_add(duration_ms(policy.burst_penalty));
                InterfaceVerdict::Hold {
                    release_at_ms: row.next_held_release_at_ms,
                }
            }
            Burst::Latched { since_ms }
                if rate == RateReading::Under
                    && now_ms >= since_ms.saturating_add(duration_ms(policy.burst_hold))
                    && row.window_count >= 2 =>
            {
                row.burst = Burst::Calm;
                InterfaceVerdict::Hold {
                    release_at_ms: row.next_held_release_at_ms,
                }
            }
            Burst::Latched { .. } => InterfaceVerdict::Hold {
                release_at_ms: row.next_held_release_at_ms,
            },
            Burst::Calm => InterfaceVerdict::Process,
        }
    }

    /// Return the next release deadline only when an interface has calmed below its threshold.
    pub(crate) fn release_due(&mut self, interface: u32, now_ms: u64) -> Option<u64> {
        let policy = self.policy;
        let row = self.interfaces.get_mut(&interface)?;
        if now_ms < row.next_held_release_at_ms {
            return Some(row.next_held_release_at_ms);
        }
        if rate_reading(row, now_ms, policy) != RateReading::Under {
            return Some(now_ms.saturating_add(duration_ms(policy.held_release_interval)));
        }
        if let Burst::Latched { since_ms } = row.burst
            && now_ms >= since_ms.saturating_add(duration_ms(policy.burst_hold))
            && row.window_count >= 2
        {
            row.burst = Burst::Calm;
        }
        row.next_held_release_at_ms =
            now_ms.saturating_add(duration_ms(policy.held_release_interval));
        Some(now_ms)
    }

    pub(crate) fn note_held(&mut self, interface: u32) {
        if let Some(row) = self.interfaces.get_mut(&interface) {
            row.counters.held = row.counters.held.saturating_add(1);
        }
    }

    pub(crate) fn note_held_dropped(&mut self, interface: u32) {
        if let Some(row) = self.interfaces.get_mut(&interface) {
            row.counters.held_dropped = row.counters.held_dropped.saturating_add(1);
        }
    }

    pub(crate) fn note_released(&mut self, interface: u32) {
        if let Some(row) = self.interfaces.get_mut(&interface) {
            row.counters.released = row.counters.released.saturating_add(1);
        }
    }

    pub(crate) fn counters(&self, interface: u32) -> AnnounceIngressCounters {
        self.interfaces
            .get(&interface)
            .map_or(AnnounceIngressCounters::default(), |row| row.counters)
    }

    /// Apply per-destination rate accounting at the point that would relay an announce.
    pub(crate) fn observe_destination(
        &mut self,
        destination: AddressHash,
        now_ms: u64,
    ) -> DestinationVerdict {
        let policy = self.policy;
        let target_ms = duration_ms(policy.destination_target);
        if target_ms == 0 {
            return DestinationVerdict::Relay;
        }
        if let Some(row) = self.destinations.get_mut(&destination) {
            if now_ms < row.blocked_until_ms {
                return DestinationVerdict::BlockRelay;
            }
            if now_ms.saturating_sub(row.last_allowed_at_ms) < target_ms {
                row.rate_violations = row.rate_violations.saturating_add(1);
            } else {
                row.rate_violations = row.rate_violations.saturating_sub(1);
            }
            if row.rate_violations > policy.destination_grace {
                row.blocked_until_ms = row
                    .last_allowed_at_ms
                    .saturating_add(target_ms)
                    .saturating_add(duration_ms(policy.destination_penalty));
                DestinationVerdict::BlockRelay
            } else {
                row.last_allowed_at_ms = now_ms;
                DestinationVerdict::Relay
            }
        } else {
            self.insert_destination(
                destination,
                DestinationState {
                    last_allowed_at_ms: now_ms,
                    blocked_until_ms: 0,
                    rate_violations: 0,
                },
            );
            DestinationVerdict::Relay
        }
    }

    fn interface_mut(&mut self, interface: u32, now_ms: u64) -> &mut InterfaceState {
        if !self.interfaces.contains_key(&interface) {
            if self.policy.interface_capacity == 0 {
                // Capacity zero still needs a short-lived row to produce a verdict. It is
                // immediately eligible for eviction on the next distinct interface.
                self.interfaces.clear();
            } else if self.interfaces.len() >= self.policy.interface_capacity {
                let evict = self
                    .interfaces
                    .iter()
                    .min_by_key(|(_, row)| row.window_started_at_ms)
                    .map(|(id, _)| *id);
                if let Some(id) = evict {
                    self.interfaces.remove(&id);
                }
            }
            self.interfaces.insert(
                interface,
                InterfaceState {
                    attached_at_ms: now_ms,
                    window_started_at_ms: now_ms,
                    window_count: 0,
                    burst: Burst::Calm,
                    next_held_release_at_ms: now_ms,
                    counters: AnnounceIngressCounters::default(),
                },
            );
        }
        self.interfaces.get_mut(&interface).expect("inserted above")
    }

    fn insert_destination(&mut self, destination: AddressHash, row: DestinationState) {
        if self.policy.destination_capacity == 0 {
            return;
        }
        if self.destinations.len() >= self.policy.destination_capacity {
            let evict = self
                .destinations
                .iter()
                .min_by_key(|(_, row)| row.last_allowed_at_ms)
                .map(|(destination, _)| *destination);
            if let Some(destination) = evict {
                self.destinations.remove(&destination);
            }
        }
        self.destinations.insert(destination, row);
    }

    fn trim_interfaces(&mut self) {
        while self.interfaces.len() > self.policy.interface_capacity {
            let evict = self
                .interfaces
                .iter()
                .min_by_key(|(_, row)| row.window_started_at_ms)
                .map(|(id, _)| *id);
            if let Some(id) = evict {
                self.interfaces.remove(&id);
            } else {
                break;
            }
        }
    }

    fn trim_destinations(&mut self) {
        while self.destinations.len() > self.policy.destination_capacity {
            let evict = self
                .destinations
                .iter()
                .min_by_key(|(_, row)| row.last_allowed_at_ms)
                .map(|(destination, _)| *destination);
            if let Some(destination) = evict {
                self.destinations.remove(&destination);
            } else {
                break;
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RateReading {
    Under,
    At,
    Over,
}

fn rate_reading(row: &InterfaceState, now_ms: u64, policy: AnnounceIngressPolicy) -> RateReading {
    let limit_hz =
        if now_ms.saturating_sub(row.attached_at_ms) < duration_ms(policy.new_interface_age) {
            policy.new_interface_hz
        } else {
            policy.established_interface_hz
        };
    let elapsed_ms = now_ms.saturating_sub(row.window_started_at_ms);
    if row.window_count < 3 || elapsed_ms == 0 || limit_hz == 0 {
        return RateReading::Under;
    }
    let count = u128::from(row.window_count) * 1_000;
    let limit = u128::from(limit_hz) * u128::from(elapsed_ms);
    if count < limit {
        RateReading::Under
    } else if count == limit {
        RateReading::At
    } else {
        RateReading::Over
    }
}

fn duration_ms(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn destination(byte: u8) -> AddressHash {
        AddressHash::from_bytes([byte; 16])
    }

    #[test]
    fn noisy_interface_is_held_without_silencing_a_quiet_neighbor() {
        let mut admission = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        for n in 0..2 {
            assert_eq!(
                admission.observe_interface(1, false, n * 100),
                InterfaceVerdict::Process
            );
        }
        assert!(matches!(
            admission.observe_interface(1, false, 200),
            InterfaceVerdict::Hold { .. }
        ));
        assert_eq!(
            admission.observe_interface(2, false, 300),
            InterfaceVerdict::Process
        );
    }

    #[test]
    fn held_releases_after_the_penalty_when_the_interface_has_calmed() {
        let mut admission = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        for n in 0..4 {
            let _ = admission.observe_interface(1, false, n * 100);
        }
        assert_eq!(admission.release_due(1, 14_999), Some(15_200));
        assert_eq!(admission.release_due(1, 15_200), Some(15_200));
        assert_eq!(admission.release_due(1, 15_201), Some(20_200));
    }

    #[test]
    fn destination_violations_escalate_to_a_block_and_recover() {
        let policy = AnnounceIngressPolicy {
            destination_target: Duration::from_secs(10),
            destination_grace: 2,
            destination_penalty: Duration::from_secs(60),
            ..AnnounceIngressPolicy::default()
        };
        let mut admission = AnnounceAdmission::new(policy);
        assert_eq!(
            admission.observe_destination(destination(1), 0),
            DestinationVerdict::Relay
        );
        assert_eq!(
            admission.observe_destination(destination(1), 1_000),
            DestinationVerdict::Relay
        );
        assert_eq!(
            admission.observe_destination(destination(1), 2_000),
            DestinationVerdict::Relay
        );
        assert_eq!(
            admission.observe_destination(destination(1), 3_000),
            DestinationVerdict::BlockRelay
        );
        assert_eq!(
            admission.observe_destination(destination(1), 72_000),
            DestinationVerdict::Relay
        );
    }
}
