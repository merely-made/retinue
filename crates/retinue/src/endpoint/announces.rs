//! Announce ingress: admission, held release, freshness, and relay.

use alloc::vec::Vec;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::announce::{Announce, AnnounceBlob};
use crate::announce_admission::{
    AnnounceIngressCounters, AnnounceIngressPolicy, DestinationVerdict, InterfaceVerdict,
};
use crate::announce_freshness::{
    AnnounceFreshness, AnnounceFreshnessCandidate, AnnounceFreshnessConfig,
    AnnounceFreshnessConfigError, AnnounceFreshnessDecision, AnnounceFreshnessReject,
};
use crate::packet::Packet;

use super::facts::PeerAnnounce;
use super::interface::InterfaceId;
use super::paths::PATH_REQUEST_GATE;
use super::routing::MAX_HOPS;
use super::runtime::{Endpoint, recv_until_closed, track};
use super::shared::Shared;

/// Host-owned policy for receive-side announce freshness: whether a verified announce may
/// change peer, path, publication, or relay state. Independent of the packet-loop cache, and
/// a destination's freshness lives as long as its route (RNS keeps blobs on the path row).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessPolicy {
    /// How long a learned route, and with it the destination's freshness, stays live. One
    /// week by default, as in RNS ([`crate::node::DEFAULT_ROUTE_TTL`]); a route carrying
    /// traffic is refreshed, and an access-point or roaming interface shortens it (see
    /// [`InterfaceMode`](crate::node::InterfaceMode)).
    pub route_ttl: Duration,
    /// Maximum destinations retained in the freshness ledger. Evicting one drops its route.
    pub destination_capacity: usize,
    /// Maximum full announce blobs retained for one destination.
    pub blob_capacity: usize,
}

impl Default for AnnounceFreshnessPolicy {
    fn default() -> Self {
        Self {
            route_ttl: Duration::from_millis(crate::node::DEFAULT_ROUTE_TTL),
            destination_capacity: 4_096,
            blob_capacity: 16,
        }
    }
}

impl AnnounceFreshnessPolicy {
    fn config(self) -> AnnounceFreshnessConfig {
        AnnounceFreshnessConfig {
            destination_capacity: self.destination_capacity,
            blob_capacity: self.blob_capacity,
        }
    }

    pub(super) fn route_ttl_ticks(self) -> u64 {
        duration_ticks(self.route_ttl)
    }
}

