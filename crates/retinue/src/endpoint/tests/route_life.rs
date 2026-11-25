//! Route life as stock RNS 1.6.0: what marks a route used, and what moves its expiry.

use super::*;

/// An accepted announce writes a new route, keeping the used mark of the one it replaces
/// (RNS `Transport.py` 2510, 2726). A new destination starts never used.
#[tokio::test]
async fn an_announce_that_replaces_a_route_keeps_its_used_mark() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x64; 64]));
    let a = ep.attach_interface();
    let entry = |dest| ep.shared.path_table.lock().unwrap()[&dest];
    let [dest, fresh] = [0xB4, 0xB5].map(|n| AddressHash::from_bytes([n; 16]));
    let aged = Instant::now() - Duration::from_secs(60);
    ep.shared.learn_path_at(dest, a.id(), 1, None, aged);
    assert_eq!(entry(dest).used, None, "a new route is unused");
    ep.shared
        .queue_single(dest, single_packet(dest, vec![1; 64]));
    let marked = entry(dest).used;
    assert!(marked.is_some());
    let later = aged + Duration::from_secs(1);
    ep.shared.learn_path_at(dest, a.id(), 2, None, later);
    let route = entry(dest);
    assert_eq!((route.learned, route.used, route.hops), (later, marked, 2));
    ep.shared.learn_path_at(fresh, a.id(), 1, None, later);
    assert_eq!(entry(fresh).used, None);
}

/// Carrying a packet across a bridge marks the route of the bridge's destination used once
/// the send is accepted (RNS `Transport.py` 2440), and moves nothing else.
#[tokio::test]
async fn carrying_across_a_bridge_marks_the_destination_route_used() {
    for accepted in [false, true] {
        let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x73; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let dest = AddressHash::from_bytes([0x7d; 16]);
        let aged = Instant::now() - Duration::from_secs(10);
        ep.shared.learn_path_at(dest, b.id(), 0, None, aged);
        let id = AddressHash::from_bytes([0x74; 16]);
        ep.shared.link_transport.lock().unwrap().insert(
            id,
            LinkBridge {
                from: a.id(),
                out: b.id(),
                destination: dest,
                seen: Instant::now(),
                proof_deadline: None,
            },
        );
        let mut data = single_packet(id, vec![0x73; 48]);
        data.destination_type = DestinationType::Link;
        if !accepted {
            while ep
                .shared
                .try_send_on_class(b.id(), data.clone(), TrafficClass::Transit)
            {}
        }
        route(&ep.shared, a.id(), data);
        let route = ep.shared.path_table.lock().unwrap()[&dest];
        assert_eq!(route.used.is_some(), accepted, "accepted: {accepted}");
        assert_eq!(route.learned, aged);
    }
}

