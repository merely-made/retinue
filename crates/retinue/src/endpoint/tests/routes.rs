//! The path table: expiry, capacity, transports, refresh, and diagnostics.

use super::*;

#[tokio::test]
async fn a_learned_route_expires_and_is_evicted() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[1u8; 64]));
    let dest = AddressHash::from_bytes([0xAB; 16]);
    let learned = Instant::now();
    ep.shared.learn_path_at(dest, 7, 2, None, learned);
    assert_eq!(
        ep.route_to_at(dest, learned),
        Some((7, 2)),
        "a fresh route is returned"
    );

    assert_eq!(
        ep.route_to_at(dest, learned + ep.shared.route_ttl()),
        None,
        "an expired route is not returned"
    );
    assert!(
        !ep.shared.path_table.lock().unwrap().contains_key(&dest),
        "and is evicted on lookup",
    );
}

#[tokio::test]
async fn route_facts_are_ordered_current_and_read_only() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x12; 64]));
    let interface = ep.attach_interface().id();
    let later = AddressHash::from_bytes([0xBB; 16]);
    let earlier = AddressHash::from_bytes([0x11; 16]);
    let transport = AddressHash::from_bytes([0x77; 16]);
    ep.shared.learn_path(later, interface, 3, Some(transport));
    ep.shared.learn_path(earlier, interface, 1, None);
    let learned = ep
        .shared
        .path_table
        .lock()
        .unwrap()
        .values()
        .map(|entry| entry.learned)
        .max()
        .unwrap();

    let facts = ep.route_facts_at(learned);
    assert_eq!(
        facts
            .iter()
            .map(|fact| fact.destination)
            .collect::<Vec<_>>(),
        vec![earlier, later]
    );
    assert!(facts.iter().all(|fact| fact.interface == interface));
    assert_eq!(facts[1].transport, Some(transport));

    let before = ep.shared.path_table.lock().unwrap().len();
    assert!(
        ep.route_facts_at(learned + ep.shared.route_ttl())
            .is_empty()
    );
    assert_eq!(
        ep.shared.path_table.lock().unwrap().len(),
        before,
        "diagnostic capture must not evict expired routes",
    );
}

#[tokio::test]
async fn diagnostic_capture_waits_for_an_inflight_writer() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x19; 64]));
    let initial_generation = ep.diagnostic_generation();
    let destination = AddressHash::from_bytes([0x66; 16]);
    let (writer_entered_tx, writer_entered_rx) = std::sync::mpsc::channel();
    let (release_writer_tx, release_writer_rx) = std::sync::mpsc::channel();
    let writer_shared = Arc::clone(&ep.shared);
    let writer = std::thread::spawn(move || {
        writer_shared.write_diagnostic(|| {
            writer_entered_tx.send(()).unwrap();
            release_writer_rx.recv().unwrap();
            writer_shared.path_table.lock().unwrap().insert(
                destination,
                PathEntry {
                    iface: 0,
                    transport: None,
                    hops: 1,
                    learned: Instant::now(),
                    mode: InterfaceMode::Full,
                },
            );
            ((), true)
        });
    });

    writer_entered_rx.recv().unwrap();
    let (capture_started_tx, capture_started_rx) = std::sync::mpsc::channel();
    let (capture_done_tx, capture_done_rx) = std::sync::mpsc::channel();
    let capture_shared = Arc::clone(&ep.shared);
    let capture = std::thread::spawn(move || {
        capture_started_tx.send(()).unwrap();
        let result =
            capture_shared.capture_diagnostic(|| capture_shared.path_table.lock().unwrap().len());
        capture_done_tx.send(result).unwrap();
    });

    capture_started_rx.recv().unwrap();
    assert!(
        capture_done_rx
            .recv_timeout(Duration::from_millis(50))
            .is_err(),
        "capture must not pass a writer holding the revision barrier",
    );

    release_writer_tx.send(()).unwrap();
    writer.join().unwrap();
    let (generation, route_count) = capture_done_rx
        .recv_timeout(Duration::from_secs(1))
        .unwrap();
    capture.join().unwrap();

    assert_eq!(route_count, 1);
    assert_eq!(generation, initial_generation + 1);
    assert_eq!(generation, ep.diagnostic_generation());
}

