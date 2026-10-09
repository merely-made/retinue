use super::*;
use outrider::{DeliveryAnnounce, PropagationAnnounce, PropagationCosts};
use postilion::management::{
    AnnounceObservation, InterfaceAnnounceCounters, ManagementCounters, StationFact,
};
use postilion::{Radio, StationRadioConfig};
use retinue::announce_admission::AnnounceIngressCounters;
use retinue::endpoint::{
    AnnounceFact, LinkFact, LinkRemoteFact, QueueCounters, RouteFact, RoutingCounters,
};
use retinue::identity::PrivateIdentity;

fn snapshot() -> ManagementSnapshot {
    let local = PrivateIdentity::from_secret_bytes(&[1; 64]);
    let direct = PrivateIdentity::from_secret_bytes(&[2; 64]);
    let propagation = PrivateIdentity::from_secret_bytes(&[3; 64]);
    let unknown = PrivateIdentity::from_secret_bytes(&[4; 64]);
    let interface = 7;
    let generation = ManagementGeneration {
        endpoint: 8,
        observations: 9,
        route_expirations: 0,
    };
    let observation = |identity: &PrivateIdentity,
                       destination: AddressHash,
                       sequence: u64,
                       age: Duration,
                       transport: Option<AddressHash>,
                       kind: AnnounceKind| AnnounceObservation {
        fact: AnnounceFact {
            destination,
            identity: *identity.public(),
            app_data: Vec::new(),
            interface,
            hops: if transport.is_some() { 2 } else { 1 },
            transport,
            sequence,
        },
        kind,
        age,
    };
    let direct_destination = AddressHash::from_bytes([0x21; 16]);
    let propagation_destination = AddressHash::from_bytes([0x31; 16]);
    let unknown_destination = AddressHash::from_bytes([0x41; 16]);
    let transport = AddressHash::from_bytes([0x51; 16]);
    ManagementSnapshot {
        generation,
        station: StationFact {
            identity: *local.public(),
            delivery_destination: AddressHash::from_bytes([0x11; 16]),
            name: "Local".to_owned(),
            radio: StationRadioConfig {
                port: "fixture".to_owned(),
                bandwidth_hz: 250_000,
                radio: Radio::Phy,
                announce_interval: Duration::from_secs(30),
                announce_history_bound: 8,
            },
        },
        interfaces: vec![interface],
        routes: vec![RouteFact {
            destination: unknown_destination,
            interface,
            transport: Some(transport),
            hops: 2,
            age: Duration::from_secs(4),
        }],
        links: vec![
            LinkFact {
                id: AddressHash::from_bytes([0x61; 16]),
                interface,
                kind: LinkFactKind::Reliable,
                direction: LinkDirection::Outbound,
                remote: LinkRemoteFact {
                    destination: Some(direct_destination),
                    identity: Some(*direct.public()),
                },
            },
            LinkFact {
                id: AddressHash::from_bytes([0x62; 16]),
                interface,
                kind: LinkFactKind::Resource,
                direction: LinkDirection::Inbound,
                remote: LinkRemoteFact::default(),
            },
            LinkFact {
                id: AddressHash::from_bytes([0x63; 16]),
                interface,
                kind: LinkFactKind::Reliable,
                direction: LinkDirection::Inbound,
                remote: LinkRemoteFact {
                    destination: None,
                    identity: Some(*propagation.public()),
                },
            },
        ],
        current_announces: vec![
            observation(
                &direct,
                direct_destination,
                1,
                Duration::from_secs(2),
                None,
                AnnounceKind::Delivery(DeliveryAnnounce::named(b"Direct".to_vec())),
            ),
            observation(
                &propagation,
                propagation_destination,
                2,
                Duration::from_secs(3),
                None,
                AnnounceKind::Propagation(PropagationAnnounce {
                    legacy: false,
                    unix_time: 1,
                    active: true,
                    transfer_limit_kib: 1,
                    sync_limit_kib: 1,
                    costs: PropagationCosts {
                        propagation: 1,
                        flexibility: 1,
                        peering: 1,
                    },
                    metadata: Vec::new(),
                }),
            ),
            observation(
                &unknown,
                unknown_destination,
                3,
                Duration::from_secs(120),
                Some(transport),
                AnnounceKind::Unknown,
            ),
        ],
        announce_history: Vec::new(),
        counters: ManagementCounters {
            routing: RoutingCounters::default(),
            queue: QueueCounters::default(),
            outbound_queue_depth: 0,
            announce_ingress: vec![InterfaceAnnounceCounters {
                interface,
                counters: AnnounceIngressCounters::default(),
            }],
        },
    }
}