pub(super) fn duration_ticks(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

/// A verified announce deferred by a noisy interface. It retains the ingress fact through
/// release so a later route cannot be accidentally attributed to a different bearer.
pub(super) struct HeldAnnounce {
    pub(super) interface: InterfaceId,
    pub(super) packet: Packet,
    pub(super) announce: Announce,
}

/// The endpoint holds at most this many interfaces' worth of the endpoint `held_capacity`,
/// so many noisy interfaces (a listener's clients) cannot each fill a full share.
const HELD_SHARES: usize = 8;

/// Held announces by ingress interface (`Interface.py` 72, 270-297). No interface keeps an
/// empty queue, and `total` is the sum of the queue lengths.
#[derive(Default)]
pub(super) struct HeldAnnounces {
    by_iface: HashMap<InterfaceId, Vec<HeldAnnounce>>,
    total: usize,
}

impl HeldAnnounces {
    /// Hold `held`, replacing one held for its destination in place (`Interface.py`
    /// 270-276). False when its interface holds `capacity` or the endpoint `ceiling`.
    pub(super) fn hold(&mut self, held: HeldAnnounce, capacity: usize, ceiling: usize) -> bool {
        let iface = held.interface;
        let queue = self.by_iface.entry(iface).or_default();
        let destination = held.announce.destination;
        if let Some(existing) = queue
            .iter_mut()
            .find(|existing| existing.announce.destination == destination)
        {
            *existing = held;
            return true;
        }
        if queue.len() < capacity && self.total < ceiling {
            queue.push(held);
            self.total += 1;
            return true;
        }
        if queue.is_empty() {
            self.by_iface.remove(&iface);
        }
        false
    }

    pub(super) fn holds(&self, iface: InterfaceId) -> bool {
        self.by_iface.contains_key(&iface)
    }

    /// The fewest-hops announce `iface` holds, the oldest among equals (`Interface.py` 285).
    pub(super) fn take_nearest(&mut self, iface: InterfaceId) -> Option<HeldAnnounce> {
        let queue = self.by_iface.get_mut(&iface)?;
        let (index, _) = queue
            .iter()
            .enumerate()
            .min_by_key(|(_, held)| held.packet.hops)?;
        let held = queue.remove(index);
        if queue.is_empty() {
            self.by_iface.remove(&iface);
        }
        self.total -= 1;
        Some(held)
    }

    /// Drop everything `iface` holds: it was detached, or its admission row evicted.
    pub(super) fn purge(&mut self, iface: InterfaceId) {
        if let Some(queue) = self.by_iface.remove(&iface) {
            self.total -= queue.len();
        }
    }
}

/// The freshness ledger and its host policy share one lock. Keeping this guard across address
/// admission, freshness commit, route replacement, observation publication, and relay
/// scheduling makes a held-release task indistinguishable from direct router ingress.
pub(super) struct AnnounceFreshnessState {
    policy: AnnounceFreshnessPolicy,
    table: AnnounceFreshness,
}

impl AnnounceFreshnessState {
    pub(super) fn new(
        policy: AnnounceFreshnessPolicy,
    ) -> Result<Self, AnnounceFreshnessConfigError> {
        Ok(Self {
            policy,
            table: AnnounceFreshness::new(policy.config())?,
        })
    }
}

impl Shared {
    pub(super) fn announce_admission_now_ms(&self) -> u64 {
        self.announce_admission_started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    /// Hold `held` until its interface calms, under that interface's `ingress` policy.
    pub(super) fn hold_announce(
        &self,
        held: HeldAnnounce,
        ingress: Option<AnnounceIngressPolicy>,
    ) -> bool {
        // RNS counts the receiving hop, so this is its `hops >= PATHFINDER_M - 1`: an
        // announce that could not be relayed on is not worth a slot.
        if held.packet.hops >= MAX_HOPS - 2 {
            return false;
        }
        let endpoint = self.announce_admission.lock().unwrap().policy();
        let capacity = ingress.unwrap_or(endpoint).held_capacity;
        let ceiling = endpoint.held_capacity.saturating_mul(HELD_SHARES);
        self.held_announces
            .lock()
            .unwrap()
            .hold(held, capacity, ceiling)
    }

    /// Whether freshness rejects `pkt` before it is verified, counting the rejection. The
    /// verified announce would carry the same destination and blob, so it would be rejected
    /// all the same; an acceptance is decided again, under the lock, once it verifies.
    /// A copy that may move its route to a higher-gravity interface is verified first, a
    /// cost only an interface with configured gravity pays.
    pub(super) fn announce_is_stale_unverified(&self, iface: InterfaceId, pkt: &Packet) -> bool {
        let Some(candidate) = crate::announce::unverified_candidate(pkt) else {
            return false;
        };
        let route_live = self.has_live_route(candidate.destination);
        let freshness = self.announce_freshness.lock().unwrap();
        let counter = match freshness.table.evaluate(candidate, route_live) {
            AnnounceFreshnessDecision::Accept(_) => return false,
            AnnounceFreshnessDecision::Reject(_)
                if self.gravity_repoints(&freshness.table, candidate, iface, pkt.hops) =>
            {
                return false;
            }
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay) => {
                &self.routing_stats.freshness_replays_rejected
            }
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase) => {
                &self.routing_stats.freshness_stale_rejected
            }
        };
        counter.fetch_add(1, Ordering::Relaxed);
        true
    }
}

