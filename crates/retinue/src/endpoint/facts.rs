//! Read-only observations of routes, links and announces.

use alloc::vec::Vec;

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::hash::AddressHash;
use crate::identity::Identity;

use super::interface::InterfaceId;
use super::runtime::Endpoint;

/// A validated announce observation, surfaced without exposing the endpoint's mutable
/// address book or path table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceFact {
    /// The destination hash announced.
    pub destination: AddressHash,
    /// The announcing identity.
    pub identity: Identity,
    /// The app data the announce carried (a host binds its own peer id here).
    pub app_data: Vec<u8>,
    /// The interface on which this observation arrived.
    pub interface: InterfaceId,
    /// Hop count carried by this observation.
    pub hops: u8,
    /// Transport node named by a header-type-2 observation, when present.
    pub transport: Option<AddressHash>,
    /// Endpoint-local monotonic observation order.
    pub sequence: u64,
}

/// Compatibility name for consumers that already treat an announce as a peer record.
pub type PeerAnnounce = AnnounceFact;

/// A current learned route captured at one caller-supplied instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteFact {
    pub destination: AddressHash,
    pub interface: InterfaceId,
    pub transport: Option<AddressHash>,
    pub hops: u8,
    pub age: Duration,
}

/// Which side initiated a live link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkDirection {
    Inbound,
    Outbound,
}

/// The endpoint discipline currently driving a live link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkFactKind {
    BestEffort,
    Reliable,
    Resource,
}

/// Remote facts authenticated by link setup or a later IDENTIFY.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkRemoteFact {
    /// The remote application destination, known for an outbound link request.
    pub destination: Option<AddressHash>,
    /// The remote public identity, known outbound and after a valid inbound IDENTIFY.
    pub identity: Option<Identity>,
}

/// A read-only live-link observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkFact {
    pub id: AddressHash,
    pub interface: InterfaceId,
    pub kind: LinkFactKind,
    pub direction: LinkDirection,
    pub remote: LinkRemoteFact,
}

/// Route and link facts captured against one stable interface-id set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointFacts {
    /// Endpoint topology revision stable across this fact capture.
    pub generation: u64,
    pub interfaces: Vec<InterfaceId>,
    pub routes: Vec<RouteFact>,
    pub links: Vec<LinkFact>,
    /// Routes omitted because their captured age reached the route lifetime.
    pub expired_routes: u64,
}

impl Endpoint {
    /// The interface a learned destination is reachable over, and its hop count. An expired
    /// route is not returned (and is evicted).
    pub fn route_to(&self, dest: AddressHash) -> Option<(InterfaceId, u8)> {
        self.route_to_at(dest, Instant::now())
    }

    /// As [`Self::route_to`], against a supplied monotonic instant. Kept private because a
    /// host captures route observations through [`Self::route_facts_at`], while endpoint tests
    /// need deterministic expiry without sleeping.
    pub(super) fn route_to_at(&self, dest: AddressHash, now: Instant) -> Option<(InterfaceId, u8)> {
        let route_ttl = self.shared.route_ttl();
        self.shared.write_diagnostic(|| {
            let mut t = self.shared.path_table.lock().unwrap();
            match t.get(&dest) {
                Some(e) if e.live_at(now, route_ttl) => (Some((e.iface, e.hops)), false),
                Some(_) => {
                    t.remove(&dest);
                    (None, true)
                }
                None => (None, false),
            }
        })
    }

    /// Current routes in deterministic destination order, aged against one instant supplied
    /// by the caller. Unlike [`route_to`](Self::route_to), observation never evicts state.
    pub fn route_facts_at(&self, captured_at: Instant) -> Vec<RouteFact> {
        let interfaces = self.interface_ids();
        self.route_facts_for_interfaces_at(captured_at, &interfaces)
            .0
    }

    fn route_facts_for_interfaces_at(
        &self,
        captured_at: Instant,
        interfaces: &[InterfaceId],
    ) -> (Vec<RouteFact>, u64) {
        let interfaces: HashSet<_> = interfaces.iter().copied().collect();
        let route_ttl = self.shared.route_ttl();
        let table = self.shared.path_table.lock().unwrap();
        let mut expired_routes = 0_u64;
        let mut facts = Vec::new();
        for (destination, entry) in table.iter() {
            if !interfaces.contains(&entry.iface) {
                continue;
            }
            let age = captured_at
                .checked_duration_since(entry.learned)
                .unwrap_or_default();
            if age >= entry.lifetime(route_ttl) {
                expired_routes = expired_routes.saturating_add(1);
                continue;
            }
            facts.push(RouteFact {
                destination: *destination,
                interface: entry.iface,
                transport: entry.transport,
                hops: entry.hops,
                age,
            });
        }
        facts.sort_unstable_by_key(|fact| fact.destination);
        (facts, expired_routes)
    }

    /// Live links in deterministic id order. Entries whose carrier was detached are omitted,
    /// because a snapshot must never name an interface absent from the same capture.
    pub fn link_facts(&self) -> Vec<LinkFact> {
        let interfaces = self.interface_ids();
        self.link_facts_for_interfaces(&interfaces)
    }

    fn link_facts_for_interfaces(&self, interfaces: &[InterfaceId]) -> Vec<LinkFact> {
        let interfaces: HashSet<_> = interfaces.iter().copied().collect();
        let links = self.shared.links.lock().unwrap();
        let mut facts: Vec<_> = links
            .iter()
            .filter_map(|(id, entry)| {
                interfaces.contains(&entry.iface).then_some(LinkFact {
                    id: *id,
                    interface: entry.iface,
                    kind: entry.kind.fact_kind(),
                    direction: entry.direction,
                    remote: entry.remote,
                })
            })
            .collect();
        facts.sort_unstable_by_key(|fact| fact.id);
        facts
    }

    /// Capture interface, route, and link facts with referential integrity. All route and
    /// link interface ids occur in the returned `interfaces` list.
    pub fn diagnostic_facts_at(&self, captured_at: Instant) -> EndpointFacts {
        let (generation, (interfaces, routes, links, expired_routes)) =
            self.shared.capture_diagnostic(|| {
                let interfaces = self.interface_ids();
                let (routes, expired_routes) =
                    self.route_facts_for_interfaces_at(captured_at, &interfaces);
                let links = self.link_facts_for_interfaces(&interfaces);
                (interfaces, routes, links, expired_routes)
            });
        EndpointFacts {
            generation,
            routes,
            links,
            interfaces,
            expired_routes,
        }
    }

    /// Monotonic source revision for interface, route, link, and announce facts.
    /// Point-in-time ages and traffic counters are sampled values, not revision sources.
    pub fn diagnostic_generation(&self) -> u64 {
        self.shared.diagnostic_generation.load(Ordering::Acquire)
    }
}
