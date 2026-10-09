//! Announce freshness: replays, stale emissions, held release, and capacity.

use super::*;

#[tokio::test]
async fn freshness_replay_and_stale_rejection_leave_all_announce_effects_unchanged() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x41; 64]));
    let iface = ep.attach_interface().id();
    ep.enable_routing();
    let peer = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let (accepted_packet, accepted) = freshness_announce(&peer, "freshness-effects", 0, 1, 10, 1);
    let destination = accepted.destination;
    process_verified_announce(&ep.shared, iface, accepted_packet.clone(), accepted.clone());
    let first = ep.next_announcement().await.expect("accepted announcement");
    let route = *ep
        .shared
        .path_table
        .lock()
        .unwrap()
        .get(&destination)
        .expect("accepted route");
    let seen = ep.shared.seen_announces.lock().unwrap().1.len();

    // Same wire blob in a different context is still an exact freshness replay. It must
    // not advance the address book, route, publication sequence, relay cache, or output.
    let (mut replay_packet, replay) =
        freshness_announce(&peer, "freshness-effects", 0x0b, 1, 10, 1);
    replay_packet.context = crate::path::CTX_PATH_RESPONSE;
    process_verified_announce(&ep.shared, iface, replay_packet, replay);

    // A distinct blob behind the incumbent is stale for the same destination.
    let (stale_packet, stale) = freshness_announce(&peer, "freshness-effects", 0, 2, 9, 3);
    process_verified_announce(&ep.shared, iface, stale_packet, stale);

    assert_eq!(
        ep.shared
            .address_book
            .lock()
            .unwrap()
            .resolve(destination)
            .expect("accepted peer retained")
            .announces_seen,
        1,
    );
    assert_eq!(
        *ep.shared
            .path_table
            .lock()
            .unwrap()
            .get(&destination)
            .unwrap(),
        route,
    );
    assert_eq!(
        ep.shared.announce_sequence.load(Ordering::Relaxed),
        first.sequence
    );
    assert_eq!(ep.shared.seen_announces.lock().unwrap().1.len(), seen);
    assert!(
        matches!(
            ep.announce_rx
                .try_lock()
                .expect("receiver is idle")
                .try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        ),
        "rejected announces are never published",
    );
    let counters = ep.routing_counters();
    assert_eq!(counters.freshness_replays_rejected, 1);
    assert_eq!(counters.freshness_stale_rejected, 1);
}