impl Endpoint {
    /// Configure bounded announce ingress control for subsequently observed packets.
    /// Rate debt resets; retained interface counters and in-flight cooldowns survive.
    /// Rows are trimmed to capacity and release tasks are woken to reconsider deadlines.
    pub fn set_announce_ingress_policy(&self, policy: AnnounceIngressPolicy) {
        let evicted = self
            .shared
            .announce_admission
            .lock()
            .unwrap()
            .set_policy(policy);
        let mut held = self.shared.held_announces.lock().unwrap();
        evicted.into_iter().for_each(|iface| held.purge(iface));
        drop(held);
        self.shared.held_release_wake.notify_waiters();
    }

    /// The active announce-ingress policy.
    pub fn announce_ingress_policy(&self) -> AnnounceIngressPolicy {
        self.shared.announce_admission.lock().unwrap().policy()
    }

    /// Per-interface ingress accounting. This is carrier attribution, not an on-air receipt.
    pub fn announce_ingress_counters(&self, interface: InterfaceId) -> AnnounceIngressCounters {
        self.shared
            .announce_admission
            .lock()
            .unwrap()
            .counters(interface)
    }

    /// Replace the host receive-freshness policy without discarding retained replay state.
    ///
    /// Shrinking a bound trims the oldest rows or blobs and drops the routes of evicted rows,
    /// as counted in [`RoutingCounters`](super::RoutingCounters).
    pub fn set_announce_freshness_policy(
        &self,
        policy: AnnounceFreshnessPolicy,
    ) -> Result<(), AnnounceFreshnessConfigError> {
        let mut freshness = self.shared.announce_freshness.lock().unwrap();
        let changed = freshness.table.reconfigure(policy.config())?;
        freshness.policy = policy;
        self.shared
            .route_ttl_ms
            .store(policy.route_ttl_ticks(), Ordering::Relaxed);
        for destination in &changed.evicted_destinations {
            self.shared.forget_path(*destination);
        }
        self.shared
            .routing_stats
            .freshness_rows_evicted
            .fetch_add(changed.evicted_destinations.len() as u64, Ordering::Relaxed);
        self.shared
            .routing_stats
            .freshness_blobs_evicted
            .fetch_add(changed.evicted_blobs as u64, Ordering::Relaxed);
        Ok(())
    }

    /// The active host receive-freshness policy.
    pub fn announce_freshness_policy(&self) -> AnnounceFreshnessPolicy {
        self.shared.announce_freshness.lock().unwrap().policy
    }

    /// The next validated announce, for building a host peer-id to destination map.
    pub async fn next_announcement(&self) -> io::Result<PeerAnnounce> {
        recv_until_closed(&self.shared, &self.announce_rx).await
    }
}

