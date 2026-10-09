//! The path table, path requests, and path responses.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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

/// The most destinations a path table will hold.
///
/// The table was unbounded, which made it the last place a stranger could grow this
/// process's memory for free: every announce that survived the address book's cap put an
/// entry here and nothing ever took one out except expiry.
///
/// The eviction policy falls out of what feeds the table. Routes are learned from announces
/// and their time is refreshed by re-announces, so the entry with the oldest `learned` is
/// exactly the peer that has gone quietest, and the one whose route is least likely to still
/// be true. Evicting it costs a path request if that peer comes back; keeping it costs a
/// route we would have had to a peer that is still talking. Expired entries go first, so a
/// table full of the dead never evicts the living.
///
/// Sized for a transport node with a real neighbourhood rather than a bench: four thousand
/// destinations is far more than a LoRa mesh sees, and a bound that is never reached in
/// practice is the point.
#[cfg(not(test))]
pub(super) const PATH_TABLE_CAPACITY: usize = 4096;
#[cfg(test)]
pub(super) const PATH_TABLE_CAPACITY: usize = 4;

/// The least time between path requests we will broadcast for the same destination.
///
/// A path request is a broadcast, and the things that provoke one are usually inbound: a
/// message from somebody we cannot identify, a retry from a peer that has gone stale. Without
/// a floor, a peer sending traffic we cannot verify would make us broadcast once per packet,
/// which on a shared band is a stranger deciding how much of it we use.
// The test values keep the suite fast; note that integration tests in dependent crates
// (outrider's, for instance) compile this crate WITHOUT cfg(test) and therefore run against
// the real 20-second floor. A test over there that needs two requests for one destination
// will hang on the second, mysteriously, unless it knows this.
#[cfg(not(test))]
pub(super) const PATH_REQUEST_MIN_INTERVAL: Duration = Duration::from_secs(20);
#[cfg(test)]
pub(super) const PATH_REQUEST_MIN_INTERVAL: Duration = Duration::from_millis(60);

/// The most path requests that may be broadcast in any [`PATH_REQUEST_MIN_INTERVAL`] window,
/// across ALL destinations.
///
/// The per-destination floor alone is not a rate limit, because the peer that provokes a path
/// request also chooses the destination: a flood of unverifiable packets with fabricated,
/// unique sources would get one broadcast each, and the floor would never engage since no key
/// repeats. This cap bounds the aggregate, and — because a refused request records nothing —
/// it also bounds how fast the budget table can be made to grow.
pub(super) const PATH_REQUEST_GLOBAL_MAX: usize = 8;

/// A learned route to a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PathEntry {
    pub(super) iface: InterfaceId,
    /// The transport node this destination is reached through, from the `transport` field of
    /// the header-type-2 announce that taught us the route. Per destination, because an
    /// interface can reach many destinations through different nodes: one radio hearing A
    /// via X and B via Y is the ordinary case, not an exotic one.
    pub(super) transport: Option<AddressHash>,
    pub(super) hops: u8,
    /// When this route was last (re)learned from an announce, or last carried traffic. Routes
    /// older than their lifetime ([`Self::lifetime`]) are treated as stale and evicted on lookup.
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

    /// Build a path response for `target` if it is one of our registered destinations: an
    /// announce for it carrying context [`crate::path::CTX_PATH_RESPONSE`]. Returns `None` if
    /// we do not own `target` — we hold no announce cache, so we cannot answer for others and
    /// stay silent rather than guess.
    pub(super) fn path_response(&self, target: AddressHash) -> Option<Packet> {
        self.path_response_at(target, host_announce_seconds())
    }

    /// The deterministic half of [`Self::path_response`]. Keeping the clock source at this
    /// seam lets the production path and its boundary cases share the same blob minting rule.
    pub(super) fn path_response_at(
        &self,
        target: AddressHash,
        source_seconds: u64,
    ) -> Option<Packet> {
        let (name, app_data) = {
            let reg = self.registered.lock().unwrap();
            let r = reg.iter().find(|r| r.dest == target)?;
            (r.name.clone(), r.app_data.clone())
        };
        // A path response is an announce, so it rotates a due ratchet too (`Destination.py`
        // 285-288 runs for both).
        let ratchet = self.advertised_ratchet(target, source_seconds);
        let mut pkt = self.build_announce_at(&name, ratchet.as_ref(), &app_data, source_seconds);
        pkt.context = crate::path::CTX_PATH_RESPONSE;
        Some(pkt)
    }

    /// Whether `dest` has an unexpired route, without evicting anything.
    pub(super) fn has_live_route(&self, dest: AddressHash) -> bool {
        let route_ttl = self.route_ttl();
        self.path_table
            .lock()
            .unwrap()
            .get(&dest)
            .is_some_and(|entry| entry.live_at(Instant::now(), route_ttl))
    }

    /// Drop `dest`'s route, if any.
    pub(super) fn forget_path(&self, dest: AddressHash) {
        self.write_diagnostic(|| ((), self.path_table.lock().unwrap().remove(&dest).is_some()));
    }

    /// Record that `dest` is reachable via `iface` at `hops`.
    ///
    /// Freshness admission has already admitted this announce. Its route is therefore the
    /// incumbent, irrespective of whether its hop count is better, equal, or worse than a
    /// formerly live route. Selecting the shortest live route here would let an
    /// older announce override the newer route decision made by the freshness ledger.
    pub(super) fn learn_path(
        &self,
        dest: AddressHash,
        iface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
    ) {
        self.learn_path_at(dest, iface, hops, transport, Instant::now());
    }

    /// As [`Self::learn_path`], at a supplied monotonic instant. This keeps route-capacity and
    /// expiry tests independent of scheduler timing.
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
                // Still full means every route is live, so the quietest peer loses. Its
                // `learned` is oldest precisely because it has stopped re-announcing.
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

    /// Whether a path request for `dest` may go out now. Same shape and same reasoning as
    /// announce-admission destination ledger on the outbound side, plus the global cap: the
    /// per-destination floor cannot be the whole answer, because the peer that provokes a
    /// path request also chooses the destination it names.
    ///
    /// Ordering is load-bearing: both checks pass before either records anything, so a
    /// request refused by the global cap does not burn the destination's own budget, and a
    /// per-destination repeat does not spend a slot in the global window.
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
            budget.retain(|_, t| now.duration_since(*t) < PATH_REQUEST_MIN_INTERVAL);
        }
        budget.insert(dest, now);
        stamps.push_back(now);
        true
    }
}

impl Endpoint {
    /// Broadcast a path request for `dest`, asking the network to make it reachable. The
    /// matching path response is an announce, ingested like any other, which populates the
    /// path table *and the address book*, so this is also how an identity is learned for a
    /// destination that has only ever been named to us. Use when a route has gone stale so a
    /// subsequent link setup has an interface to go out on, or when a message arrives from a
    /// source whose keys we do not have.
    ///
    /// Rate-limited per destination (see `PATH_REQUEST_MIN_INTERVAL`) and silent when the
    /// request is suppressed, because callers ask on someone else's schedule. Returns whether
    /// a request actually went out, for tests and diagnostics.
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