/// Carrying a link proof whose signature this node verified proves the destination alive
/// once the proof is sent (RNS `Transport.py` 2928-2938): the route's life starts over in its
/// interface's current mode. A proof passed on the side check alone, the destination's
/// identity unknown, marks the route used and moves nothing else.
#[tokio::test]
async fn a_verified_carried_link_proof_proves_its_destination_alive() {
    for (known, accepted) in [(true, true), (true, false), (false, true)] {
        let (ep, a, b, responder, _, request) = super::transit::transit_fixture();
        let dest = request.destination;
        if !known {
            ep.shared.address_book.lock().unwrap().forget(dest);
        }
        let aged = Instant::now() - Duration::from_secs(60);
        ep.shared
            .path_table
            .lock()
            .unwrap()
            .get_mut(&dest)
            .unwrap()
            .learned = aged;
        assert!(ep.set_interface_mode(b.id(), InterfaceMode::AccessPoint));
        route(&ep.shared, a.id(), request);
        let carried = b.outbound.queues.pop().unwrap();
        b.outbound.queues.delivery_complete();
        let trailer = link::request_trailer(&carried).unwrap().unwrap();
        let (_, proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();
        if !accepted {
            while ep
                .shared
                .try_send_on_class(a.id(), proof.clone(), TrafficClass::Transit)
            {}
        }
        route(&ep.shared, b.id(), proof);
        let route = ep.shared.path_table.lock().unwrap()[&dest];
        let case = std::format!("known {known}, accepted {accepted}");
        if known && accepted {
            assert!(route.learned > aged, "{case}");
            assert_eq!(route.used, Some(route.learned), "{case}");
            assert_eq!(route.mode, InterfaceMode::AccessPoint, "{case}");
        } else {
            assert_eq!(route.learned, aged, "{case}");
            assert_eq!(route.mode, InterfaceMode::default(), "{case}");
        }
    }
}

/// Establishing a link as its initiator proves the destination alive (RNS `Link.py` 513,
/// `Transport.py` 3632-3641): the route's life starts over in its interface's current mode,
/// and it is marked used.
#[tokio::test]
async fn an_own_link_proves_its_destination_alive() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x65; 64]));
    let a = ep.attach_interface();
    let responder = PrivateIdentity::from_secret_bytes(&[0x66; 64]);
    let (_, announce) = freshness_announce(&responder, "alive", 0, 1, 1, 0);
    let dest = announce.destination;
    let aged = Instant::now() - Duration::from_secs(60);
    ep.shared.learn_path_at(dest, a.id(), 1, None, aged);
    assert!(ep.set_interface_mode(a.id(), InterfaceMode::Roaming));
    let answer = async {
        let request = loop {
            if let Some(packet) = a.outbound.queues.pop() {
                break packet;
            }
            tokio::task::yield_now().await;
        };
        a.outbound.queues.delivery_complete();
        let trailer = link::request_trailer(&request).unwrap().unwrap();
        let (_, proof) = link::accept(&request, &responder, &[0x67; 64], trailer).unwrap();
        route(&ep.shared, a.id(), proof);
    };
    let (established, ()) = tokio::join!(ep.establish(dest, *responder.public()), answer);
    established.unwrap();
    let route = ep.shared.path_table.lock().unwrap()[&dest];
    assert!(route.learned > aged);
    assert_eq!(route.used, Some(route.learned));
    assert_eq!(route.mode, InterfaceMode::Roaming);
}

/// A full table drops, after the routes past their lifetime, the never-used routes, the one
/// that would expire soonest first, and then the route used longest ago (D30).
#[tokio::test]
async fn a_full_table_drops_never_used_then_least_recently_used() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x77; 64]));
    let a = ep.attach_interface();
    let peers = [1, 2, 3, 4, 5, 6, 7].map(|n| AddressHash::from_bytes([n; 16]));
    let aged = Instant::now() - Duration::from_secs(60);
    let at = |s: u64| aged + Duration::from_secs(s);
    let learn = |n: usize, s| ep.shared.learn_path_at(peers[n], a.id(), 1, None, at(s));
    let use_at = |n: usize, s| {
        ep.shared
            .path_table
            .lock()
            .unwrap()
            .get_mut(&peers[n])
            .unwrap()
            .used = Some(at(s));
    };
    let held = || {
        let table = ep.shared.path_table.lock().unwrap();
        let mut held: Vec<_> = (0..peers.len())
            .filter(|n| table.contains_key(&peers[*n]))
            .collect();
        held.sort_unstable();
        held
    };
    for n in 0..PATH_TABLE_CAPACITY {
        learn(n, n as u64);
    }
    use_at(0, 10);
    learn(4, 11);
    assert_eq!(
        held(),
        [0, 2, 3, 4],
        "the never-used route learned first goes"
    );
    use_at(2, 12);
    use_at(3, 13);
    learn(5, 14);
    assert_eq!(
        held(),
        [0, 2, 3, 5],
        "a never-used route goes before used ones"
    );
    use_at(5, 15);
    learn(6, 16);
    assert_eq!(held(), [2, 3, 5, 6], "then the route used longest ago");
    assert_eq!(ep.routing_counters().paths_evicted, 3);
}