/// Interface admission for a verified announce (`Transport.py` 1807-1825): one for an
/// unknown destination waits while its interface bursts, unless we asked for its path in the
/// last [`PATH_REQUEST_GATE`], so a path response is never stuck behind a flood.
pub(super) fn admit_verified_announce(
    shared: &Arc<Shared>,
    iface: InterfaceId,
    pkt: Packet,
    announce: Announce,
) {
    let destination = announce.destination;
    let known = shared.path_table.lock().unwrap().contains_key(&destination)
        || shared.path_requested_within(destination, PATH_REQUEST_GATE);
    let ingress = shared.iface_policy(iface).ingress;
    let now = shared.announce_admission_now_ms();
    let (verdict, evicted) = shared
        .announce_admission
        .lock()
        .unwrap()
        .observe_interface(iface, known, now, ingress);
    if let Some(evicted) = evicted {
        shared.held_announces.lock().unwrap().purge(evicted);
    }
    let InterfaceVerdict::Hold { release_at_ms } = verdict else {
        return process_verified_announce(shared, iface, pkt, announce);
    };
    let held = HeldAnnounce {
        interface: iface,
        packet: pkt,
        announce,
    };
    let stats = &shared.routing_stats;
    if shared.hold_announce(held, ingress) {
        shared.announce_admission.lock().unwrap().note_held(iface);
        stats.held_announces.fetch_add(1, Ordering::Relaxed);
        start_held_announce_release(shared, iface, release_at_ms);
    } else {
        let mut admission = shared.announce_admission.lock().unwrap();
        admission.note_held_dropped(iface);
        stats.held_announces_dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Release verified unknown-route announces one at a time after their ingress interface has
/// calmed, fewest hops first (`Interface.py` 278-297). The task is per interface, not per
/// packet, so a burst cannot turn into a timer storm. It is tracked with the endpoint's other
/// tasks and is aborted on close.
pub(super) fn start_held_announce_release(
    shared: &Arc<Shared>,
    iface: InterfaceId,
    first_due_ms: u64,
) {
    if !shared.held_release_tasks.lock().unwrap().insert(iface) {
        return;
    }
    let owner = Arc::clone(shared);
    if !track(shared, async move {
        let mut due_ms = first_due_ms;
        loop {
            let now_ms = owner.announce_admission_now_ms();
            if due_ms > now_ms {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(due_ms - now_ms)) => {}
                    _ = owner.held_release_wake.notified() => {}
                }
            }
            if !owner.is_running() {
                break;
            }

            let now_ms = owner.announce_admission_now_ms();
            if !owner.held_announces.lock().unwrap().holds(iface) {
                break;
            }
            let ingress = owner.iface_policy(iface).ingress;
            let Some(next_due_ms) = owner
                .announce_admission
                .lock()
                .unwrap()
                .release_due(iface, now_ms, ingress)
            else {
                // The bounded ledger evicted this interface (or policy cleared it).
                // Retire its deferred work; otherwise task restart would spin forever.
                owner.held_announces.lock().unwrap().purge(iface);
                break;
            };
            if next_due_ms > now_ms {
                due_ms = next_due_ms;
                continue;
            }

            let held = owner.held_announces.lock().unwrap().take_nearest(iface);
            let Some(held) = held else {
                due_ms = next_due_ms;
                continue;
            };
            owner
                .announce_admission
                .lock()
                .unwrap()
                .note_released(iface);
            process_verified_announce(&owner, iface, held.packet, held.announce);
            due_ms = next_due_ms;
        }

        owner.held_release_tasks.lock().unwrap().remove(&iface);
        let next_due_ms = owner.announce_admission_now_ms();
        if owner.held_announces.lock().unwrap().holds(iface) {
            start_held_announce_release(&owner, iface, next_due_ms);
        }
    }) {
        shared.held_release_tasks.lock().unwrap().remove(&iface);
    }
}

