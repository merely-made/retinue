//! Route lifetime, eviction, interface modes and next-hop addressing.

use super::*;

/// The transport table is a fixed board resource: expired paths go first, then the
/// quietest live route makes room. A flood cannot turn it into a lifetime allocation.
#[test]
fn transport_routes_expire_then_evict_at_their_bound() {
    let mut relay = Node::<8, 8, 4, 2>::new(
        PrivateIdentity::from_secret_bytes(&[0x50; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig {
        route_ttl: 100,
        ..TransportConfig::transit()
    });
    let peer = |seed, name| {
        Node::<8, 8, 4, 2>::new(
            PrivateIdentity::from_secret_bytes(&[seed; 64]),
            DestinationName::new("retinue", [name]).name_hash(),
        )
    };
    let a = peer(0x11, "a");
    let b = peer(0x22, "b");
    let c = peer(0x33, "c");

    relay.ingest(IFACE, &a.announce(&blob([1; RAND_HASH_LEN]), None), 0);
    relay.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 1);
    assert_eq!(relay.route_count(), 2, "the typed route bound is full");

    relay.ingest(IFACE, &c.announce(&blob([3; RAND_HASH_LEN]), None), 2);
    assert_eq!(
        relay.route_count(),
        2,
        "a third route displaces, never grows"
    );
    assert_eq!(
        relay.route_to(a.destination(), 2),
        None,
        "the quietest live route was evicted"
    );
    assert_eq!(relay.transport_counters().evicted_routes, 1);

    let _ = relay.poll(102, IFACE, Some(&blob([0; RAND_HASH_LEN])));
    assert_eq!(relay.route_count(), 0, "stale routes are reclaimed by poll");
    assert_eq!(relay.transport_counters().expired_routes, 2);
}

/// A transit source addresses its own request to the relay that taught it the route, and
/// reports that relay as the next hop until the route's TTL passes.
#[test]
fn open_link_addresses_the_first_relay() {
    let (_, mut destination) = pair();
    let transit = |seed, name| {
        Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[seed; 64]),
            DestinationName::new("retinue", [name]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit())
    };
    let mut source = transit(0x46, "source");
    let mut relay = transit(0x47, "relay");

    let announce = destination.announce(&blob([0x78; RAND_HASH_LEN]), None);
    let relayed = relayed(&mut relay, IFACE, &announce, 0).unwrap();
    source.ingest(IFACE, &relayed, 1);
    let hop = source.next_hop(destination.destination(), 1).unwrap();
    assert_eq!(hop.via, Some(relay.identity.hash()));
    assert_eq!(hop.hops, 1);
    assert_eq!(
        relay.next_hop(destination.destination(), 1).unwrap().via,
        None
    );

    let request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x9A; 64], 1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request.header_type, HeaderType::Type2);
    assert_eq!(request.transport, Some(relay.identity.hash()));
    let forwarded = sent(&relay.ingest(IFACE, &request, 2)).unwrap();
    let proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
    let back = sent(&relay.ingest(IFACE, &proof, 4)).unwrap();
    assert!(link_up(&source.ingest(IFACE, &back, 5)).is_some());

    let expired = 1 + DEFAULT_ROUTE_TTL;
    assert_eq!(source.next_hop(destination.destination(), expired), None);
    assert_eq!(
        source.route_count(),
        1,
        "the read-only accessor does not evict"
    );
}

/// `open_link` reads the route's TTL at its own `now`: one tick before expiry the route
/// names the relay, and at expiry the request is not addressed through it, though nothing
/// has evicted the route yet.
#[test]
fn open_link_does_not_address_via_an_expired_unevicted_route() {
    let (mut source, destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x4A; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let announce = destination.announce(&blob([0x7A; RAND_HASH_LEN]), None);
    let learned = 1;
    source.ingest(
        IFACE,
        &relayed(&mut relay, IFACE, &announce, 0).unwrap(),
        learned,
    );
    let expiry = learned + DEFAULT_ROUTE_TTL;

    assert_eq!(
        source
            .next_hop(destination.destination(), expiry - 1)
            .and_then(|hop| hop.via),
        Some(relay.identity.hash())
    );

    let stale = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x9D; 64], expiry)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(source.route_count(), 1, "the expired route is not evicted");
    assert_eq!(stale.header_type, HeaderType::Type1);
    assert_eq!(stale.transport, None);
}

