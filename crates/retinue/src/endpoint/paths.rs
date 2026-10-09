//! The path table, path requests, and path responses.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::announce::AnnounceBlob;
use crate::announce_freshness::{AnnounceFreshness, AnnounceFreshnessCandidate};
use crate::hash::AddressHash;
use crate::node::InterfaceMode;
use crate::packet::Packet;

use super::announces::duration_ticks;
use super::dedup::SEEN_ANNOUNCES;
use super::entropy::fill_random;
use super::interface::InterfaceId;
use super::registration::host_announce_seconds;
use super::runtime::Endpoint;
use super::shared::Shared;

/// The most destinations a path table will hold, so strangers' announces cannot grow memory
/// without bound. Expired routes go first; then the oldest `learned`, which re-announces
/// refresh, so the quietest peer is evicted. Far above what a LoRa mesh sees.
#[cfg(not(test))]
pub(super) const PATH_TABLE_CAPACITY: usize = 4096;
#[cfg(test)]
pub(super) const PATH_TABLE_CAPACITY: usize = 4;

/// The least time between path requests we broadcast for one destination, so a peer sending
/// traffic we cannot verify cannot make us broadcast once per packet.
// Integration tests in dependent crates (outrider's, for one) build without cfg(test) and get
// the real 20-second floor: a second request for one destination there waits it out.
#[cfg(not(test))]
pub(super) const PATH_REQUEST_MIN_INTERVAL: Duration = Duration::from_secs(20);
#[cfg(test)]
pub(super) const PATH_REQUEST_MIN_INTERVAL: Duration = Duration::from_millis(60);

/// The most path requests broadcast in any [`PATH_REQUEST_MIN_INTERVAL`] window, across ALL
/// destinations: the peer provoking a request also names its destination, so fabricated
/// unique ones would never meet the per-destination floor. A refused request records nothing,
/// so this also bounds the budget table.
pub(super) const PATH_REQUEST_GLOBAL_MAX: usize = 8;

/// How long a path request we sent exempts its destination's announces from ingress holds
/// (RNS `PATH_REQUEST_GATE_TIMEOUT`, `Transport.py` 135, 980-986, 1815-1821).
pub(super) const PATH_REQUEST_GATE: Duration = Duration::from_secs(45);

/// A learned route to a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PathEntry {
    pub(super) iface: InterfaceId,
    /// The transport node this destination is reached through, from the header-type-2
    /// announce that taught us the route. Per destination, because one interface reaches
    /// different destinations through different nodes.
    pub(super) transport: Option<AddressHash>,
    pub(super) hops: u8,
    /// When this route was last learned from an announce, or last carried traffic. Routes
    /// past [`Self::lifetime`] are stale and evicted on lookup.
    pub(super) learned: Instant,
    /// The learning interface's mode when the route was learned.
    pub(super) mode: InterfaceMode,
}

impl PathEntry {
    /// How long this route lives, given the policy's full-mode route TTL.
    pub(super) fn lifetime(&self, route_ttl: Duration) -> Duration {
        Duration::from_millis(self.mode.route_ttl(duration_ticks(route_ttl)))
    }

    pub(super) fn live_at(&self, now: Instant, route_ttl: Duration) -> bool {
        now.saturating_duration_since(self.learned) < self.lifetime(route_ttl)
    }
}

impl Shared {
    /// Mark `dest`'s live route used now. RNS refreshes a path's timestamp whenever it
    /// carries a packet (`Transport.py` 1406, 1426, 2113), so a route in use does not lapse.
    pub(super) fn touch_path(&self, dest: AddressHash) {
        let now = Instant::now();
        let route_ttl = self.route_ttl();
        self.write_diagnostic(|| {
            let mut paths = self.path_table.lock().unwrap();
            match paths.get_mut(&dest) {
                Some(entry) if entry.live_at(now, route_ttl) => {
                    entry.learned = now;
                    ((), true)
                }
                _ => ((), false),
            }
        });
    }

    pub(super) fn route_ttl(&self) -> Duration {
        Duration::from_millis(self.route_ttl_ms.load(Ordering::Relaxed))
    }