/// Continue a verified announce after interface admission. A destination-rate block keeps
/// learning and local publication intact, but stops the expensive mesh-wide rebroadcast.
pub(super) fn process_verified_announce(
    shared: &Arc<Shared>,
    iface: InterfaceId,
    pkt: Packet,
    announce: Announce,
) {
    // This guard spans every announce effect: held-release tasks run beside the packet loop,
    // and two candidates must not both pass and publish or relay out of freshness order.
    let mut freshness = shared.announce_freshness.lock().unwrap();
    // A known destination announced under another key is rejected outright, before it can
    // touch freshness, a route, or a relay (RNS `Identity.validate_announce`).
    if shared.address_book.lock().unwrap().key_conflicts(&announce) {
        shared
            .routing_stats
            .key_mismatch_announces
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    let candidate = AnnounceFreshnessCandidate {
        destination: announce.destination,
        blob: AnnounceBlob::from_wire(announce.rand_hash),
    };
    // Freshness belongs to the route. Without a live one this is a first sighting, as after
    // an RNS path cull, which is what lets a cached path response restore an expired route.
    let route_live = shared.has_live_route(announce.destination);
    let accepted = match freshness.table.evaluate(candidate, route_live) {
        AnnounceFreshnessDecision::Accept(accepted) => accepted,
        AnnounceFreshnessDecision::Reject(_)
            if shared.gravity_repoints(&freshness.table, candidate, iface, pkt.hops) =>
        {
            shared.learn_path(announce.destination, iface, pkt.hops, pkt.transport);
            return;
        }
        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay) => {
            shared
                .routing_stats
                .freshness_replays_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase) => {
            shared
                .routing_stats
                .freshness_stale_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    // A full book evicts the least recently heard peer with no live path or link. A refusal
    // only keeps the identity out of the book (no `PeerAnnounce`): the path is still learned
    // and the announce still relayed, as RNS relays from its path table.
    let now = super::known_destinations::book_clock_ms();
    let in_use = {
        let book = shared.address_book.lock().unwrap();
        book.is_full() && !book.knows(announce.destination)
    }
    .then(|| shared.destinations_in_use());
    let admitted = shared
        .address_book
        .lock()
        .unwrap()
        .ingest_at(&announce, now, |destination| {
            in_use
                .as_ref()
                .is_some_and(|in_use| in_use.contains(&destination))
        })
        != crate::address_book::Ingested::Refused;
    if !admitted {
        shared
            .routing_stats
            .refused_announces
            .fetch_add(1, Ordering::Relaxed);
    }
    let record = freshness.table.record_accepted(candidate, accepted);
    if let Some(evicted) = record.evicted_destination {
        // A route never outlives its freshness row.
        shared.forget_path(evicted);
        shared
            .routing_stats
            .freshness_rows_evicted
            .fetch_add(1, Ordering::Relaxed);
    }
    if record.evicted_blob.is_some() {
        shared
            .routing_stats
            .freshness_blobs_evicted
            .fetch_add(1, Ordering::Relaxed);
    }
    let destination = announce.destination;
    // A header-type-2 announce's transport node belongs to this destination's route, not to
    // the interface: one radio reaches different destinations through different nodes.
    shared.learn_path(destination, iface, pkt.hops, pkt.transport);
    if admitted {
        let sequence = shared.announce_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = shared.announce_tx.send(PeerAnnounce {
            destination,
            identity: announce.identity,
            app_data: announce.app_data,
            interface: iface,
            hops: pkt.hops,
            transport: pkt.transport,
            sequence,
        });
    }

    // A path response answers one requester: RNS learns from it but never rebroadcasts it or
    // rate-counts it, so one path request cannot flood the mesh.
    if pkt.context == crate::path::CTX_PATH_RESPONSE {
        return;
    }

    // RNS rate-counts every path-updating announce arriving on an interface with a rate rule,
    // relayed or not (`Transport.py` 2299-2333). An interface's own rule applies always; the
    // endpoint's only to a transport, as RNS sets `default_ar_*` (`Reticulum.py` 968-971).
    let policy = shared.routing.lock().unwrap().clone();
    let now = shared.announce_admission_now_ms();
    let rate_override = shared.iface_policy(iface).announce_rate;
    let mut admission = shared.announce_admission.lock().unwrap();
    let rate = rate_override.or_else(|| {
        policy
            .forward_announces
            .then(|| admission.policy().destination_rate())
            .flatten()
    });
    let blocked = rate.is_some_and(|rate| {
        admission.observe_destination(destination, rate, now) == DestinationVerdict::BlockRelay
    });
    drop(admission);

    // Relay as a transport node: hops+1, stamped with our identity so downstream peers
    // address replies through us, from the rebroadcast table.
    if !policy.relays_announce_from(iface) || !shared.announce_is_new(pkt.hash()) {
        return;
    }
    if blocked {
        shared
            .routing_stats
            .relay_rate_limited_announces
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    if pkt.hops.saturating_add(1) >= policy.max_hops {
        shared
            .routing_stats
            .hop_limit_dropped
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    let mut fwd = pkt;
    fwd.hops += 1;
    fwd.header_type = crate::packet::HeaderType::Type2;
    fwd.transport = Some(shared.identity.public().hash());
    shared.schedule_rebroadcast(iface, fwd, candidate.blob.timebase());
}