/// A transit relay and a source that learned `destination` through it, at tick 0.
fn source_via_relay() -> (Node<32, 8, 4>, Node<32, 8, 4, 4>, Node<32, 8, 4>) {
    let (mut source, destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x4B; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let announce = destination.announce(&blob([0x7B; RAND_HASH_LEN]), None);
    source.ingest(IFACE, &relayed(&mut relay, IFACE, &announce, 0).unwrap(), 0);
    (source, relay, destination)
}

/// RNS keeps a path for a week (`PATHFINDER_E`), so a route outlives a day of quiet.
#[test]
fn routes_live_for_a_week_by_default() {
    assert_eq!(DEFAULT_ROUTE_TTL, 7 * 24 * 60 * 60 * 1_000);
    assert_eq!(TransportConfig::none().route_ttl, DEFAULT_ROUTE_TTL);
    assert_eq!(TransportConfig::transit().route_ttl, DEFAULT_ROUTE_TTL);
    let (source, _, destination) = source_via_relay();
    let day = 24 * 60 * 60 * 1_000;
    assert!(source.next_hop(destination.destination(), day).is_some());
    assert!(
        source
            .next_hop(destination.destination(), DEFAULT_ROUTE_TTL)
            .is_none()
    );
}

/// Access-point and roaming interfaces bound their routes to a day and six hours, and
/// never lengthen a shorter configured lifetime.
#[test]
fn interface_modes_shorten_route_lifetimes() {
    assert_eq!(
        InterfaceMode::Full.route_ttl(DEFAULT_ROUTE_TTL),
        DEFAULT_ROUTE_TTL
    );
    assert_eq!(
        InterfaceMode::AccessPoint.route_ttl(DEFAULT_ROUTE_TTL),
        ACCESS_POINT_ROUTE_TTL
    );
    assert_eq!(InterfaceMode::Roaming.route_ttl(10), 10);

    let (_, destination) = pair();
    let mut n = node();
    n.set_interface_mode(1, InterfaceMode::Roaming).unwrap();
    n.set_interface_mode(2, InterfaceMode::AccessPoint).unwrap();
    assert_eq!(n.interface_mode(1), InterfaceMode::Roaming);
    assert_eq!(n.interface_mode(3), InterfaceMode::Full);
    let announce = destination.announce(&blob([0x7C; RAND_HASH_LEN]), None);
    n.ingest(1, &announce, 0);
    assert!(
        n.next_hop(destination.destination(), ROAMING_ROUTE_TTL - 1)
            .is_some()
    );
    assert!(
        n.next_hop(destination.destination(), ROAMING_ROUTE_TTL)
            .is_none()
    );

    n.forget_interface(1);
    n.ingest(
        2,
        &destination.announce(&blob([0x7D; RAND_HASH_LEN]), None),
        0,
    );
    assert!(
        n.next_hop(destination.destination(), ACCESS_POINT_ROUTE_TTL - 1)
            .is_some()
    );
    assert!(
        n.next_hop(destination.destination(), ACCESS_POINT_ROUTE_TTL)
            .is_none()
    );

    // Clearing a mode frees its slot; a fifth distinct mode does not fit.
    n.set_interface_mode(2, InterfaceMode::Full).unwrap();
    for interface in 10..14 {
        n.set_interface_mode(interface, InterfaceMode::Roaming)
            .unwrap();
    }
    assert_eq!(
        n.set_interface_mode(14, InterfaceMode::Roaming),
        Err(InterfaceModeTableFull)
    );
}

/// RNS refreshes a path whenever it carries a packet. A source addressing its relay, and
/// a relay carrying transit, each keep the route alive past its learned-at expiry.
#[test]
fn a_route_in_use_is_refreshed() {
    let (mut source, mut relay, destination) = source_via_relay();
    let late = DEFAULT_ROUTE_TTL - 1;
    let request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x9E; 64], late)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request.transport, Some(relay.identity.hash()));
    assert!(
        source
            .next_hop(destination.destination(), DEFAULT_ROUTE_TTL)
            .is_some(),
        "addressing the relay refreshed the source's route"
    );

    assert!(sent(&relay.ingest(IFACE, &request, late)).is_some());
    assert!(
        relay
            .next_hop(destination.destination(), DEFAULT_ROUTE_TTL)
            .is_some(),
        "carrying transit refreshed the relay's route"
    );
}