    /// A path response for `target` if it is one of our registered destinations: an announce
    /// with context [`crate::path::CTX_PATH_RESPONSE`]. `None` otherwise, since we hold no
    /// announce cache to answer for others.
    pub(super) fn path_response(&self, target: AddressHash) -> Option<Packet> {
        self.path_response_at(target, host_announce_seconds())
    }

    /// The deterministic half of [`Self::path_response`], with the clock supplied.
    pub(super) fn path_response_at(
        &self,
        target: AddressHash,
        source_seconds: u64,
    ) -> Option<Packet> {
        let (name, app_data, source) = {
            let reg = self.registered.lock().unwrap();
            let r = reg.iter().find(|r| r.dest == target)?;
            (
                r.name.clone(),
                r.app_data.clone(),
                r.app_data_source.clone(),
            )
        };
        let app_data = source.map_or(app_data, |source| source(source_seconds));
        // A path response is an announce, so it rotates a due ratchet too (`Destination.py`
        // 285-288 runs for both).
        let ratchet = self.advertised_ratchet(target, source_seconds);
        let mut pkt = self.build_announce_at(&name, ratchet.as_ref(), &app_data, source_seconds);
        pkt.context = crate::path::CTX_PATH_RESPONSE;
        Some(pkt)
    }

    /// Whether `dest` has an unexpired route, without evicting anything.
    pub(super) fn has_live_route(&self, dest: AddressHash) -> bool {
        self.live_route(dest).is_some()
    }

    fn live_route(&self, dest: AddressHash) -> Option<PathEntry> {
        let route_ttl = self.route_ttl();
        let paths = self.path_table.lock().unwrap();
        paths
            .get(&dest)
            .filter(|entry| entry.live_at(Instant::now(), route_ttl))
            .copied()
    }

    /// Whether a copy that `table` refused should still move the route to `iface`: the same
    /// emission as the route's, with no more hops, heard on an interface of higher gravity
    /// (`Transport.py` 2229-2251). Only the route moves; the emission was already published
    /// and relayed when it first arrived.
    pub(super) fn gravity_repoints(
        &self,
        table: &AnnounceFreshness,
        candidate: AnnounceFreshnessCandidate,
        iface: InterfaceId,
        hops: u8,
    ) -> bool {
        let Some(route) = self.live_route(candidate.destination) else {
            return false;
        };
        if hops > route.hops
            || self.iface_policy(iface).gravity <= self.iface_policy(route.iface).gravity
        {
            return false;
        }
        // The refusal means the route's timebase is not older than this one; accepting the
        // next second means it is not newer either, so it is the same emission.
        AnnounceBlob::mint(candidate.blob.nonce(), candidate.timebase() + 1).is_ok_and(|blob| {
            let next = AnnounceFreshnessCandidate { blob, ..candidate };
            table.evaluate(next, true).is_accepted()
        })
    }

    /// Drop `dest`'s route, if any.
    pub(super) fn forget_path(&self, dest: AddressHash) {
        self.write_diagnostic(|| ((), self.path_table.lock().unwrap().remove(&dest).is_some()));
    }

    /// Record that `dest` is reachable via `iface` at `hops`.
    ///
    /// Freshness has already admitted this announce, so its route becomes the incumbent
    /// whatever its hop count: preferring the shortest here would let an older announce
    /// override the freshness ledger's decision.
    pub(super) fn learn_path(
        &self,
        dest: AddressHash,
        iface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
    ) {
        self.learn_path_at(dest, iface, hops, transport, Instant::now());
    }