#[tokio::test]
async fn held_older_announce_cannot_publish_after_newer_direct_ingress() {
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xA1; 64]));
    hub.enable_routing();
    hub.set_relay_jitter(Duration::ZERO);
    let noisy = hub.attach_interface();
    let noisy_id = noisy.id();
    let noisy_sink = noisy.sink();
    let quiet = hub.attach_interface();
    let quiet_id = quiet.id();
    let quiet_sink = quiet.sink();
    let _egress = hub.attach_interface();
    let ingress_policy = AnnounceIngressPolicy {
        held_capacity: 4,
        frequency_window: Duration::from_secs(10),
        burst_hold: Duration::from_millis(200),
        burst_penalty: Duration::from_millis(200),
        held_release_interval: Duration::from_millis(1),
        new_interface_hz: 1,
        established_interface_hz: 1,
        destination_target: Duration::ZERO,
        ..AnnounceIngressPolicy::default()
    };
    hub.set_announce_ingress_policy(ingress_policy);

    // The first two unknown destinations prime the noisy interface. The third enters the
    // real held queue while remaining unknown to the address book and path table.
    for (index, (seed, name)) in [(0xA2, "freshness-prime-a"), (0xA3, "freshness-prime-b")]
        .into_iter()
        .enumerate()
    {
        let peer = PrivateIdentity::from_secret_bytes(&[seed; 64]);
        let (packet, _) = freshness_announce(&peer, name, 0, 1, 1, 1);
        assert!(noisy_sink.deliver(packet));
        let expected = index as u64 + 1;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while hub.shared.announce_sequence.load(Ordering::Relaxed) < expected
            && tokio::time::Instant::now() < deadline
        {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            hub.shared.announce_sequence.load(Ordering::Relaxed),
            expected
        );
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    let target_peer = PrivateIdentity::from_secret_bytes(&[0xA4; 64]);
    let (older_packet, older) =
        freshness_announce(&target_peer, "freshness-held-order", 0, 1, 10, 2);
    let destination = older.destination;
    assert!(noisy_sink.deliver(older_packet));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while hub.announce_ingress_counters(noisy_id).held == 0
        && tokio::time::Instant::now() < deadline
    {
        tokio::task::yield_now().await;
    }
    assert_eq!(hub.announce_ingress_counters(noisy_id).held, 1);
    assert!(hub.resolve(destination).is_none());
    hub.set_announce_ingress_policy(AnnounceIngressPolicy {
        new_interface_hz: 1_000,
        established_interface_hz: 1_000,
        ..ingress_policy
    });

    // A newer copy on the quiet interface is processed immediately. Reconfiguring the
    // retained bounds while the release task exists shares the same freshness guard.
    let (newer_packet, _) = freshness_announce(&target_peer, "freshness-held-order", 0, 2, 11, 1);
    assert!(quiet_sink.deliver(newer_packet));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while hub.resolve(destination).is_none() && tokio::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    assert!(hub.resolve(destination).is_some());
    hub.set_announce_freshness_policy(hub.announce_freshness_policy())
        .unwrap();
    let sequence_before_release = hub.shared.announce_sequence.load(Ordering::Relaxed);
    assert_eq!(sequence_before_release, 3);
    // The three accepted announces' first transmissions; retries are seconds away.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while hub.routing_counters().forwarded_announces < 3 && tokio::time::Instant::now() < deadline {
        tokio::task::yield_now().await;
    }
    let forwards_before_release = hub.routing_counters().forwarded_announces;
    assert_eq!(forwards_before_release, 3);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while hub.announce_ingress_counters(noisy_id).released == 0
        && tokio::time::Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(hub.announce_ingress_counters(noisy_id).released, 1);
    assert_eq!(hub.routing_counters().freshness_stale_rejected, 1);
    assert_eq!(
        hub.shared.announce_sequence.load(Ordering::Relaxed),
        sequence_before_release,
        "the deferred stale copy did not publish"
    );
    assert_eq!(
        hub.routing_counters().forwarded_announces,
        forwards_before_release,
        "the deferred stale copy did not relay"
    );
    assert_eq!(hub.route_to(destination), Some((quiet_id, 1)));
    assert_eq!(
        hub.shared
            .address_book
            .lock()
            .unwrap()
            .resolve(destination)
            .unwrap()
            .announces_seen,
        1
    );
}

#[tokio::test]
async fn newer_equal_and_worse_routes_replace_the_incumbent() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x43; 64]));
    let first_iface = ep.attach_interface().id();
    let equal_iface = ep.attach_interface().id();
    let worse_iface = ep.attach_interface().id();
    let peer = PrivateIdentity::from_secret_bytes(&[0x44; 64]);

    let (first_packet, first) = freshness_announce(&peer, "freshness-route", 0, 1, 10, 1);
    let destination = first.destination;
    process_verified_announce(&ep.shared, first_iface, first_packet, first);
    let _ = ep.next_announcement().await.unwrap();

    let (equal_packet, equal) = freshness_announce(&peer, "freshness-route", 0, 2, 11, 1);
    process_verified_announce(&ep.shared, equal_iface, equal_packet, equal);
    let _ = ep.next_announcement().await.unwrap();
    assert_eq!(ep.route_to(destination), Some((equal_iface, 1)));

    let (worse_packet, worse) = freshness_announce(&peer, "freshness-route", 0, 3, 12, 5);
    process_verified_announce(&ep.shared, worse_iface, worse_packet, worse);
    let _ = ep.next_announcement().await.unwrap();
    assert_eq!(ep.route_to(destination), Some((worse_iface, 5)));
}

/// An endpoint whose routes live a minute, so a test can age one past its TTL.
fn short_ttl_endpoint(seed: u8) -> Endpoint {
    Endpoint::with_announce_freshness_policy(
        PrivateIdentity::from_secret_bytes(&[seed; 64]),
        AnnounceFreshnessPolicy {
            route_ttl: Duration::from_secs(60),
            ..AnnounceFreshnessPolicy::default()
        },
    )
    .unwrap()
}