/// Detaching an interface takes its routes and bridges with it, so a request goes out
/// without a route rather than naming a relay on an interface that is gone.
#[test]
fn forgetting_an_interface_culls_its_routes_and_bridges() {
    let (mut source, mut relay, destination) = source_via_relay();
    relay.ingest(
        IFACE + 1,
        &destination.announce(&blob([0x7E; RAND_HASH_LEN]), None),
        0,
    );
    relay.remember_bridge(
        AddressHash::from_bytes([0xB1; 16]),
        destination.destination(),
        IFACE,
        IFACE + 1,
        u64::MAX,
        0,
    );
    relay.remember_bridge(
        AddressHash::from_bytes([0xB2; 16]),
        destination.destination(),
        IFACE,
        IFACE + 2,
        u64::MAX,
        0,
    );
    relay.forget_interface(IFACE + 1);
    assert_eq!(relay.route_count(), 0);
    assert_eq!(relay.bridges.len(), 1);
    assert_eq!(relay.bridges[0].out, IFACE + 2);

    source.set_first_hop_airtime(IFACE, 64).unwrap();
    source.forget_interface(IFACE);
    assert_eq!(source.first_hop_airtime(IFACE), 0);
    assert!(source.next_hop(destination.destination(), 1).is_none());
    let request = sent(
        &source
            .open_link(destination.destination(), IFACE + 3, &[0x9F; 64], 1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request.header_type, HeaderType::Type1);
}

/// This is the desk half of the T114 flood: enough distinct signed announces to turn the
/// route table over many times, while every retained table stays at its declared ceiling.
/// The board's allocator probe supplies the separate live-byte high-water receipt.
#[test]
fn transport_flood_keeps_retained_state_bounded() {
    let mut relay = Node::<128, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x55; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());

    for seed in 1_u8..=32 {
        let peer = Node::<128, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[seed; 64]),
            DestinationName::new("retinue", ["flood"]).name_hash(),
        );
        let at = u64::from(seed) * 10_000;
        let actions = relay.ingest(
            IFACE,
            &peer.announce(&blob([seed; RAND_HASH_LEN]), None),
            at,
        );
        assert!(actions.len() <= 1, "one learn at most");
        assert_eq!(relay.poll(at + REBROADCAST_WINDOW, IFACE, None).len(), 1);
        let retry = at + 2 * REBROADCAST_WINDOW + REBROADCAST_GRACE;
        assert_eq!(relay.poll(retry, IFACE, None).len(), 1, "and one retry");
        assert_eq!(relay.rebroadcasts.len(), 0);
        assert_eq!(
            relay.route_count(),
            usize::from(seed).min(4),
            "route residency remains at its four-entry ceiling"
        );
    }
    let counters = relay.transport_counters();
    assert_eq!(counters.forwarded_announces, 64);
    assert_eq!(counters.evicted_routes, 28);
    assert_eq!(relay.route_count(), 4);
}

/// The modes RNS gives no route lifetime of their own expire routes as full mode does
/// (`Transport.py` 964-969).
#[test]
fn other_modes_keep_the_full_route_lifetime() {
    for mode in [
        InterfaceMode::PointToPoint,
        InterfaceMode::Boundary,
        InterfaceMode::Gateway,
        InterfaceMode::Internal,
    ] {
        assert_eq!(mode.route_ttl(DEFAULT_ROUTE_TTL), DEFAULT_ROUTE_TTL);
    }
}

/// A relay leaves where it was heard, so an access-point or roaming interface keeps it, and
/// a boundary one passes it on (`Transport.py` 1458-1516).
#[test]
fn interface_modes_gate_relayed_announces() {
    for (mode, relays) in [
        (InterfaceMode::AccessPoint, false),
        (InterfaceMode::Roaming, false),
        (InterfaceMode::Boundary, true),
        (InterfaceMode::Full, true),
    ] {
        let mut relay = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x52; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        relay.set_interface_mode(IFACE, mode).unwrap();
        let (_, destination) = pair();
        relay.ingest(
            IFACE,
            &destination.announce(&blob([0x7E; RAND_HASH_LEN]), None),
            0,
        );
        let due = relay.next_rebroadcast().expect("scheduled");
        assert_eq!(
            sent(&relay.poll(due, IFACE, None)).is_some(),
            relays,
            "{mode:?}"
        );
        assert!(
            relay.next_hop(destination.destination(), due).is_some(),
            "still learned"
        );
    }
}
