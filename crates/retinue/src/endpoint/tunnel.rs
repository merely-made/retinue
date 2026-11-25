//! Tunnel synthesis: a reconnecting client's paths are kept for it by the node it rejoins
//! (`TCPInterface.py` 169-180, 298-299; `Transport.py` 2463-2473, 2764-2860).
//!
//! A client signs `public key || interface hash || random` with its transport identity; the
//! receiving node keys a tunnel by `full_hash(public key || interface hash)`, mirrors every
//! route learned over that interface into it, and restores them onto whichever interface the
//! same tunnel next appears on.

use alloc::vec::Vec;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use crate::announce::{ANNOUNCE_NONCE_LEN, ANNOUNCE_TIMEBASE_MAX, AnnounceBlob};
use crate::announce_freshness::{
    AnnounceFreshness, AnnounceFreshnessAccept, AnnounceFreshnessCandidate,
    AnnounceFreshnessDecision,
};
use crate::destination::DestinationName;
use crate::hash::{AddressHash, HASH_LEN, full_hash};
use crate::identity::{IDENTITY_LEN, Identity, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};

use super::entropy::fill_random;
use super::interface::InterfaceId;
use super::paths::{PATH_TABLE_CAPACITY, PathEntry, make_room_for_path};
use super::shared::Shared;

/// A tunnel, and each path mirrored into it, lives this long unrefreshed (`TUNNEL_TIMEOUT`,
/// `TUNNEL_PATH_TIMEOUT`, `Transport.py` 157-158).
pub(super) const TUNNEL_TIMEOUT: Duration = Duration::from_secs(8 * 60 * 60);

/// Tunnels kept at once. RNS keeps every one; a signed synthesis costs only a fresh identity,
/// so this is bounded. Away tunnels make room first: an interface binds at most one tunnel,
/// so one peer minting identities cannot evict the tunnels of clients still connected.
const TUNNEL_CAPACITY: usize = 256;

/// Paths kept across all tunnels; the oldest mirrored makes room.
const TUNNEL_PATH_CAPACITY: usize = PATH_TABLE_CAPACITY;

/// Random bytes in a synthesis (`RNS.Reticulum.TRUNCATED_HASHLENGTH`).
const RANDOM_LEN: usize = 16;

/// A synthesis payload: key, interface hash, random, signature over the first three: 176
/// bytes (`Transport.py` 2788-2800).
pub(super) const SYNTHESIS_LEN: usize = IDENTITY_LEN + HASH_LEN + RANDOM_LEN + SIGNATURE_LEN;

type TunnelId = [u8; HASH_LEN];

#[derive(Debug)]
struct Tunnel {
    /// The interface the tunnel is on now; `None` while its client is away.
    iface: Option<InterfaceId>,
    paths: HashMap<AddressHash, Mirrored>,
    expires: Instant,
}

#[derive(Clone, Copy, Debug)]
struct Mirrored {
    entry: PathEntry,
    at: Instant,
    /// The route's announce timebase, taken when the tunnel goes away; `None` while it is
    /// bound, when the freshness row is still this route's own.
    timebase: Option<u64>,
}

/// Tunnels this node keeps for its clients, by tunnel id.
#[derive(Debug)]
pub(super) struct Tunnels {
    /// `PLAIN rnstransport.tunnel.synthesize`.
    destination: AddressHash,
    table: Mutex<HashMap<TunnelId, Tunnel>>,
}

impl Default for Tunnels {
    fn default() -> Self {
        Self {
            destination: synthesis_destination(),
            table: Mutex::default(),
        }
    }
}

/// The plain destination a synthesis is addressed to (`Transport.py` 362).
pub(super) fn synthesis_destination() -> AddressHash {
    DestinationName::new("rnstransport", ["tunnel", "synthesize"]).plain_hash()
}

