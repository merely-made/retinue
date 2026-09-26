//! Bounded host admission using per-key virtual arrival deadlines.
//!
//! Endpoint owns packets and queues. This module owns only rate budgets and counters.
//! The former Prns-influenced implementation is retained in Git history and the donor
//! ledger. The replacement follows the Retinue-owned contract in that ledger; public
//! policy fields/defaults remain compatible. This is not reference scheduler parity.

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
    /// Maximum accumulated interface rate-debt horizon.
    pub frequency_window: Duration,
    /// Minimum interface cooldown after excess traffic.
    pub burst_hold: Duration,
    /// Initial delay before a held announce may be released.
    pub burst_penalty: Duration,
    /// Minimum separation between held-announcement releases on one interface.
    pub held_release_interval: Duration,
    /// Minimum normal interval between announcements for one destination.
    pub destination_target: Duration,
    /// Number of extra immediate destination announcements allowed.
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
            // Preserve the public one-second destination floor. Grace allows extra
            // immediate events; penalty delays recovery after the budget is exceeded.
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

#[derive(Debug)]
struct InterfaceBudget {
    attached: u64,
    last_used: u64,
    arrival: u64,
    resume: u64,
    counters: AnnounceIngressCounters,
}

#[derive(Debug)]
struct DestinationBudget {
    last_used: u64,
    arrival: u64,
    resume: u64,
}

#[derive(Debug)]
pub(crate) struct AnnounceAdmission {
    policy: AnnounceIngressPolicy,
    interfaces: HashMap<u32, InterfaceBudget>,
    destinations: HashMap<AddressHash, DestinationBudget>,
}

fn millis(d: Duration) -> u64 {
    d.as_millis().min(u128::from(u64::MAX)) as u64
}

fn period(hz: u64) -> u64 {
    if hz == 0 { 0 } else { 1_000u64.div_ceil(hz) }
}

impl AnnounceAdmission {
    pub(crate) fn new(policy: AnnounceIngressPolicy) -> Self {
        Self {
            policy,
            interfaces: HashMap::new(),
            destinations: HashMap::new(),
        }
    }

    pub(crate) fn policy(&self) -> AnnounceIngressPolicy {
        self.policy
    }

    pub(crate) fn set_policy(&mut self, policy: AnnounceIngressPolicy) {
        self.policy = policy;
        // Keep accounting and in-flight cooldowns; reset only rate debt.
        for row in self.interfaces.values_mut() {
            row.arrival = row.last_used;
        }
        while self.interfaces.len() > policy.interface_capacity {
            let oldest = self
                .interfaces
                .iter()
                .min_by_key(|(key, row)| (row.last_used, **key))
                .map(|(key, _)| *key);
            if let Some(key) = oldest {
                self.interfaces.remove(&key);
            }
        }
        self.destinations.clear();
    }

    pub(crate) fn attach_interface(&mut self, id: u32, now: u64) {
        if self.policy.interface_capacity == 0 || self.interfaces.contains_key(&id) {
            return;
        }
        if self.interfaces.len() >= self.policy.interface_capacity {
            let oldest = self
                .interfaces
                .iter()
                .min_by_key(|(key, row)| (row.last_used, **key))
                .map(|(key, _)| *key);
            if let Some(key) = oldest {
                self.interfaces.remove(&key);
            }
        }
        self.interfaces.insert(
            id,
            InterfaceBudget {
                attached: now,
                last_used: now,
                arrival: now,
                resume: now,
                counters: AnnounceIngressCounters::default(),
            },
        );
    }

    pub(crate) fn forget_interface(&mut self, id: u32) {
        self.interfaces.remove(&id);
    }

    pub(crate) fn observe_interface(&mut self, id: u32, known: bool, now: u64) -> InterfaceVerdict {
        self.attach_interface(id, now);
        let policy = self.policy;
        let Some(row) = self.interfaces.get_mut(&id) else {
            return if known || !policy.enabled {
                InterfaceVerdict::Process
            } else {
                InterfaceVerdict::Hold {
                    release_at_ms: now.saturating_add(millis(policy.held_release_interval).max(1)),
                }
            };
        };
        row.last_used = now;
        row.counters.observed = row.counters.observed.saturating_add(1);
        if known || !policy.enabled {
            return InterfaceVerdict::Process;
        }
        let hz = if now.saturating_sub(row.attached) < millis(policy.new_interface_age) {
            policy.new_interface_hz
        } else {
            policy.established_interface_hz
        };
        let step = period(hz);
        if step == 0 {
            return InterfaceVerdict::Process;
        }
        let excess = row.arrival > now.saturating_add(step);
        row.arrival =
            row.arrival.max(now).saturating_add(step).min(
                now.saturating_add(millis(policy.frequency_window).max(step.saturating_mul(2))),
            );
        if excess {
            row.resume = row.resume.max(
                now.saturating_add(millis(policy.burst_hold).max(millis(policy.burst_penalty))),
            );
        }
        if excess || now < row.resume {
            InterfaceVerdict::Hold {
                release_at_ms: row.resume.max(row.arrival),
            }
        } else {
            InterfaceVerdict::Process
        }
    }

    pub(crate) fn release_due(&mut self, id: u32, now: u64) -> Option<u64> {
        let row = self.interfaces.get_mut(&id)?;
        let due = if self.policy.enabled {
            row.resume.max(row.arrival)
        } else {
            now
        };
        if now < due {
            return Some(due);
        }
        row.resume = now.saturating_add(millis(self.policy.held_release_interval).max(1));
        Some(now)
    }

