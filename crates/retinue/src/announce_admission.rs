//! Bounded host admission using per-key virtual arrival deadlines.
//!
//! Endpoint owns packets and queues. This module owns only rate budgets and counters.
//! The former Prns-influenced implementation is retained in Git history and the donor
//! ledger. The replacement follows the Retinue-owned contract in that ledger; public
//! policy fields/defaults remain compatible. Interface bursts are not reference scheduler
//! parity; the destination announce rate follows RNS (`Transport.py` 2298-2338).

use std::collections::HashMap;
use std::time::Duration;

use crate::endpoint::AnnounceRate;
use crate::hash::AddressHash;

/// Interface-local burst and held-announce policy.
///
/// Endpoint-wide through [`set_announce_ingress_policy`](crate::endpoint::Endpoint::set_announce_ingress_policy),
/// or per interface through
/// [`set_interface_ingress_policy`](crate::endpoint::Endpoint::set_interface_ingress_policy)
/// (RNS `ingress_control` and `ic_*`, `Reticulum.py` 904-927). An interface override uses
/// only the burst and hold fields; the row capacities and the `destination_*` rule stay
/// endpoint-wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceIngressPolicy {
    /// Whether unknown-destination announces may be held during an interface burst. False
    /// is RNS `ingress_control = no`, as every serial-family interface runs.
    pub enabled: bool,
    /// Number of interface rows retained. Least-recently-observed rows are evicted first.
    pub interface_capacity: usize,
    /// Number of destination rate rows retained. Least-recently-allowed rows are evicted first.
    pub destination_capacity: usize,
    /// Maximum verified announcements one interface may hold while it bursts
    /// (`Interface.py` 72, 270-276).
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
    /// The relay rule a transport endpoint applies to one destination's path-updating
    /// announces, unless the ingress interface sets its own [`AnnounceRate`]: RNS
    /// `default_ar_target` (`Reticulum.py` 651-666, 968-971). Zero disables it.
    pub destination_target: Duration,
    /// Violations tolerated before the destination's relays are blocked.
    pub destination_grace: u16,
    /// Extra block time after a destination exceeds its grace.
    pub destination_penalty: Duration,
}

impl Default for AnnounceIngressPolicy {
    /// RNS's defaults (`Interface.py` 70-92).
    fn default() -> Self {
        let rate = AnnounceRate::default();
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
            destination_target: rate.target,
            destination_grace: rate.grace,
            destination_penalty: rate.penalty,
        }
    }
}