/// The desktop culls a never-used route once it outlives stock's linger (D29;
/// `Transport.py` 228-233, 1145): 12 minutes on a leaf, 1.75 hours on a transport node, and
/// ten times that on an interface slower than 25 kbit/s. A used route keeps its lifetime.
#[tokio::test]
async fn a_never_used_route_is_culled_after_its_linger() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x78; 64]));
    let fast = ep.attach_interface();
    let slow = ep.attach_interface();
    assert!(ep.set_interface_bitrate(slow.id(), Some(24_999)));
    let learned = Instant::now();
    let ms = Duration::from_millis(1);
    let [a, b, c, d, e] = [0xC1, 0xC2, 0xC3, 0xC4, 0xC5].map(|n| AddressHash::from_bytes([n; 16]));
    for (dest, iface) in [(a, fast.id()), (b, slow.id()), (c, fast.id())] {
        ep.shared.learn_path_at(dest, iface, 1, None, learned);
    }
    ep.shared
        .path_table
        .lock()
        .unwrap()
        .get_mut(&c)
        .unwrap()
        .used = Some(learned);

    let leaf = Duration::from_secs(12 * 60);
    assert!(ep.route_to_at(a, learned + leaf - ms).is_some());
    assert!(
        ep.route_to_at(a, learned + leaf).is_none(),
        "a leaf lingers 12 minutes"
    );
    assert!(ep.route_to_at(b, learned + leaf * 10 - ms).is_some());
    assert!(
        ep.route_to_at(b, learned + leaf * 10).is_none(),
        "ten times on a slow link"
    );
    assert!(
        ep.route_to_at(c, learned + leaf * 10).is_some(),
        "a used route keeps its lifetime"
    );

    ep.enable_routing();
    let transport = Duration::from_secs(105 * 60);
    for dest in [d, e] {
        ep.shared.learn_path_at(dest, fast.id(), 1, None, learned);
    }
    assert!(ep.route_to_at(d, learned + transport - ms).is_some());
    assert!(
        ep.route_to_at(d, learned + transport).is_none(),
        "a transport node lingers 1.75 hours"
    );
    assert!(
        ep.route_facts_at(learned + transport)
            .iter()
            .all(|fact| fact.destination != e),
        "facts age routes by the same rule"
    );
}

/// An interface is slow by its configured bitrate, else by its first-hop airtime allowance:
/// over 160 ms for one 500-byte MTU is under 25 kbit/s. With neither it is not slow.
#[tokio::test]
async fn a_slow_interface_is_known_by_bitrate_else_airtime() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x79; 64]));
    let airtime = |ms| {
        let iface = ep.attach_interface();
        ep.set_first_hop_airtime(iface.id(), Duration::from_millis(ms));
        iface
    };
    let over = airtime(161);
    let at = airtime(160);
    let neither = ep.attach_interface();
    let fast_bitrate = airtime(1_000);
    assert!(ep.set_interface_bitrate(fast_bitrate.id(), Some(100_000)));
    let slow_bitrate = airtime(1);
    assert!(ep.set_interface_bitrate(slow_bitrate.id(), Some(1_200)));
    let learned = Instant::now();
    let cases = [
        (&over, true),
        (&at, false),
        (&neither, false),
        (&fast_bitrate, false),
        (&slow_bitrate, true),
    ];
    let leaf = Duration::from_secs(12 * 60);
    // One case at a time: five routes would overflow the test table of four.
    for (n, (iface, slow)) in cases.iter().enumerate() {
        let dest = AddressHash::from_bytes([0xD0 + n as u8; 16]);
        ep.shared.learn_path_at(dest, iface.id(), 1, None, learned);
        let lingers = ep.route_to_at(dest, learned + leaf).is_some();
        assert_eq!(lingers, *slow, "case {n}: slow {slow}");
        ep.shared.forget_path(dest);
    }
}
