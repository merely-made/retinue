//! Read-only management facts for one Postilion station.
//!
//! This module retains observations and classifies only application data that Outrider can
//! decode. Product roles, staleness policy, and graph identities belong to the host.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use outrider::{DeliveryAnnounce, PropagationAnnounce};
use retinue::announce_admission::AnnounceIngressCounters;
use retinue::endpoint::{
    AnnounceFact, Endpoint, InterfaceId, LinkFact, QueueCounters, RouteFact, RoutingCounters,
};
use retinue::hash::AddressHash;
use retinue::identity::Identity;

use crate::{Peer, StationRadioConfig};

/// Ordinary host default. Callers own this policy through [`crate::StationConfig`].
pub const DEFAULT_ANNOUNCE_HISTORY_BOUND: usize = 256;

/// Application data classification backed by a successful Outrider decode.
#[derive(Clone, Debug, PartialEq)]
pub enum AnnounceKind {
    Delivery(DeliveryAnnounce),
    Propagation(PropagationAnnounce),
    Unknown,
}

/// One announce observation aged against the snapshot's single capture instant.
#[derive(Clone, Debug, PartialEq)]
pub struct AnnounceObservation {
    pub fact: AnnounceFact,
    pub kind: AnnounceKind,
    pub age: Duration,
}

/// Public, non-secret station facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StationFact {
    pub identity: Identity,
    pub delivery_destination: AddressHash,
    pub name: String,
    pub radio: StationRadioConfig,
}

/// Per-interface ingress accounting in the same deterministic order as `interfaces`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfaceAnnounceCounters {
    pub interface: InterfaceId,
    pub counters: AnnounceIngressCounters,
}

/// Point-in-time endpoint counters. These are observations, not delivery receipts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManagementCounters {
    pub routing: RoutingCounters,
    pub queue: QueueCounters,
    pub outbound_queue_depth: usize,
    pub announce_ingress: Vec<InterfaceAnnounceCounters>,
}

/// One bounded, deterministic management read model.
#[derive(Clone, Debug, PartialEq)]
pub struct ManagementSnapshot {
    pub generation: ManagementGeneration,
    pub station: StationFact,
    pub interfaces: Vec<InterfaceId>,
    pub routes: Vec<RouteFact>,
    pub links: Vec<LinkFact>,
    pub current_announces: Vec<AnnounceObservation>,
    pub announce_history: Vec<AnnounceObservation>,
    pub counters: ManagementCounters,
}

/// One ordered source revision. Endpoint topology and Postilion observation changes advance
/// independently without being collapsed into a lossy hash.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct ManagementGeneration {
    pub endpoint: u64,
    pub observations: u64,
    /// Monotonic within one endpoint revision; resets only after `endpoint` advances.
    pub route_expirations: u64,
}

impl ManagementSnapshot {
    pub(crate) fn capture(
        endpoint: &Endpoint,
        identity: Identity,
        delivery_destination: AddressHash,
        name: &str,
        radio: &StationRadioConfig,
        management: &Mutex<ManagementState>,
        captured_at: Instant,
    ) -> Self {
        let (observation_generation, current_announces, announce_history) =
            management.lock().unwrap().observations_at(captured_at);
        let endpoint_facts = endpoint.diagnostic_facts_at(captured_at);
        let interfaces = endpoint_facts.interfaces;
        let announce_ingress = interfaces
            .iter()
            .copied()
            .map(|interface| InterfaceAnnounceCounters {
                interface,
                counters: endpoint.announce_ingress_counters(interface),
            })
            .collect();

        Self {
            generation: ManagementGeneration {
                endpoint: endpoint_facts.generation,
                observations: observation_generation,
                route_expirations: endpoint_facts.expired_routes,
            },
            station: StationFact {
                identity,
                delivery_destination,
                name: name.to_owned(),
                radio: radio.clone(),
            },
            interfaces,
            routes: endpoint_facts.routes,
            links: endpoint_facts.links,
            current_announces,
            announce_history,
            counters: ManagementCounters {
                routing: endpoint.routing_counters(),
                queue: endpoint.queue_counters(),
                outbound_queue_depth: endpoint.outbound_queue_depth(),
                announce_ingress,
            },
        }
    }
}