/// The path table is capped, and forgets the peer that has gone quietest.
#[tokio::test]
async fn a_full_path_table_forgets_the_quietest_peer() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x34; 64]));
    let iface = InterfaceId::from(1_u32);
    let quiet = AddressHash::from_bytes([0x01; 16]);

    // The quiet one is learned first, so its `learned` is oldest.
    ep.shared.learn_path(quiet, iface, 1, None);
    for n in 2..=PATH_TABLE_CAPACITY as u8 {
        ep.shared
            .learn_path(AddressHash::from_bytes([n; 16]), iface, 1, None);
    }
    assert_eq!(
        ep.shared.path_table.lock().unwrap().len(),
        PATH_TABLE_CAPACITY
    );

    // One more destination than the table holds.
    let newcomer = AddressHash::from_bytes([0xFE; 16]);
    ep.shared.learn_path(newcomer, iface, 1, None);

    let table = ep.shared.path_table.lock().unwrap();
    assert_eq!(table.len(), PATH_TABLE_CAPACITY, "the bound holds");
    assert!(table.contains_key(&newcomer), "the newcomer is learned");
    assert!(
        !table.contains_key(&quiet),
        "and the peer that had gone quietest is what made room",
    );
    drop(table);
    assert_eq!(
        ep.routing_counters().paths_evicted,
        1,
        "evictions are counted, not silent",
    );
}

/// A peer that keeps announcing keeps its route: re-announcing refreshes `learned`.
#[tokio::test]
async fn a_peer_that_keeps_announcing_keeps_its_route() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x35; 64]));
    let iface = InterfaceId::from(1_u32);
    let talkative = AddressHash::from_bytes([0x01; 16]);

    ep.shared.learn_path(talkative, iface, 1, None);
    for n in 2..=PATH_TABLE_CAPACITY as u8 {
        ep.shared
            .learn_path(AddressHash::from_bytes([n; 16]), iface, 1, None);
    }
    // It re-announces, which moves it off oldest.
    ep.shared.learn_path(talkative, iface, 1, None);

    ep.shared
        .learn_path(AddressHash::from_bytes([0xFE; 16]), iface, 1, None);

    let table = ep.shared.path_table.lock().unwrap();
    assert!(
        table.contains_key(&talkative),
        "a peer still announcing must not be the one forgotten",
    );
}

/// One radio reaches different destinations through different transport nodes: A via X
/// and B via Y on one interface must each keep their own.

#[tokio::test]
async fn two_destinations_on_one_interface_keep_their_own_transports() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x31; 64]));
    let iface = InterfaceId::from(3_u32);
    let a = AddressHash::from_bytes([0xAA; 16]);
    let b = AddressHash::from_bytes([0xBB; 16]);
    let via_x = AddressHash::from_bytes([0x11; 16]);
    let via_y = AddressHash::from_bytes([0x22; 16]);

    ep.shared.learn_path(a, iface, 1, Some(via_x));
    ep.shared.learn_path(b, iface, 1, Some(via_y));

    let addressed = |dest| {
        let mut pkt = crate::path::path_request(dest, &[0x5A; 16]);
        pkt.destination = dest;
        ep.shared.address_for(iface, pkt).transport
    };
    assert_eq!(addressed(a), Some(via_x), "A must still route through X");
    assert_eq!(addressed(b), Some(via_y), "B routes through Y");
}

/// Once freshness admits an announce, it is the current route even when the predecessor
/// had fewer hops: freshness owns ordering.
#[tokio::test]
async fn a_newer_worse_route_replaces_the_incumbent() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x32; 64]));
    let dest = AddressHash::from_bytes([0xCC; 16]);
    let good = InterfaceId::from(1_u32);
    let worse = InterfaceId::from(2_u32);

    let learned = Instant::now();
    ep.shared.learn_path_at(dest, good, 1, None, learned);
    ep.shared.learn_path_at(
        dest,
        worse,
        5,
        Some(AddressHash::from_bytes([0xA5; 16])),
        learned + Duration::from_millis(1),
    );

    let entry = *ep.shared.path_table.lock().unwrap().get(&dest).unwrap();
    assert_eq!(entry.iface, worse, "the newer route becomes incumbent");
    assert_eq!(entry.hops, 5);
    assert_eq!(
        entry.transport,
        Some(AddressHash::from_bytes([0xA5; 16])),
        "all route facts come from the accepted announce",
    );
}