/// A synthesis for the interface named `label`, signed by `identity` (`Transport.py`
/// 2764-2784). The tunnel id is stable for as long as the identity and label are.
pub(super) fn synthesis(identity: &PrivateIdentity, label: &str) -> Packet {
    let mut payload = Vec::with_capacity(SYNTHESIS_LEN);
    payload.extend_from_slice(&identity.public().to_public_bytes());
    payload.extend_from_slice(&full_hash(label.as_bytes()));
    let mut random = [0u8; RANDOM_LEN];
    fill_random(&mut random);
    payload.extend_from_slice(&random);
    let signature = identity.sign(&payload);
    payload.extend_from_slice(&signature);
    Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Plain,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: synthesis_destination(),
        context: 0,
        payload,
    }
}

/// The tunnel id a valid synthesis payload names, or `None` if its length or signature is
/// wrong (`Transport.py` 2788-2805).
fn verify(payload: &[u8]) -> Option<TunnelId> {
    if payload.len() != SYNTHESIS_LEN {
        return None;
    }
    let (signed, signature) = payload.split_at(SYNTHESIS_LEN - SIGNATURE_LEN);
    let key: &[u8; IDENTITY_LEN] = signed[..IDENTITY_LEN].try_into().ok()?;
    let signature: &[u8; SIGNATURE_LEN] = signature.try_into().ok()?;
    Identity::from_public_bytes(key)
        .ok()?
        .verify(signed, signature)
        .then(|| full_hash(&signed[..IDENTITY_LEN + HASH_LEN]))
}