#[derive(Clone)]
struct StoredObservation {
    peer: Peer,
    kind: AnnounceKind,
    observed_at: Instant,
}

impl StoredObservation {
    fn public_at(&self, captured_at: Instant) -> AnnounceObservation {
        AnnounceObservation {
            fact: self.peer.announce.clone(),
            kind: self.kind.clone(),
            age: captured_at
                .checked_duration_since(self.observed_at)
                .unwrap_or_default(),
        }
    }
}

/// The synchronized host state shared by the announce task and snapshot callers.
pub(crate) struct ManagementState {
    current: BTreeMap<AddressHash, StoredObservation>,
    history: VecDeque<StoredObservation>,
    history_bound: usize,
    generation: u64,
}

impl ManagementState {
    pub(crate) fn new(history_bound: usize) -> Self {
        Self {
            current: BTreeMap::new(),
            history: VecDeque::with_capacity(history_bound),
            history_bound,
            generation: 0,
        }
    }

    /// Refresh current peer data and append the observation to bounded history. Returns
    /// whether this destination was first seen, preserving `PeerAppeared` semantics.
    pub(crate) fn observe(&mut self, peer: Peer, observed_at: Instant) -> bool {
        let fresh = !self.current.contains_key(&peer.destination);
        let stored = StoredObservation {
            kind: classify(&peer.announce.app_data),
            peer,
            observed_at,
        };
        self.current.insert(stored.peer.destination, stored.clone());
        if self.history_bound > 0 {
            self.history.push_back(stored);
            while self.history.len() > self.history_bound {
                self.history.pop_front();
            }
        }
        self.generation = self.generation.saturating_add(1);
        fresh
    }

    pub(crate) fn peers(&self) -> Vec<Peer> {
        self.current
            .values()
            .map(|observation| observation.peer.clone())
            .collect()
    }

    fn observations_at(
        &self,
        captured_at: Instant,
    ) -> (u64, Vec<AnnounceObservation>, Vec<AnnounceObservation>) {
        let current = self
            .current
            .values()
            .map(|observation| observation.public_at(captured_at))
            .collect();
        let mut history: Vec<_> = self
            .history
            .iter()
            .map(|observation| observation.public_at(captured_at))
            .collect();
        history.sort_by_key(|observation| observation.fact.sequence);
        (self.generation, current, history)
    }
}