/// Routes default to RNS's one-week lifetime, and an access-point or roaming interface
/// bounds the routes learned on it to a day or six hours.
#[tokio::test]
async fn routes_live_a_week_unless_their_interface_mode_shortens_them() {
    assert_eq!(
        AnnounceFreshnessPolicy::default().route_ttl,
        Duration::from_secs(7 * 24 * 60 * 60)
    );
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x61; 64]));
    let full = ep.attach_interface();
    let roaming = ep.attach_interface();
    let access_point = ep.attach_interface();
    assert!(ep.set_interface_mode(roaming.id(), InterfaceMode::Roaming));
    assert!(ep.set_interface_mode(access_point.id(), InterfaceMode::AccessPoint));
    assert!(!ep.set_interface_mode(u32::MAX, InterfaceMode::Roaming));

    let learned = Instant::now();
    let hours = |h: u64| Duration::from_secs(h * 60 * 60);
    let [a, b, c] = [0xA1, 0xA2, 0xA3].map(|n| AddressHash::from_bytes([n; 16]));
    ep.shared.learn_path_at(a, full.id(), 1, None, learned);
    ep.shared.learn_path_at(b, roaming.id(), 1, None, learned);
    ep.shared
        .learn_path_at(c, access_point.id(), 1, None, learned);

    let just_before = |h| learned + hours(h) - Duration::from_millis(1);
    assert!(ep.route_to_at(b, just_before(6)).is_some());
    assert!(ep.route_to_at(b, learned + hours(6)).is_none());
    assert!(ep.route_to_at(c, just_before(24)).is_some());
    assert!(ep.route_to_at(c, learned + hours(24)).is_none());
    assert!(ep.route_to_at(a, just_before(7 * 24)).is_some());
    assert!(ep.route_to_at(a, learned + hours(7 * 24)).is_none());
}

/// A route carrying traffic is refreshed, as RNS refreshes a path when it inserts a packet
/// into transport or forwards transit along it. A direct local send does not refresh.
#[tokio::test]
async fn a_route_in_use_is_refreshed() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x62; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    let aged = Instant::now()
        .checked_sub(Duration::from_secs(60 * 60))
        .expect("the monotonic clock predates an hour");
    let learned = |dest| ep.shared.path_table.lock().unwrap()[&dest].learned;
    let [via_relay, direct, onward] = [0xB1, 0xB2, 0xB3].map(|n| AddressHash::from_bytes([n; 16]));
    let relay = AddressHash::from_bytes([0xBF; 16]);
    ep.shared
        .learn_path_at(via_relay, a.id(), 2, Some(relay), aged);
    ep.shared.learn_path_at(direct, a.id(), 0, None, aged);
    ep.shared.learn_path_at(onward, b.id(), 0, None, aged);

    ep.shared
        .queue_single(via_relay, single_packet(via_relay, vec![1; 64]));
    assert_eq!(a.outbound.queues.pop().unwrap().transport, Some(relay));
    a.outbound.queues.delivery_complete();
    assert!(
        learned(via_relay) > aged,
        "inserting into transport refreshes"
    );

    ep.shared
        .queue_single(direct, single_packet(direct, vec![2; 64]));
    assert!(a.outbound.queues.pop().is_some());
    a.outbound.queues.delivery_complete();
    assert_eq!(learned(direct), aged, "a direct send does not");

    let mut transit = single_packet(onward, vec![3; 64]);
    transit.header_type = crate::packet::HeaderType::Type2;
    transit.transport = Some(ep.identity().hash());
    route(&ep.shared, a.id(), transit);
    assert!(b.outbound.queues.pop().is_some());
    assert!(learned(onward) > aged, "carrying transit refreshes");
}

/// Detaching an interface culls the routes learned on it and the bridges that cross it, so
/// a send to that destination falls back to broadcast on what is left.
#[tokio::test]
async fn detaching_an_interface_culls_its_routes_and_bridges() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x63; 64]));
    let gone = ep.attach_interface();
    let kept = ep.attach_interface();
    let dest = AddressHash::from_bytes([0xC1; 16]);
    ep.shared.learn_path(
        dest,
        gone.id(),
        1,
        Some(AddressHash::from_bytes([0xCF; 16])),
    );
    let [crossing, beside] = [0xC2, 0xC3].map(|n| AddressHash::from_bytes([n; 16]));
    {
        let mut bridges = ep.shared.link_transport.lock().unwrap();
        let bridge = |from, out| LinkBridge {
            from,
            out,
            destination: AddressHash::from_bytes([0xCF; 16]),
            seen: Instant::now(),
            proof_deadline: None,
        };
        bridges.insert(crossing, bridge(kept.id(), gone.id()));
        bridges.insert(beside, bridge(kept.id(), kept.id()));
    }

    ep.detach_interface(gone.id());
    assert_eq!(ep.route_to(dest), None);
    let bridges = ep.shared.link_transport.lock().unwrap().clone();
    assert!(!bridges.contains_key(&crossing));
    assert!(bridges.contains_key(&beside));

    let result = ep
        .shared
        .queue_single(dest, single_packet(dest, vec![4; 64]));
    assert_eq!(result.queued, 1, "broadcast on the remaining interface");
    let sent = kept.outbound.queues.pop().expect("broadcast reached it");
    assert_eq!(sent.header_type, crate::packet::HeaderType::Type1);
}