fn expire_route(ep: &Endpoint, destination: AddressHash) {
    let aged = Instant::now()
        .checked_sub(ep.shared.route_ttl())
        .expect("the monotonic clock predates one route TTL");
    ep.shared
        .path_table
        .lock()
        .unwrap()
        .get_mut(&destination)
        .expect("learned route")
        .learned = aged;
    assert_eq!(ep.route_to(destination), None, "route expired");
}

/// RNS culls a path row with its random blobs (`Transport.py` 957-978, 1086-1090), so the
/// next announce is a first sighting whatever its emission time or hops.
#[tokio::test]
async fn expired_routes_admit_any_announce_as_a_first_sighting() {
    let ep = short_ttl_endpoint(0x45);
    let first_iface = ep.attach_interface().id();
    let replacement_iface = ep.attach_interface().id();
    let better_peer = PrivateIdentity::from_secret_bytes(&[0x46; 64]);
    let equal_peer = PrivateIdentity::from_secret_bytes(&[0x47; 64]);
    let worse_peer = PrivateIdentity::from_secret_bytes(&[0x48; 64]);
    let cases = [
        (&better_peer, "freshness-expired-better", 1_u8),
        (&equal_peer, "freshness-expired-equal", 2_u8),
        (&worse_peer, "freshness-expired-worse", 3_u8),
    ];

    for (peer, name, hops) in cases {
        let (first_packet, first) = freshness_announce(peer, name, 0, 1, 10, 2);
        let destination = first.destination;
        process_verified_announce(&ep.shared, first_iface, first_packet, first);
        let _ = ep.next_announcement().await.unwrap();
        expire_route(&ep, destination);

        let (packet, older) = freshness_announce(peer, name, 0, 2, 9, hops);
        process_verified_announce(&ep.shared, replacement_iface, packet, older);
        let accepted = ep.next_announcement().await.unwrap();
        assert_eq!((accepted.destination, accepted.hops), (destination, hops));
        assert_eq!(ep.route_to(destination), Some((replacement_iface, hops)));
    }
    assert_eq!(ep.routing_counters().freshness_stale_rejected, 0);
}

/// A transport answering `request_path` from its cache sends the blob it already relayed
/// (`Transport.py` 3459-3530). While the route lives that is a replay; once it has gone,
/// the same blob restores it.
#[tokio::test]
async fn a_cached_path_response_restores_an_expired_route() {
    let ep = short_ttl_endpoint(0x4B);
    let first_iface = ep.attach_interface().id();
    let answer_iface = ep.attach_interface().id();
    let peer = PrivateIdentity::from_secret_bytes(&[0x4C; 64]);
    let (packet, announced) = freshness_announce(&peer, "freshness-restore", 0, 1, 10, 2);
    let destination = announced.destination;
    let (cached_packet, cached) = freshness_announce(
        &peer,
        "freshness-restore",
        crate::path::CTX_PATH_RESPONSE,
        1,
        10,
        3,
    );
    process_verified_announce(&ep.shared, first_iface, packet.clone(), announced.clone());
    let _ = ep.next_announcement().await.unwrap();

    process_verified_announce(
        &ep.shared,
        answer_iface,
        cached_packet.clone(),
        cached.clone(),
    );
    assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);
    assert_eq!(ep.route_to(destination), Some((first_iface, 2)));

    expire_route(&ep, destination);
    process_verified_announce(&ep.shared, answer_iface, cached_packet, cached);
    let restored = ep.next_announcement().await.unwrap();
    assert_eq!((restored.destination, restored.sequence), (destination, 2));
    assert_eq!(ep.route_to(destination), Some((answer_iface, 3)));
    assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);

    process_verified_announce(&ep.shared, first_iface, packet, announced);
    assert_eq!(
        ep.routing_counters().freshness_replays_rejected,
        2,
        "the restored route refuses the blob again"
    );
}