    pub(crate) fn counters(&self, id: u32) -> AnnounceIngressCounters {
        self.interfaces
            .get(&id)
            .map(|r| r.counters)
            .unwrap_or_default()
    }
    pub(crate) fn note_held(&mut self, id: u32) {
        if let Some(r) = self.interfaces.get_mut(&id) {
            r.counters.held = r.counters.held.saturating_add(1);
        }
    }
    pub(crate) fn note_held_dropped(&mut self, id: u32) {
        if let Some(r) = self.interfaces.get_mut(&id) {
            r.counters.held_dropped = r.counters.held_dropped.saturating_add(1);
        }
    }
    pub(crate) fn note_released(&mut self, id: u32) {
        if let Some(r) = self.interfaces.get_mut(&id) {
            r.counters.released = r.counters.released.saturating_add(1);
        }
    }

    pub(crate) fn observe_destination(&mut self, key: AddressHash, now: u64) -> DestinationVerdict {
        let step = millis(self.policy.destination_target);
        if step == 0 {
            return DestinationVerdict::Relay;
        }
        if self.policy.destination_capacity == 0 {
            return DestinationVerdict::BlockRelay;
        }
        if !self.destinations.contains_key(&key)
            && self.destinations.len() >= self.policy.destination_capacity
        {
            let oldest = self
                .destinations
                .iter()
                .min_by_key(|(key, row)| (row.last_used, key.as_bytes()))
                .map(|(key, _)| *key);
            if let Some(oldest) = oldest {
                self.destinations.remove(&oldest);
            }
        }
        let row = self.destinations.entry(key).or_insert(DestinationBudget {
            last_used: now,
            arrival: now,
            resume: now,
        });
        row.last_used = now;
        if now < row.resume {
            return DestinationVerdict::BlockRelay;
        }
        let tolerance = step.saturating_mul(u64::from(self.policy.destination_grace));
        if row.arrival > now.saturating_add(tolerance) {
            row.resume = row
                .arrival
                .saturating_add(millis(self.policy.destination_penalty));
            return DestinationVerdict::BlockRelay;
        }
        row.arrival = row.arrival.max(now).saturating_add(step);
        DestinationVerdict::Relay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(n: u8) -> AddressHash {
        AddressHash::from_bytes([n; 16])
    }

    #[test]
    fn burst_is_isolated_and_known_routes_still_progress() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        assert_eq!(a.observe_interface(1, false, 0), InterfaceVerdict::Process);
        assert_eq!(a.observe_interface(1, false, 1), InterfaceVerdict::Process);
        assert!(matches!(
            a.observe_interface(1, false, 2),
            InterfaceVerdict::Hold { .. }
        ));
        assert_eq!(a.observe_interface(2, false, 2), InterfaceVerdict::Process);
        assert_eq!(a.observe_interface(1, true, 3), InterfaceVerdict::Process);
    }

    #[test]
    fn release_waits_for_debt_and_cooldown_then_is_paced() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        for t in 0..10 {
            a.observe_interface(1, false, t);
        }
        let due = a.release_due(1, 10).unwrap();
        assert!(due >= 15_009);
        assert_eq!(a.release_due(1, due), Some(due));
        assert_eq!(a.release_due(1, due + 1), Some(due + 5_000));
    }

    #[test]
    fn destination_grace_is_finite_and_repeated_refusal_does_not_extend_penalty() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy {
            destination_target: Duration::from_secs(10),
            destination_grace: 2,
            destination_penalty: Duration::from_secs(60),
            ..Default::default()
        });
        for t in [0, 1_000, 2_000] {
            assert_eq!(a.observe_destination(key(1), t), DestinationVerdict::Relay);
        }
        assert_eq!(
            a.observe_destination(key(1), 3_000),
            DestinationVerdict::BlockRelay
        );
        assert_eq!(
            a.observe_destination(key(1), 89_999),
            DestinationVerdict::BlockRelay
        );
        assert_eq!(
            a.observe_destination(key(1), 90_000),
            DestinationVerdict::Relay
        );
        assert_eq!(
            a.observe_destination(key(2), 3_000),
            DestinationVerdict::Relay
        );
    }

    #[test]
    fn zero_capacity_retains_nothing_and_fails_closed() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy {
            interface_capacity: 0,
            destination_capacity: 0,
            ..Default::default()
        });
        assert!(matches!(
            a.observe_interface(1, false, 0),
            InterfaceVerdict::Hold { .. }
        ));
        assert_eq!(
            a.observe_destination(key(1), 0),
            DestinationVerdict::BlockRelay
        );
        assert!(a.interfaces.is_empty() && a.destinations.is_empty());
        assert_eq!(a.observe_interface(1, true, 0), InterfaceVerdict::Process);
    }

    #[test]
    fn eviction_and_policy_reset_bound_retention() {
        let p = AnnounceIngressPolicy {
            interface_capacity: 1,
            destination_capacity: 1,
            ..Default::default()
        };
        let mut a = AnnounceAdmission::new(p);
        for i in 1..4 {
            a.observe_interface(i, false, u64::from(i));
            a.observe_destination(key(i as u8), u64::from(i));
        }
        assert_eq!(a.interfaces.len(), 1);
        assert_eq!(a.destinations.len(), 1);
        assert!(a.interfaces.contains_key(&3));
        assert!(a.destinations.contains_key(&key(3)));
        a.set_policy(p);
        assert_eq!(a.interfaces.len(), 1);
        assert!(a.destinations.is_empty());
        assert_eq!(a.interfaces[&3].arrival, a.interfaces[&3].last_used);
    }

    #[test]
    fn sustained_regular_traffic_is_admitted_and_clock_limits_do_not_panic() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        for t in (0..10_000).step_by(1_000) {
            assert_eq!(a.observe_interface(1, false, t), InterfaceVerdict::Process);
        }
        a.observe_interface(1, false, u64::MAX);
        a.observe_destination(key(1), u64::MAX);
    }
}