impl AnnounceIngressPolicy {
    /// The endpoint-wide destination announce rate, or `None` when disabled.
    pub(crate) fn destination_rate(&self) -> Option<AnnounceRate> {
        (!self.destination_target.is_zero()).then_some(AnnounceRate {
            target: self.destination_target,
            grace: self.destination_grace,
            penalty: self.destination_penalty,
        })
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
    /// Verified announces dropped because the interface's held queue was full, or because
    /// they were too far away to hold.
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

/// RNS's announce rate entry (`Transport.py` 2306-2333), less its unused timestamp list.
#[derive(Debug)]
struct DestinationRate {
    last_used: u64,
    last: u64,
    violations: u16,
    blocked_until: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct AnnounceAdmission {
    policy: AnnounceIngressPolicy,
    interfaces: HashMap<u32, InterfaceBudget>,
    destinations: HashMap<AddressHash, DestinationRate>,
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

    /// Charge one verified announce to interface `id`, under `policy` (its override, else
    /// the endpoint's). Every announce counts toward the burst (`Transport.py` 1812,
    /// `Interface.py` 303-305), but only one for an unknown destination is held (1814-1825).
    pub(crate) fn observe_interface(
        &mut self,
        id: u32,
        known: bool,
        now: u64,
        policy: Option<AnnounceIngressPolicy>,
    ) -> InterfaceVerdict {
        self.attach_interface(id, now);
        let policy = policy.unwrap_or(self.policy);
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
        if !policy.enabled {
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
        if known {
            return InterfaceVerdict::Process;
        }
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

    /// When interface `id` may next release a held announce, under `policy` as above.
    pub(crate) fn release_due(
        &mut self,
        id: u32,
        now: u64,
        policy: Option<AnnounceIngressPolicy>,
    ) -> Option<u64> {
        let policy = policy.unwrap_or(self.policy);
        let row = self.interfaces.get_mut(&id)?;
        let due = if policy.enabled {
            row.resume.max(row.arrival)
        } else {
            now
        };
        if now < due {
            return Some(due);
        }
        row.resume = now.saturating_add(millis(policy.held_release_interval).max(1));
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

    /// Whether a path-updating announce for `key` may be relayed under `rate` (RNS
    /// `Transport.py` 2303-2333): an arrival sooner than the target after the last relayed
    /// one is a violation, a later one forgives one, and past the grace the destination is
    /// blocked until `last + target + penalty`. A blocked announce is still learned.
    pub(crate) fn observe_destination(
        &mut self,
        key: AddressHash,
        rate: AnnounceRate,
        now: u64,
    ) -> DestinationVerdict {
        if self.policy.destination_capacity == 0 {
            return DestinationVerdict::BlockRelay;
        }
        let Some(row) = self.destinations.get_mut(&key) else {
            if self.destinations.len() >= self.policy.destination_capacity {
                let oldest = self
                    .destinations
                    .iter()
                    .min_by_key(|(key, row)| (row.last_used, key.as_bytes()))
                    .map(|(key, _)| *key);
                if let Some(oldest) = oldest {
                    self.destinations.remove(&oldest);
                }
            }
            self.destinations.insert(
                key,
                DestinationRate {
                    last_used: now,
                    last: now,
                    violations: 0,
                    blocked_until: None,
                },
            );
            return DestinationVerdict::Relay;
        };
        row.last_used = now;
        if row.blocked_until.is_some_and(|until| now <= until) {
            return DestinationVerdict::BlockRelay;
        }
        if now.saturating_sub(row.last) < millis(rate.target) {
            row.violations = row.violations.saturating_add(1);
        } else {
            row.violations = row.violations.saturating_sub(1);
        }
        if row.violations > rate.grace {
            row.blocked_until = Some(
                row.last
                    .saturating_add(millis(rate.target))
                    .saturating_add(millis(rate.penalty)),
            );
            return DestinationVerdict::BlockRelay;
        }
        row.last = now;
        DestinationVerdict::Relay
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(n: u8) -> AddressHash {
        AddressHash::from_bytes([n; 16])
    }

    fn held(verdict: InterfaceVerdict) -> bool {
        matches!(verdict, InterfaceVerdict::Hold { .. })
    }

    #[test]
    fn burst_is_isolated_and_known_routes_still_progress() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        let mut observe = |id, known, t| held(a.observe_interface(id, known, t, None));
        let verdicts = [
            observe(1, false, 0),
            observe(1, false, 1),
            observe(1, false, 2),
            observe(2, false, 2),
            observe(1, true, 3),
        ];
        assert_eq!(verdicts, [false, false, true, false, false]);
    }

    /// A storm of re-announces for known destinations trips the burst for the next unknown
    /// one, as RNS counts every valid announce (`Interface.py` 303-305).
    #[test]
    fn known_announces_are_charged_but_never_held() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        assert!((0..5).all(|t| !held(a.observe_interface(1, true, t, None))));
        assert!(held(a.observe_interface(1, false, 5, None)));
    }

    #[test]
    fn an_interface_override_replaces_the_endpoint_policy() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        let off = Some(AnnounceIngressPolicy {
            enabled: false,
            ..Default::default()
        });
        assert!((0..10).all(|t| !held(a.observe_interface(1, false, t, off))));
        assert_eq!(a.counters(1).observed, 10);
        assert_eq!(a.release_due(1, 10, off), Some(10));
    }

    #[test]
    fn release_waits_for_debt_and_cooldown_then_is_paced() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        for t in 0..10 {
            a.observe_interface(1, false, t, None);
        }
        let due = a.release_due(1, 10, None).unwrap();
        assert!(due >= 15_009);
        assert_eq!(a.release_due(1, due, None), Some(due));
        assert_eq!(a.release_due(1, due + 1, None), Some(due + 5_000));
    }

    #[test]
    fn zero_capacity_retains_nothing_and_fails_closed() {
        let mut a = AnnounceAdmission::new(AnnounceIngressPolicy {
            interface_capacity: 0,
            destination_capacity: 0,
            ..Default::default()
        });
        assert!(matches!(
            a.observe_interface(1, false, 0, None),
            InterfaceVerdict::Hold { .. }
        ));
        assert_eq!(
            a.observe_destination(key(1), AnnounceRate::default(), 0),
            DestinationVerdict::BlockRelay
        );
        assert!(a.interfaces.is_empty() && a.destinations.is_empty());
        assert_eq!(
            a.observe_interface(1, true, 0, None),
            InterfaceVerdict::Process
        );
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
            a.observe_interface(i, false, u64::from(i), None);
            a.observe_destination(key(i as u8), AnnounceRate::default(), u64::from(i));
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
            assert_eq!(
                a.observe_interface(1, false, t, None),
                InterfaceVerdict::Process
            );
        }
        a.observe_interface(1, false, u64::MAX, None);
        let r = AnnounceRate {
            target: Duration::MAX,
            grace: 0,
            penalty: Duration::MAX,
        };
        a.observe_destination(key(1), r, u64::MAX);
        a.observe_destination(key(1), r, u64::MAX);
    }
}