/// The newest timebase `table` has accepted for `dest`, or `None` without a row. The table
/// answers only comparisons (a candidate is accepted iff newer), so bisect them.
fn row_timebase(table: &AnnounceFreshness, dest: AddressHash) -> Option<u64> {
    let accepts = |timebase| {
        let blob = AnnounceBlob::mint([0; ANNOUNCE_NONCE_LEN], timebase).expect("in range");
        let candidate = AnnounceFreshnessCandidate {
            destination: dest,
            blob,
        };
        table.evaluate(candidate, true)
    };
    let first = AnnounceFreshnessDecision::Accept(AnnounceFreshnessAccept::FirstSighting);
    if accepts(0) == first {
        return None;
    }
    // Invariant: `lo` is not accepted, everything from `hi` up is.
    let (mut lo, mut hi) = (0, ANNOUNCE_TIMEBASE_MAX + 1);
    while hi - lo > 1 {
        let mid = lo + (hi - lo) / 2;
        if accepts(mid).is_accepted() {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(lo)
}

impl Tunnel {
    /// The client is away (`void_tunnel_interface`, `Transport.py` 2813-2817). Each mirrored
    /// route's timebase is taken now, while its freshness row is still its own; one without a
    /// row is dropped, since a route never outlives its row.
    fn void(&mut self, freshness: &AnnounceFreshness) {
        if self.iface.take().is_none() {
            return;
        }
        self.paths.retain(|dest, path| {
            path.timebase = row_timebase(freshness, *dest);
            path.timebase.is_some()
        });
    }
}

impl Tunnels {
    fn cull(table: &mut HashMap<TunnelId, Tunnel>, now: Instant) {
        table.retain(|_, tunnel| {
            tunnel
                .paths
                .retain(|_, path| now.saturating_duration_since(path.at) < TUNNEL_TIMEOUT);
            now < tunnel.expires
        });
    }

    /// Make room for one more tunnel: the away one expiring soonest, else any.
    fn make_room(table: &mut HashMap<TunnelId, Tunnel>) {
        if table.len() < TUNNEL_CAPACITY {
            return;
        }
        let victim = table
            .iter()
            .min_by_key(|(_, t)| (t.iface.is_some(), t.expires))
            .map(|(id, _)| *id);
        if let Some(victim) = victim {
            table.remove(&victim);
        }
    }

    /// Drop the oldest mirrored path across all tunnels once they hold the cap.
    fn make_path_room(table: &mut HashMap<TunnelId, Tunnel>) {
        let held: usize = table.values().map(|t| t.paths.len()).sum();
        if held < TUNNEL_PATH_CAPACITY {
            return;
        }
        let oldest = table
            .iter()
            .flat_map(|(id, t)| t.paths.iter().map(move |(dest, p)| (p.at, *id, *dest)))
            .min_by_key(|(at, ..)| *at);
        if let Some((_, id, dest)) = oldest
            && let Some(tunnel) = table.get_mut(&id)
        {
            tunnel.paths.remove(&dest);
        }
    }
}

impl Shared {
    /// Handle `pkt` if it is a tunnel synthesis; true if it was one and is consumed. One that
    /// fails validation counts as a protocol violation on `iface`.
    pub(super) fn tunnel_intercept(&self, iface: InterfaceId, pkt: &Packet) -> bool {
        if pkt.packet_type != PacketType::Data || pkt.destination != self.tunnels.destination {
            return false;
        }
        match verify(&pkt.payload) {
            Some(id) => self.handle_tunnel(id, iface, Instant::now()),
            None => {
                self.with_iface(iface, |i| i.stats.note_protocol_violation());
            }
        }
        true
    }

    /// Create tunnel `id` on `iface`, or re-point it there and restore its paths
    /// (`Transport.py` 2820-2860). A path is restored over a current route with more hops,
    /// or an expired one, if its timebase is not older (2846-2850); with no current route, if
    /// it is unexpired and its destination still has a freshness row. Restores obey the
    /// path table's capacity. Lock order: freshness, tunnels, then the path table.
    fn handle_tunnel(&self, id: TunnelId, iface: InterfaceId, now: Instant) {
        let freshness = self.announce_freshness.lock().unwrap();
        let mut table = self.tunnels.table.lock().unwrap();
        Tunnels::cull(&mut table, now);
        if let Some(tunnel) = table.get_mut(&id).filter(|t| t.iface == Some(iface)) {
            tunnel.expires = now + TUNNEL_TIMEOUT;
            return;
        }
        for tunnel in table.values_mut() {
            if tunnel.iface == Some(iface) {
                tunnel.void(&freshness.table);
            }
        }
        if !table.contains_key(&id) {
            Tunnels::make_room(&mut table);
        }
        let tunnel = table.entry(id).or_insert_with(|| Tunnel {
            iface: None,
            paths: HashMap::new(),
            expires: now,
        });
        tunnel.void(&freshness.table);
        tunnel.iface = Some(iface);
        tunnel.expires = now + TUNNEL_TIMEOUT;
        if tunnel.paths.is_empty() {
            return;
        }
        let life = self.path_life();
        let mode = self.interface_mode(iface);
        let (restored, evicted) = self.write_diagnostic(|| {
            let mut paths = self.path_table.lock().unwrap();
            let (mut restored, mut evicted) = (0u64, 0u64);
            tunnel.paths.retain(|dest, mirrored| {
                let not_older = || {
                    let current = row_timebase(&freshness.table, *dest);
                    current.is_some_and(|current| mirrored.timebase >= Some(current))
                };
                let keep = match paths.get(dest) {
                    Some(current) => {
                        (mirrored.entry.hops <= current.hops || !current.live_at(now, life))
                            && not_older()
                    }
                    None => mirrored.entry.live_at(now, life) && not_older(),
                };
                if !keep {
                    return false;
                }
                if !paths.contains_key(dest) && make_room_for_path(&mut paths, now, life) {
                    evicted += 1;
                }
                let entry = PathEntry {
                    iface,
                    mode,
                    ..mirrored.entry
                };
                paths.insert(*dest, entry);
                mirrored.timebase = None;
                restored += 1;
                true
            });
            ((restored, evicted), restored > 0)
        });
        let stats = &self.routing_stats;
        stats
            .tunnel_paths_restored
            .fetch_add(restored, Ordering::Relaxed);
        stats.paths_evicted.fetch_add(evicted, Ordering::Relaxed);
    }

    /// `dest`'s route was just learned or replaced as `entry`. Called outside the path-table
    /// lock. A route on a tunnel's interface is mirrored into it and refreshes it
    /// (`Transport.py` 2463-2473). One elsewhere supersedes a bound tunnel's copy, since
    /// freshness admitted it over that copy's route; an away tunnel's copy waits for the
    /// timebase test on restore.
    pub(super) fn tunnel_path_learned(&self, dest: AddressHash, entry: &PathEntry) {
        let now = Instant::now();
        let mut table = self.tunnels.table.lock().unwrap();
        if table.is_empty() {
            return;
        }
        let mut owner = None;
        for (id, tunnel) in table.iter_mut() {
            match tunnel.iface {
                Some(bound) if bound == entry.iface => owner = Some(*id),
                Some(_) => {
                    tunnel.paths.remove(&dest);
                }
                None => {}
            }
        }
        let Some(owner) = owner else { return };
        if !table[&owner].paths.contains_key(&dest) {
            Tunnels::make_path_room(&mut table);
        }
        if let Some(tunnel) = table.get_mut(&owner) {
            let mirrored = Mirrored {
                entry: *entry,
                at: now,
                timebase: None,
            };
            tunnel.paths.insert(dest, mirrored);
            tunnel.expires = now + TUNNEL_TIMEOUT;
        }
    }

    /// Interface `id` is gone; its routes have been culled (`void_tunnel_interface`,
    /// `Transport.py` 2813-2817). Lock order: freshness, then tunnels.
    pub(super) fn tunnel_interface_lost(&self, id: InterfaceId) {
        let freshness = self.announce_freshness.lock().unwrap();
        for tunnel in self.tunnels.table.lock().unwrap().values_mut() {
            if tunnel.iface == Some(id) {
                tunnel.void(&freshness.table);
            }
        }
    }

    /// Forget the tunnels on interface `id` (`Transport.py` 3703).
    pub(super) fn drop_tunnels_on(&self, id: InterfaceId) {
        self.tunnels
            .table
            .lock()
            .unwrap()
            .retain(|_, tunnel| tunnel.iface != Some(id));
    }

    #[cfg(test)]
    pub(super) fn tunnel_count(&self) -> usize {
        self.tunnels.table.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endpoint::Endpoint;
    use crate::endpoint::router::route;

    fn endpoint() -> Endpoint {
        Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[9; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        )
    }

    fn dest(byte: u8) -> AddressHash {
        AddressHash::from_bytes([byte; 16])
    }

    #[test]
    fn synthesis_is_176_signed_bytes_naming_a_stable_tunnel() {
        let client = PrivateIdentity::from_secret_bytes(&[1; 64]);
        let (a, b) = (synthesis(&client, "x"), synthesis(&client, "x"));
        assert_eq!(a.payload.len(), 176);
        assert_ne!(a.payload, b.payload, "fresh random bytes each time");
        let mut id_data = client.public().to_public_bytes().to_vec();
        id_data.extend_from_slice(&full_hash(b"x"));
        assert_eq!(verify(&a.payload), Some(full_hash(&id_data)));
        assert_eq!(verify(&a.payload), verify(&b.payload));
        let mut forged = a.payload.clone();
        forged[100] ^= 1;
        assert_eq!(verify(&forged), None);
        assert_eq!(verify(&a.payload[..175]), None);
    }

    #[tokio::test]
    async fn invalid_synthesis_is_consumed_as_a_protocol_violation() {
        let ep = endpoint();
        let iface = ep.attach_interface().id();
        let mut pkt = synthesis(&PrivateIdentity::from_secret_bytes(&[1; 64]), "x");
        pkt.payload.pop();
        assert!(ep.shared.tunnel_intercept(iface, &pkt));
        assert_eq!(ep.shared.tunnel_count(), 0);
        assert_eq!(ep.interface_counters(iface).unwrap().protocol_violations, 1);
    }

    /// Learn a route as an announce would: freshness first, under its lock.
    fn learn(ep: &Endpoint, d: u8, iface: InterfaceId, hops: u8, timebase: u64) {
        let candidate = AnnounceFreshnessCandidate {
            destination: dest(d),
            blob: AnnounceBlob::mint([d; ANNOUNCE_NONCE_LEN], timebase).unwrap(),
        };
        let mut freshness = ep.shared.announce_freshness.lock().unwrap();
        let live = ep.shared.has_live_route(dest(d));
        let AnnounceFreshnessDecision::Accept(accepted) = freshness.table.evaluate(candidate, live)
        else {
            panic!("refused");
        };
        freshness.table.record_accepted(candidate, accepted);
        ep.shared.learn_path(dest(d), iface, hops, None);
    }

    fn mirrored(hops: u8, at: Instant) -> Mirrored {
        let entry = PathEntry {
            iface: 0,
            transport: None,
            hops,
            learned: at,
            mode: crate::node::InterfaceMode::Full,
            used: None,
        };
        Mirrored {
            entry,
            at,
            timebase: None,
        }
    }

    #[tokio::test]
    async fn row_timebase_reads_the_newest_accepted() {
        let ep = endpoint();
        learn(&ep, 1, 0, 0, 1234);
        learn(&ep, 2, 0, 0, ANNOUNCE_TIMEBASE_MAX);
        let freshness = ep.shared.announce_freshness.lock().unwrap();
        assert_eq!(row_timebase(&freshness.table, dest(1)), Some(1234));
        let max = Some(ANNOUNCE_TIMEBASE_MAX);
        assert_eq!(row_timebase(&freshness.table, dest(2)), max);
        assert_eq!(row_timebase(&freshness.table, dest(3)), None);
    }

    /// A client's routes survive its reconnect onto a new interface, with no new announce.
    #[tokio::test]
    async fn reappearing_tunnel_restores_its_paths() {
        let ep = endpoint();
        let client = PrivateIdentity::from_secret_bytes(&[1; 64]);
        let first = ep.attach_interface().id();
        route(&ep.shared, first, synthesis(&client, "c"));
        assert_eq!(ep.shared.tunnel_count(), 1);
        learn(&ep, 1, first, 2, 10);
        ep.detach_interface(first);
        assert!(ep.route_to(dest(1)).is_none());

        let second = ep.attach_interface().id();
        route(&ep.shared, second, synthesis(&client, "c"));
        assert_eq!(ep.route_to(dest(1)), Some((second, 2)));
        assert_eq!(ep.routing_counters().tunnel_paths_restored, 1);
    }

    /// The hops and timebase test of `Transport.py` 2836-2850, and the freshness row rule.
    #[tokio::test]
    async fn restore_needs_fewer_hops_and_a_timebase_not_older() {
        let ep = endpoint();
        let client = PrivateIdentity::from_secret_bytes(&[1; 64]);
        let (tunnel, other) = (ep.attach_interface().id(), ep.attach_interface().id());
        route(&ep.shared, tunnel, synthesis(&client, "c"));
        for d in 1..=3 {
            learn(&ep, d, tunnel, 3, 100);
        }
        ep.detach_interface(tunnel);
        // A shorter route at the same emission stays.
        learn(&ep, 1, other, 1, 100);
        // A newer emission with more hops still wins.
        learn(&ep, 2, other, 5, 101);
        // An older emission heard in the gap, with more hops, is replaced.
        learn(&ep, 3, other, 5, 99);
        let back = ep.attach_interface().id();
        route(&ep.shared, back, synthesis(&client, "c"));
        assert_eq!(ep.route_to(dest(1)), Some((other, 1)));
        assert_eq!(ep.route_to(dest(2)), Some((other, 5)));
        assert_eq!(ep.route_to(dest(3)), Some((back, 3)));
        assert_eq!(ep.routing_counters().tunnel_paths_restored, 1);
    }

    #[tokio::test]
    async fn a_route_without_a_freshness_row_is_not_restored() {
        let ep = endpoint();
        let client = PrivateIdentity::from_secret_bytes(&[1; 64]);
        let first = ep.attach_interface().id();
        route(&ep.shared, first, synthesis(&client, "c"));
        ep.shared.learn_path(dest(1), first, 2, None);
        ep.detach_interface(first);
        let second = ep.attach_interface().id();
        route(&ep.shared, second, synthesis(&client, "c"));
        assert!(ep.route_to(dest(1)).is_none());
    }

    #[tokio::test]
    async fn restores_obey_the_path_table_capacity() {
        let ep = endpoint();
        let client = PrivateIdentity::from_secret_bytes(&[1; 64]);
        let (first, other) = (ep.attach_interface().id(), ep.attach_interface().id());
        route(&ep.shared, first, synthesis(&client, "c"));
        learn(&ep, 0, first, 1, 10);
        ep.detach_interface(first);
        for d in 1..=PATH_TABLE_CAPACITY as u8 {
            learn(&ep, d, other, 1, 10);
        }
        let second = ep.attach_interface().id();
        route(&ep.shared, second, synthesis(&client, "c"));
        assert_eq!(ep.route_to(dest(0)), Some((second, 1)));
        assert_eq!(
            ep.shared.path_table.lock().unwrap().len(),
            PATH_TABLE_CAPACITY
        );
        assert_eq!(ep.routing_counters().paths_evicted, 1);
    }

    #[test]
    fn away_tunnels_make_room_first() {
        let now = Instant::now();
        let mut table = HashMap::new();
        for n in 0..TUNNEL_CAPACITY {
            let tunnel = Tunnel {
                iface: (n > 0).then_some(n as InterfaceId),
                paths: HashMap::new(),
                expires: now + Duration::from_secs(TUNNEL_CAPACITY as u64 - n as u64),
            };
            table.insert([n as u8; HASH_LEN], tunnel);
        }
        Tunnels::make_room(&mut table);
        assert!(!table.contains_key(&[0; HASH_LEN]), "the away one goes");
        Tunnels::make_room(&mut table);
        assert_eq!(table.len(), TUNNEL_CAPACITY - 1);
    }

    #[test]
    fn tunnels_and_paths_expire_after_eight_hours() {
        let start = Instant::now();
        let paths = HashMap::from([(dest(1), mirrored(0, start))]);
        let expires = start + TUNNEL_TIMEOUT * 2;
        let tunnel = Tunnel {
            iface: None,
            paths,
            expires,
        };
        let mut table = HashMap::from([([1; HASH_LEN], tunnel)]);
        Tunnels::cull(&mut table, start + TUNNEL_TIMEOUT);
        assert!(table[&[1; HASH_LEN]].paths.is_empty());
        Tunnels::cull(&mut table, start + TUNNEL_TIMEOUT * 2);
        assert!(table.is_empty());
    }

    #[test]
    fn tunnel_paths_are_bounded_across_tunnels() {
        let start = Instant::now();
        let paths = (0..TUNNEL_PATH_CAPACITY)
            .map(|n| {
                (
                    dest(n as u8),
                    mirrored(0, start + Duration::from_secs(n as u64)),
                )
            })
            .collect();
        let expires = start + TUNNEL_TIMEOUT;
        let tunnel = Tunnel {
            iface: None,
            paths,
            expires,
        };
        let mut table = HashMap::from([([1; HASH_LEN], tunnel)]);
        Tunnels::make_path_room(&mut table);
        let held = &table[&[1; HASH_LEN]].paths;
        assert_eq!(held.len(), TUNNEL_PATH_CAPACITY - 1);
        assert!(!held.contains_key(&dest(0)), "the oldest goes");
    }
}