/// A refused identity publishes no `PeerAnnounce`, but its path is learned and its
/// freshness committed: a book's capacity never decides what the router can reach.
#[tokio::test]
async fn address_book_refusal_still_learns_the_path() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x47; 64]));
    let iface = ep.attach_interface().id();
    let peer = PrivateIdentity::from_secret_bytes(&[0x48; 64]);
    let (packet, announcement) = freshness_announce(&peer, "freshness-refusal", 0, 1, 10, 1);
    let destination = announcement.destination;
    *ep.shared.address_book.lock().unwrap() = AddressBook::with_max_peers(0);
    process_verified_announce(&ep.shared, iface, packet.clone(), announcement.clone());
    assert_eq!(ep.routing_counters().refused_announces, 1);
    assert_eq!(ep.shared.announce_sequence.load(Ordering::Relaxed), 0);
    assert_eq!(ep.route_to(destination), Some((iface, 1)));
    assert!(ep.resolve(destination).is_none());

    *ep.shared.address_book.lock().unwrap() = AddressBook::with_max_peers(1);
    process_verified_announce(&ep.shared, iface, packet, announcement);
    assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);
}

/// A full book admits a newcomer by evicting the least recently heard peer whose path
/// has gone stale, and the newcomer is relayed whether or not the book takes it.
#[tokio::test]
async fn a_full_address_book_evicts_a_peer_whose_path_expired() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4B; 64]));
    let a = ep.attach_interface();
    ep.enable_routing();
    *ep.shared.address_book.lock().unwrap() = AddressBook::with_max_peers(2);
    let announcements: Vec<_> = (0..4u8)
        .map(|i| {
            let peer = PrivateIdentity::from_secret_bytes(&[0x60 + i; 64]);
            freshness_announce(&peer, "eviction", 0, i, 10, 1)
        })
        .collect();
    let relayed = |destination: AddressHash| {
        ep.shared
            .rebroadcasts
            .lock()
            .unwrap()
            .scheduled(destination)
    };

    for (packet, announcement) in &announcements[..3] {
        process_verified_announce(&ep.shared, a.id(), packet.clone(), announcement.clone());
        assert!(relayed(announcement.destination));
    }
    let held = |i: usize| ep.resolve(announcements[i].1.destination).is_some();
    assert!(held(0) && held(1) && !held(2));
    assert_eq!(ep.routing_counters().refused_announces, 1);
    assert_eq!(ep.shared.announce_sequence.load(Ordering::Relaxed), 2);

    // Every path is now past its TTL, so nothing protects the held peers.
    ep.shared.route_ttl_ms.store(0, Ordering::Relaxed);
    let (packet, announcement) = announcements[3].clone();
    process_verified_announce(&ep.shared, a.id(), packet, announcement.clone());
    assert!(relayed(announcement.destination));
    assert_eq!(ep.next_announcement().await.unwrap().sequence, 1);
    assert_eq!(ep.next_announcement().await.unwrap().sequence, 2);
    assert_eq!(
        ep.next_announcement().await.unwrap().destination,
        announcement.destination
    );
    assert!(held(3));
    assert_eq!(usize::from(held(0)) + usize::from(held(1)), 1);
    assert_eq!(ep.shared.address_book.lock().unwrap().evicted(), 1);
}

#[tokio::test]
async fn freshness_capacity_eviction_is_visible() {
    let policy = AnnounceFreshnessPolicy {
        destination_capacity: 1,
        blob_capacity: 1,
        ..AnnounceFreshnessPolicy::default()
    };
    let ep = Endpoint::with_announce_freshness_policy(
        PrivateIdentity::from_secret_bytes(&[0x49; 64]),
        policy,
    )
    .unwrap();
    let iface = ep.attach_interface().id();
    let peer = PrivateIdentity::from_secret_bytes(&[0x4A; 64]);
    let (a_packet, a) = freshness_announce(&peer, "freshness-capacity-a", 0, 1, 10, 1);
    let (renewed_packet, renewed) = freshness_announce(&peer, "freshness-capacity-a", 0, 3, 11, 1);
    let (b_packet, b) = freshness_announce(&peer, "freshness-capacity-b", 0, 2, 10, 1);
    process_verified_announce(&ep.shared, iface, a_packet, a);
    let _ = ep.next_announcement().await.unwrap();
    process_verified_announce(&ep.shared, iface, renewed_packet, renewed);
    let _ = ep.next_announcement().await.unwrap();
    process_verified_announce(&ep.shared, iface, b_packet, b);
    let _ = ep.next_announcement().await.unwrap();
    assert_eq!(ep.routing_counters().freshness_rows_evicted, 1);
    assert_eq!(ep.routing_counters().freshness_blobs_evicted, 1);
}