#[test]
fn projection_is_typed_stable_and_does_not_invent_links_or_peering() {
    let material = project_management(
        &snapshot(),
        1_000_000,
        StalePolicy {
            after: Duration::from_secs(60),
        },
    );
    assert!(material.relations.iter().all(|relation| {
        material.nodes.iter().any(|node| node.id == relation.from)
            && material.nodes.iter().any(|node| node.id == relation.to)
    }));
    assert_eq!(
        material
            .relations
            .iter()
            .filter(|relation| relation.kind == ManagementRelationKind::LiveLink)
            .count(),
        2,
        "the unattributed inbound link does not acquire a guessed endpoint"
    );
    assert!(
        !material
            .nodes
            .iter()
            .any(|node| node.id.as_str().starts_with("identity:")),
        "an identity-only link reuses the destination proven by its announce"
    );
    assert!(
        !material
            .relations
            .iter()
            .any(|relation| { relation.kind.vocabulary() == "signalman:propagation-peering" })
    );

    let propagation = material
        .nodes
        .iter()
        .find(|node| {
            node.announce_classes
                .contains(&AnnounceClassification::Propagation)
        })
        .unwrap();
    assert!(propagation.roles.contains(&ManagementRole::PropagationNode));
    assert!(
        propagation.label.starts_with("Propagation "),
        "a fresher link does not erase the decoded announce label"
    );
    let unknown = material
        .nodes
        .iter()
        .find(|node| {
            node.announce_classes
                .contains(&AnnounceClassification::Unknown)
        })
        .unwrap();
    assert!(!unknown.roles.contains(&ManagementRole::PropagationNode));
    assert_eq!(
        unknown.presence,
        ManagementPresence::Live,
        "a live route wins over an old announce"
    );

    let old_announce = material
        .relations
        .iter()
        .find(|relation| relation.source.observation_sequence == Some(3))
        .unwrap();
    assert_eq!(old_announce.source.observed_unix_ms, 880_000);
}

#[test]
fn stable_ids_and_order_do_not_depend_on_snapshot_order() {
    let first = snapshot();
    let mut reversed = first.clone();
    reversed.current_announces.reverse();
    reversed.routes.reverse();
    reversed.links.reverse();
    let first = project_management(&first, 500_000, StalePolicy::default());
    let reversed = project_management(&reversed, 500_000, StalePolicy::default());
    assert_eq!(first, reversed);
}

#[test]
fn old_routes_and_their_transport_do_not_bypass_stale_policy() {
    let mut snapshot = snapshot();
    snapshot.routes[0].age = Duration::from_secs(120);
    let destination = snapshot.routes[0].destination;
    let transport = snapshot.routes[0].transport.unwrap();
    let material = project_management(
        &snapshot,
        1_000_000,
        StalePolicy {
            after: Duration::from_secs(60),
        },
    );
    for id in [
        ManagementNodeId::destination(destination),
        ManagementNodeId::destination(transport),
    ] {
        let node = material
            .nodes
            .iter()
            .find(|node| node.id == id)
            .expect("stale route material remains visible");
        assert_eq!(node.presence, ManagementPresence::Stale);
        assert!(node.roles.contains(&ManagementRole::KnownButStale));
    }
}
