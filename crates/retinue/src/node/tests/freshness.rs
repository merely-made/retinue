//! Announce freshness, replay protection and the address-book bound.

use super::*;

#[test]
fn freshness_gates_effects_and_newer_route_replaces_regardless_of_hops() {
    let mut relay = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x81; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x82; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );

    let mut first = peer.announce(&blob([1, 0, 0, 0, 0, 0, 0, 0, 0, 10]), None);
    first.hops = 1;
    let accepted = relay.ingest(IFACE, &first, 0);
    assert_eq!(accepted.len(), 2, "learn plus relay");
    assert_eq!(relay.route_to(peer.destination(), 0), Some((IFACE, 1)));

    let mut newer_equal = peer.announce(&blob([2, 0, 0, 0, 0, 0, 0, 0, 0, 11]), None);
    newer_equal.hops = 1;
    let accepted = relay.ingest(IFACE + 1, &newer_equal, 1);
    assert_eq!(accepted.len(), 2, "newer equal-hop announce replaces");
    assert_eq!(relay.route_to(peer.destination(), 1), Some((IFACE + 1, 1)));

    let mut newer_worse = peer.announce(&blob([3, 0, 0, 0, 0, 0, 0, 0, 0, 12]), None);
    newer_worse.hops = 7;
    let accepted = relay.ingest(IFACE + 2, &newer_worse, 2);
    assert_eq!(accepted.len(), 2, "newer announce still learns and relays");
    assert_eq!(relay.route_to(peer.destination(), 2), Some((IFACE + 2, 7)));

    let mut stale = peer.announce(&blob([4, 0, 0, 0, 0, 0, 0, 0, 0, 11]), None);
    stale.hops = 0;
    assert!(relay.ingest(IFACE, &stale, 3).is_empty());
    assert_eq!(relay.peers().len(), 1);
    assert_eq!(relay.route_to(peer.destination(), 3), Some((IFACE + 2, 7)));
    assert_eq!(relay.transport_counters().stale_announces, 1);

    assert!(relay.ingest(IFACE, &newer_worse, 4).is_empty());
    assert_eq!(relay.transport_counters().replayed_announces, 1);
}

/// RNS culls a path row with its random blobs (`Transport.py` 957-978, 1086-1090), so the
/// next announce is a first sighting whatever its emission time or hops.
#[test]
fn an_expired_route_admits_any_announce_as_a_first_sighting() {
    let mut relay = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x83; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig {
        route_ttl: 10,
        ..TransportConfig::transit()
    });
    let better_peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x84; 64]),
        DestinationName::new("retinue", ["better"]).name_hash(),
    );
    let equal_peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x85; 64]),
        DestinationName::new("retinue", ["equal"]).name_hash(),
    );
    let worse_peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x86; 64]),
        DestinationName::new("retinue", ["worse"]).name_hash(),
    );
    for peer in [&better_peer, &equal_peer, &worse_peer] {
        let mut first = peer.announce(&blob([1, 0, 0, 0, 0, 0, 0, 0, 0, 20]), None);
        first.hops = 2;
        assert_eq!(relay.ingest(IFACE, &first, 0).len(), 2);
    }
    assert_eq!(relay.route_count(), 3);
    let _ = relay.poll(10, IFACE, Some(&blob([0; RAND_HASH_LEN])));
    assert_eq!(relay.route_count(), 0, "route TTL removed the routes");

    for (peer, hops) in [(&better_peer, 1), (&equal_peer, 2), (&worse_peer, 3)] {
        let mut older = peer.announce(&blob([2, 0, 0, 0, 0, 0, 0, 0, 0, 19]), None);
        older.hops = hops;
        assert_eq!(relay.ingest(IFACE + 1, &older, 11).len(), 2);
        assert_eq!(
            relay.route_to(peer.destination(), 11),
            Some((IFACE + 1, hops))
        );
    }
    assert_eq!(relay.transport_counters().stale_announces, 0);
}