    /// As [`Self::learn_path`], at a supplied instant, for tests independent of timing.
    pub(super) fn learn_path_at(
        &self,
        dest: AddressHash,
        iface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
        now: Instant,
    ) {
        let mode = self.interface_mode(iface);
        self.write_diagnostic(|| {
            let mut t = self.path_table.lock().unwrap();
            let route_ttl = self.route_ttl();
            if t.len() >= PATH_TABLE_CAPACITY && !t.contains_key(&dest) {
                // The dead first: a table full of expired routes must never evict a live one.
                t.retain(|_, e| e.live_at(now, route_ttl));
                // Every route is live, so the quietest peer loses.
                if t.len() >= PATH_TABLE_CAPACITY
                    && let Some(stalest) = t
                        .iter()
                        .min_by_key(|(_, e)| e.learned)
                        .map(|(dest, _)| *dest)
                {
                    t.remove(&stalest);
                    self.routing_stats
                        .paths_evicted
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            let next = PathEntry {
                iface,
                transport,
                hops,
                learned: now,
                mode,
            };
            let changed = t.get(&dest) != Some(&next);
            t.insert(dest, next);
            ((), changed)
        });
    }

    /// Destinations the address book must keep while it is full: those with an unexpired path
    /// or a live link. Each table is locked in turn, never two at once.
    pub(super) fn destinations_in_use(&self) -> HashSet<AddressHash> {
        let route_ttl = self.route_ttl();
        let now = Instant::now();
        let mut in_use: HashSet<AddressHash> = self
            .path_table
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, entry)| entry.live_at(now, route_ttl))
            .map(|(destination, _)| *destination)
            .collect();
        in_use.extend(
            self.links
                .lock()
                .unwrap()
                .values()
                .filter_map(|entry| entry.remote.destination),
        );
        in_use
    }

    /// The interface to reach `dest`, if a route is known and unexpired. Evicts an expired
    /// route as a side effect, so a stale path never lingers past a lookup.
    pub(super) fn path_iface(&self, dest: AddressHash) -> Option<InterfaceId> {
        let route_ttl = self.route_ttl();
        self.write_diagnostic(|| {
            let mut t = self.path_table.lock().unwrap();
            match t.get(&dest) {
                Some(e) if e.live_at(Instant::now(), route_ttl) => (Some(e.iface), false),
                Some(_) => {
                    t.remove(&dest);
                    (None, true)
                }
                None => (None, false),
            }
        })
    }

    /// Whether a path request for `dest` may go out now, under the per-destination floor and
    /// the global cap.
    ///
    /// Both checks pass before either records anything, so a request refused by one never
    /// spends the other's budget.
    fn path_request_within_budget(&self, dest: AddressHash) -> bool {
        let mut budget = self.path_request_budget.lock().unwrap();
        let mut stamps = self.path_request_stamps.lock().unwrap();
        let now = Instant::now();
        if let Some(&last) = budget.get(&dest)
            && now.duration_since(last) < PATH_REQUEST_MIN_INTERVAL
        {
            return false;
        }
        while let Some(&oldest) = stamps.front()
            && now.duration_since(oldest) >= PATH_REQUEST_MIN_INTERVAL
        {
            stamps.pop_front();
        }
        if stamps.len() >= PATH_REQUEST_GLOBAL_MAX {
            return false;
        }
        if budget.len() > SEEN_ANNOUNCES {
            let keep = PATH_REQUEST_MIN_INTERVAL.max(PATH_REQUEST_GATE);
            budget.retain(|_, t| now.duration_since(*t) < keep);
        }
        budget.insert(dest, now);
        stamps.push_back(now);
        true
    }

    /// Whether we sent a path request for `dest` within `window`.
    pub(super) fn path_requested_within(&self, dest: AddressHash, window: Duration) -> bool {
        let budget = self.path_request_budget.lock().unwrap();
        budget.get(&dest).is_some_and(|at| at.elapsed() < window)
    }
}

impl Endpoint {
    /// Broadcast a path request for `dest`. The path response is an announce, which fills the
    /// path table *and the address book*: use it when a route has gone stale, or to learn the
    /// identity of a source whose keys we lack.
    ///
    /// Rate-limited per destination and overall, silently. Returns whether a request went out.
    pub fn request_path(&self, dest: AddressHash) -> bool {
        if !self.shared.path_request_within_budget(dest) {
            return false;
        }
        let mut tag = [0u8; crate::path::TAG_LEN];
        fill_random(&mut tag);
        self.shared.broadcast(crate::path::path_request(dest, &tag));
        true
    }
}