fn classify(app_data: &[u8]) -> AnnounceKind {
    if let Ok(delivery) = DeliveryAnnounce::decode(app_data) {
        AnnounceKind::Delivery(delivery)
    } else if let Ok(propagation) = PropagationAnnounce::decode(app_data) {
        AnnounceKind::Propagation(propagation)
    } else {
        AnnounceKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use outrider::{PropagationCosts, delivery_destination, delivery_name};
    use retinue::announce;
    use retinue::announce_admission::AnnounceIngressPolicy;
    use retinue::destination::DestinationName;
    use retinue::endpoint::{LinkDirection, LinkRemoteFact};
    use retinue::identity::PrivateIdentity;
    use retinue::link::{self, LinkMode, LinkTrailer};
    use retinue::packet::HeaderType;

    fn peer(destination_byte: u8, sequence: u64, app_data: Vec<u8>) -> Peer {
        let identity = PrivateIdentity::from_secret_bytes(&[destination_byte; 64]);
        let destination = AddressHash::from_bytes([destination_byte; 16]);
        Peer {
            destination,
            stamp_cost: None,
            name: None,
            announce: AnnounceFact {
                destination,
                identity: *identity.public(),
                app_data,
                interface: 7,
                hops: 1,
                transport: None,
                sequence,
            },
        }
    }

    #[test]
    fn repeated_announces_refresh_current_and_append_bounded_history() {
        let start = Instant::now();
        let mut state = ManagementState::new(2);
        let first = DeliveryAnnounce::named(b"old".to_vec()).encode().unwrap();
        let refreshed = DeliveryAnnounce::named(b"new".to_vec()).encode().unwrap();

        assert!(state.observe(peer(1, 1, first), start));
        assert!(!state.observe(peer(1, 2, refreshed), start + Duration::from_secs(1)));
        assert!(state.observe(peer(2, 3, vec![0xc1]), start + Duration::from_secs(2)));

        let (generation, current, history) = state.observations_at(start + Duration::from_secs(4));
        assert_eq!(generation, 3);
        assert_eq!(current.len(), 2, "one current identity per destination");
        assert_eq!(history.len(), 2, "history obeys its caller-owned bound");
        assert_eq!(history[0].fact.sequence, 2);
        assert_eq!(history[0].age, Duration::from_secs(3));
        assert_eq!(history[1].fact.sequence, 3);
        assert_eq!(history[1].kind, AnnounceKind::Unknown);

        let refreshed = current
            .iter()
            .find(|observation| observation.fact.destination == AddressHash::from_bytes([1; 16]))
            .unwrap();
        assert!(matches!(
            &refreshed.kind,
            AnnounceKind::Delivery(DeliveryAnnounce { display_name: Some(name), .. })
                if name == b"new"
        ));
    }

    #[test]
    fn propagation_is_typed_only_when_its_whole_shape_decodes() {
        let propagation = PropagationAnnounce {
            legacy: false,
            unix_time: 42,
            active: true,
            transfer_limit_kib: 128,
            sync_limit_kib: 64,
            costs: PropagationCosts {
                propagation: 1,
                flexibility: 2,
                peering: 3,
            },
            metadata: Vec::new(),
        };
        assert_eq!(
            classify(&propagation.encode().unwrap()),
            AnnounceKind::Propagation(propagation)
        );
        assert_eq!(classify(&[0xc1]), AnnounceKind::Unknown);
    }

    #[test]
    fn capture_ages_are_read_only_and_use_the_supplied_instant() {
        let start = Instant::now();
        let mut state = ManagementState::new(4);
        state.observe(peer(1, 1, Vec::new()), start);

        let first = state.observations_at(start + Duration::from_secs(5));
        let second = state.observations_at(start + Duration::from_secs(5));
        assert_eq!(first, second);
        assert_eq!(first.1[0].age, Duration::from_secs(5));
        assert_eq!(state.generation, 1, "capturing did not mutate generation");
    }

    #[tokio::test]
    async fn one_fixture_captures_the_management_boundary_without_invention() {
        let local = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
        let endpoint = Endpoint::new(local.clone());
        let ingress_policy = AnnounceIngressPolicy {
            enabled: false,
            ..AnnounceIngressPolicy::default()
        };
        endpoint.set_announce_ingress_policy(ingress_policy);
        let interface = endpoint.attach_interface();
        let interface_id = interface.id();
        let sink = interface.sink();
        let started = Instant::now();
        let management = Mutex::new(ManagementState::new(2));

        let direct = PrivateIdentity::from_secret_bytes(&[0x32; 64]);
        let old_delivery = DeliveryAnnounce::named(b"old direct".to_vec())
            .encode()
            .unwrap();
        let first_packet = announce::build(
            &direct,
            delivery_name().name_hash(),
            &[0x41; 10],
            None,
            &old_delivery,
        );
        assert!(sink.deliver(first_packet));
        let first = tokio::time::timeout(Duration::from_secs(1), endpoint.next_announcement())
            .await
            .expect("direct delivery announce surfaced")
            .unwrap();
        assert!(
            management
                .lock()
                .unwrap()
                .observe(Peer::from_announce(first), started)
        );

        let refreshed_delivery = DeliveryAnnounce::named(b"refreshed direct".to_vec())
            .encode()
            .unwrap();
        let refreshed_packet = announce::build(
            &direct,
            delivery_name().name_hash(),
            &[0x42; 10],
            None,
            &refreshed_delivery,
        );
        assert!(sink.deliver(refreshed_packet));
        let refreshed = tokio::time::timeout(Duration::from_secs(1), endpoint.next_announcement())
            .await
            .expect("refreshed delivery announce surfaced")
            .unwrap();
        assert!(!management.lock().unwrap().observe(
            Peer::from_announce(refreshed),
            started + Duration::from_secs(1),
        ));

        let unknown = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
        let unknown_name = DestinationName::new("signalman", ["unknown"]);
        let transport = AddressHash::from_bytes([0x44; 16]);
        let mut transported_packet = announce::build(
            &unknown,
            unknown_name.name_hash(),
            &[0x43; 10],
            None,
            &[0xc1],
        );
        transported_packet.header_type = HeaderType::Type2;
        transported_packet.hops = 2;
        transported_packet.transport = Some(transport);
        assert!(sink.deliver(transported_packet));
        let transported =
            tokio::time::timeout(Duration::from_secs(1), endpoint.next_announcement())
                .await
                .expect("transported unknown announce surfaced")
                .unwrap();
        assert!(management.lock().unwrap().observe(
            Peer::from_announce(transported),
            started + Duration::from_secs(2),
        ));

        let radio = StationRadioConfig {
            port: "fixture".to_owned(),
            bandwidth_hz: 250_000,
            radio: crate::Radio::Phy,
            announce_interval: Duration::from_secs(30),
            announce_history_bound: 2,
        };
        let capture_at = started + Duration::from_secs(3);
        let before_link = ManagementSnapshot::capture(
            &endpoint,
            *local.public(),
            delivery_destination(local.public()),
            "local",
            &radio,
            &management,
            capture_at,
        );

        let local_service = DestinationName::new("signalman", ["fixture"]);
        let local_service_destination = local_service.destination_hash(endpoint.identity());
        endpoint.register(local_service, b"");
        let (_, link_request) = link::PendingLink::open(
            local_service_destination,
            *endpoint.identity(),
            &[0x51; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        assert!(sink.deliver(link_request));
        let accepted = tokio::time::timeout(Duration::from_secs(1), endpoint.accept_on_any())
            .await
            .expect("inbound fixture link accepted")
            .unwrap();

        let endpoint_generation = endpoint.diagnostic_generation();
        let observation_generation = management.lock().unwrap().generation;
        let snapshot = ManagementSnapshot::capture(
            &endpoint,
            *local.public(),
            delivery_destination(local.public()),
            "local",
            &radio,
            &management,
            capture_at,
        );
        let same_snapshot = ManagementSnapshot::capture(
            &endpoint,
            *local.public(),
            delivery_destination(local.public()),
            "local",
            &radio,
            &management,
            capture_at,
        );

        assert!(snapshot.generation > before_link.generation);
        assert_eq!(snapshot.generation.endpoint, endpoint_generation);
        assert_eq!(snapshot, same_snapshot, "same capture is deterministic");
        assert_eq!(endpoint.diagnostic_generation(), endpoint_generation);
        assert_eq!(
            management.lock().unwrap().generation,
            observation_generation
        );
        assert_eq!(snapshot.interfaces, vec![interface_id]);
        assert!(
            snapshot
                .routes
                .iter()
                .all(|route| snapshot.interfaces.contains(&route.interface))
        );
        assert!(
            snapshot
                .links
                .iter()
                .all(|link| snapshot.interfaces.contains(&link.interface))
        );
        assert_eq!(snapshot.current_announces.len(), 2);
        assert_eq!(snapshot.announce_history.len(), 2);
        assert_eq!(snapshot.announce_history[0].fact.sequence, 2);
        assert_eq!(snapshot.announce_history[1].fact.sequence, 3);
        assert!(
            snapshot
                .current_announces
                .iter()
                .any(|observation| matches!(
                    &observation.kind,
                    AnnounceKind::Delivery(DeliveryAnnounce { display_name: Some(name), .. })
                        if name == b"refreshed direct"
                ))
        );
        assert!(
            snapshot
                .current_announces
                .iter()
                .any(|observation| observation.kind == AnnounceKind::Unknown)
        );
        assert!(
            snapshot
                .routes
                .iter()
                .any(|route| route.transport == Some(transport) && route.hops == 2)
        );
        assert!(snapshot.links.iter().any(|link| {
            link.direction == LinkDirection::Inbound && link.remote == LinkRemoteFact::default()
        }));

        let expired = ManagementSnapshot::capture(
            &endpoint,
            *local.public(),
            delivery_destination(local.public()),
            "local",
            &radio,
            &management,
            capture_at + Duration::from_secs(60 * 60),
        );
        assert!(expired.routes.is_empty());
        assert!(expired.generation > snapshot.generation);
        assert_eq!(expired.generation.route_expirations, 2);

        drop(accepted);
    }
}