/// A transport answering `request_path` from its cache sends the blob it already relayed
/// (`Transport.py` 3459-3530). Once the route has gone, that same blob restores it.
#[test]
fn a_same_blob_announce_restores_an_expired_route() {
    let mut n = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x91; 64]),
        DestinationName::new("retinue", ["node"]).name_hash(),
    )
    .with_transport_config(TransportConfig {
        route_ttl: 10,
        ..TransportConfig::none()
    });
    let peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x92; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let mut announce = peer.announce(&blob([7, 0, 0, 0, 0, 0, 0, 0, 0, 30]), None);
    announce.hops = 3;
    assert_eq!(n.ingest(IFACE, &announce, 0).len(), 1);

    assert!(n.ingest(IFACE, &announce, 9).is_empty(), "live: a replay");
    assert_eq!(n.transport_counters().replayed_announces, 1);

    let mut cached = announce.clone();
    cached.context = crate::path::CTX_PATH_RESPONSE;
    assert_eq!(n.route_to(peer.destination(), 10), None, "route expired");
    assert_eq!(n.ingest(IFACE + 1, &cached, 10).len(), 1);
    assert_eq!(n.route_to(peer.destination(), 10), Some((IFACE + 1, 3)));
    assert_eq!(n.transport_counters().replayed_announces, 1);

    assert!(
        n.ingest(IFACE, &announce, 11).is_empty(),
        "the restored route refuses the blob again"
    );
    assert_eq!(n.transport_counters().replayed_announces, 2);
}

#[test]
fn a_live_route_admits_only_a_later_emission() {
    let mut n = node();
    let peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x93; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let at = |nonce: u8, timebase: u8, hops: u8| {
        let mut packet = peer.announce(&blob([nonce, 0, 0, 0, 0, 0, 0, 0, 0, timebase]), None);
        packet.hops = hops;
        packet
    };
    assert_eq!(n.ingest(IFACE, &at(1, 10, 4), 0).len(), 1);
    assert_eq!(n.ingest(IFACE, &at(2, 12, 4), 1).len(), 1);
    // Between the accepted emissions, at better, equal, and worse hops: all stale.
    for hops in [1, 4, 9] {
        assert!(n.ingest(IFACE, &at(3, 11, hops), 2).is_empty());
    }
    assert!(
        n.ingest(IFACE, &at(4, 12, 1), 2).is_empty(),
        "equal emission"
    );
    assert_eq!(n.transport_counters().stale_announces, 4);
    assert_eq!(n.route_to(peer.destination(), 2), Some((IFACE, 4)));
    assert_eq!(n.ingest(IFACE + 1, &at(5, 13, 9), 3).len(), 1);
    assert_eq!(n.route_to(peer.destination(), 3), Some((IFACE + 1, 9)));
}

/// A book full of routed peers refuses the identity, but the announce is still a route
/// and still relayed, as RNS relays from its path table rather than its known
/// destinations. Because the route changed, the freshness candidate is committed.
#[test]
fn address_book_refusal_still_learns_and_relays_the_route() {
    let mut n = Node::<1, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x85; 64]),
        DestinationName::new("retinue", ["node"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let first_peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x86; 64]),
        DestinationName::new("retinue", ["first"]).name_hash(),
    );
    let second_peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x87; 64]),
        DestinationName::new("retinue", ["second"]).name_hash(),
    );
    n.ingest(
        IFACE,
        &first_peer.announce(&blob([1; RAND_HASH_LEN]), None),
        0,
    );
    let packet = second_peer.announce(&blob([2; RAND_HASH_LEN]), None);
    let candidate = AnnounceFreshnessCandidate {
        destination: second_peer.destination(),
        blob: crate::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]),
    };
    let actions = n.ingest(IFACE, &packet, 1);
    assert_eq!(n.refused_peers(), 1);
    assert!(!actions.iter().any(|a| matches!(a, Action::Learned { .. })));
    assert!(
        actions
            .iter()
            .any(|a| matches!(a, Action::Send { packet, .. }
            if packet.packet_type == PacketType::Announce
                && packet.destination == second_peer.destination())),
        "the refused identity's announce is still relayed"
    );
    assert!(!n.peers().knows(second_peer.destination()));
    assert!(n.peers().knows(first_peer.destination()));
    assert_eq!(n.route_count(), 2);
    assert_eq!(n.route_to(second_peer.destination(), 1), Some((IFACE, 0)));
    assert!(matches!(
        n.freshness.evaluate(candidate, true),
        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
    ));
}

/// A small book does not stop a node learning new destinations forever. Once the
/// routes of the peers it holds expire, the least recently heard of them yields its slot.
#[test]
fn a_full_book_evicts_a_peer_whose_route_expired() {
    let mut n = Node::<2, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x9A; 64]),
        DestinationName::new("retinue", ["node"]).name_hash(),
    )
    .with_transport_config(TransportConfig {
        route_ttl: 10,
        ..TransportConfig::transit()
    });
    let peers: [Node<8, 8, 4, 4>; 4] = core::array::from_fn(|i| {
        Node::new(
            PrivateIdentity::from_secret_bytes(&[0xA0 + i as u8; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        )
    });
    let relayed = |actions: &Actions<8>, destination: AddressHash| {
        actions.iter().any(|a| {
            matches!(a, Action::Send { packet, .. }
            if packet.packet_type == PacketType::Announce
                && packet.destination == destination)
        })
    };
    let learned = |actions: &Actions<8>, destination: AddressHash| {
        actions
            .iter()
            .any(|a| *a == Action::Learned { destination })
    };

    for (i, peer) in peers[..3].iter().enumerate() {
        let at = i as u64;
        let actions = n.ingest(
            IFACE,
            &peer.announce(&blob([i as u8; RAND_HASH_LEN]), None),
            at,
        );
        assert!(relayed(&actions, peer.destination()));
        assert_eq!(learned(&actions, peer.destination()), i < 2);
    }
    assert_eq!(
        n.refused_peers(),
        1,
        "both held peers still had live routes"
    );

    // Past every route's TTL, nothing protects the held peers.
    let later = 20;
    let fourth = peers[3].destination();
    let actions = n.ingest(
        IFACE,
        &peers[3].announce(&blob([3; RAND_HASH_LEN]), None),
        later,
    );
    assert!(learned(&actions, fourth), "admitted by eviction");
    assert!(relayed(&actions, fourth));
    assert!(n.peers().knows(fourth));
    assert!(
        !n.peers().knows(peers[0].destination()),
        "the least recently heard peer went"
    );
    assert!(n.peers().knows(peers[1].destination()));
    assert_eq!(n.peers().len(), 2);
}

/// RNS learns from a path response but never rebroadcasts it: it answers one requester.
#[test]
fn a_path_response_is_learned_but_not_relayed() {
    let mut relay = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x9B; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x9C; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let mut response = peer.announce(&blob([5; RAND_HASH_LEN]), None);
    response.context = crate::path::CTX_PATH_RESPONSE;
    response.hops = 2;
    let actions = relay.ingest(IFACE, &response, 0);
    assert_eq!(
        actions.iter().collect::<Vec<_>>(),
        [&Action::Learned {
            destination: peer.destination()
        }]
    );
    assert!(relay.peers().knows(peer.destination()));
    assert_eq!(relay.route_to(peer.destination(), 0), Some((IFACE, 2)));
    assert_eq!(relay.transport_counters().forwarded_announces, 0);
}

#[test]
fn packet_loop_dedup_is_after_freshness() {
    let mut relay = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x88; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x89; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    );
    let packet = peer.announce(&blob([4; RAND_HASH_LEN]), None);
    assert!(relay.transit_is_new(&packet, 0));
    let actions = relay.ingest(IFACE, &packet, 1);
    assert_eq!(
        actions.len(),
        1,
        "freshness learns before loop dedup suppresses relay"
    );
    assert!(actions.iter().any(|a| matches!(a, Action::Learned { .. })));
    assert_eq!(relay.peers().len(), 1);
    assert_eq!(relay.transport_counters().replayed_announces, 0);
}

#[test]
fn stale_same_blob_cannot_roll_back_ratchet_or_app_data() {
    let mut n = node();
    let peer = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x90; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    )
    .with_app_data(b"current");
    let announce_blob = blob([9; RAND_HASH_LEN]);
    let current = peer.announce(&announce_blob, Some(&[0xA1; RATCHET_LEN]));
    assert_eq!(n.ingest(IFACE, &current, 0).len(), 1);
    assert_eq!(
        n.peers().resolve(peer.destination()).unwrap().app_data,
        b"current"
    );
    assert_eq!(
        n.peers().resolve(peer.destination()).unwrap().ratchet,
        Some([0xA1; RATCHET_LEN])
    );

    let older = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x90; 64]),
        DestinationName::new("retinue", ["peer"]).name_hash(),
    )
    .with_app_data(b"rollback");
    let rollback = older.announce(&announce_blob, Some(&[0xB2; RATCHET_LEN]));
    assert!(n.ingest(IFACE, &rollback, 1).is_empty());
    let retained = n.peers().resolve(peer.destination()).unwrap();
    assert_eq!(retained.app_data, b"current");
    assert_eq!(retained.ratchet, Some([0xA1; RATCHET_LEN]));
}

#[test]
fn freshness_policy_is_bounded_and_reconfigures_without_resetting_history() {
    let mut n = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x8A; 64]),
        DestinationName::new("retinue", ["node"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    assert!(
        Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x8B; 64]),
            DestinationName::new("retinue", ["invalid"]).name_hash(),
        )
        .with_freshness_policy(FreshnessPolicy {
            max_destinations: 0,
            max_blobs_per_destination: 8,
        })
        .is_err()
    );

    let a = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x8C; 64]),
        DestinationName::new("retinue", ["a"]).name_hash(),
    );
    let b = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x8D; 64]),
        DestinationName::new("retinue", ["b"]).name_hash(),
    );
    let a_announce = a.announce(&blob([5; RAND_HASH_LEN]), None);
    n.ingest(IFACE, &a_announce, 0);
    n.ingest(IFACE, &b.announce(&blob([6; RAND_HASH_LEN]), None), 1);
    assert_eq!(n.freshness.config().destination_capacity, 8);
    let report = n
        .set_freshness_policy(FreshnessPolicy {
            max_destinations: 1,
            max_blobs_per_destination: 1,
        })
        .expect("valid bounds");
    assert_eq!(report.evicted_destinations, [a.destination()]);
    assert_eq!(n.freshness_policy().max_destinations, 1);
    assert_eq!(n.transport_counters().evicted_freshness_rows, 1);
    assert_eq!(n.transport_counters().evicted_freshness_blobs, 0);
    // A route never outlives its freshness row, so A's evicted row took its route along
    // and A's blob is a first sighting again. B's route and history survive.
    assert_eq!(n.route_to(a.destination(), 1), None);
    assert!(n.route_to(b.destination(), 1).is_some());
    assert!(
        n.ingest(IFACE, &b.announce(&blob([6; RAND_HASH_LEN]), None), 1)
            .is_empty()
    );
    assert_eq!(n.transport_counters().replayed_announces, 1);

    // Readmitting A evicts B, row and route, under the one-row bound. Packet-loop dedup
    // still suppresses the relay of a packet relayed a moment ago.
    assert!(
        n.ingest(IFACE, &a_announce, 2)
            .iter()
            .any(|action| matches!(action, Action::Learned { .. }))
    );
    assert_eq!(n.transport_counters().evicted_freshness_rows, 2);
    assert_eq!(n.route_to(b.destination(), 2), None);

    // The remaining destination's second accepted blob now exercises per-row history
    // pressure independently of destination-row pressure.
    let mut a_again = a.announce(&blob([7; RAND_HASH_LEN]), None);
    a_again.hops = 1;
    n.ingest(IFACE, &a_again, 3);
    assert_eq!(n.transport_counters().evicted_freshness_blobs, 1);
}

/// A validly signed announce naming a known destination under a different key is
/// rejected before it can touch freshness, a route or a relay (RNS `Identity.py`
/// 569-577).
#[test]
fn an_announce_with_a_different_key_for_a_known_destination_is_rejected() {
    let (_, destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let packet = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
    // Stand in for an earlier announce of the same destination hash under another key:
    // a real one would need a hash collision.
    let mut known = Announce::decode(&packet).unwrap();
    known.identity = *PrivateIdentity::from_secret_bytes(&[0x45; 64]).public();
    assert_eq!(relay.book.ingest(&known), Ingested::Learned);

    assert!(relay.ingest(IFACE, &packet, 0).is_empty());
    assert_eq!(relay.transport_counters().key_mismatch_announces, 1);
    assert_eq!(relay.route_count(), 0);
    assert_eq!(
        relay
            .peers()
            .resolve(destination.destination())
            .unwrap()
            .identity,
        known.identity
    );
}
