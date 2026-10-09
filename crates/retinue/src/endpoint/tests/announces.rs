//! Announce timebases, key conflicts, observations, echoes, and relay admission.

use super::*;
use crate::announce_admission::{AnnounceAdmission, DestinationVerdict};
use crate::endpoint::{AnnounceRate, InterfaceSelector, RoutingPolicy};

use super::super::announces::{HeldAnnounce, HeldAnnounces};

/// An announce naming a known destination under another key is rejected whole: the
/// known key stays, and no route or announcement follows (RNS `Identity.py` 569-577).
#[tokio::test]
async fn an_announce_with_a_different_key_for_a_known_destination_is_rejected() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xB5; 64]));
    let wire = ep.attach_interface();
    let (packet, announce) = peer_announce(0xB6, "mismatch");
    // A real impostor would need a hash collision; stand one in by swapping the key.
    let mut known = announce.clone();
    known.identity = *PrivateIdentity::from_secret_bytes(&[0xB7; 64]).public();
    ep.shared.address_book.lock().unwrap().ingest(&known);

    assert!(wire.sink().deliver(packet));
    tokio::time::timeout(Duration::from_secs(1), async {
        while ep.routing_counters().key_mismatch_announces == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the mismatch is counted");
    assert_eq!(ep.resolve(announce.destination), Some(known.identity));
    assert_eq!(ep.route_to(announce.destination), None);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), ep.next_announcement())
            .await
            .is_err()
    );
}

fn emitted_timebase(packet: &Packet) -> u64 {
    AnnounceBlob::from_wire(
        Announce::decode(packet)
            .expect("locally emitted announce verifies")
            .rand_hash,
    )
    .timebase()
}

#[tokio::test]
async fn endpoint_announce_advances_within_one_source_second() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x90; 64]));
    let name = DestinationName::new("retinue", ["endpoint-same-second"]);

    let first = ep.build_announce_at(&name, b"cap", 4_000);
    let second = ep.build_announce_at(&name, b"cap", 4_000);

    assert_eq!(emitted_timebase(&first), 4_000);
    assert_eq!(emitted_timebase(&second), 4_001);
}

#[tokio::test]
async fn endpoint_announce_ignores_a_backward_source_clock() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x91; 64]));
    let name = DestinationName::new("retinue", ["endpoint-backward-clock"]);

    let first = ep.build_announce_at(&name, b"cap", 9_000);
    let second = ep.build_announce_at(&name, b"cap", 8_999);

    assert_eq!(emitted_timebase(&first), 9_000);
    assert_eq!(emitted_timebase(&second), 9_001);
}

#[tokio::test]
async fn endpoint_and_owned_path_response_keep_timebases_per_destination() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x92; 64]));
    let first_name = DestinationName::new("retinue", ["endpoint-first"]);
    let second_name = DestinationName::new("retinue", ["endpoint-second"]);
    let second_destination = second_name.destination_hash(ep.identity());
    ep.shared.registered.lock().unwrap().push(Registered {
        dest: second_destination,
        kind: RegistrationKind::BestEffort,
        name: second_name.clone(),
        app_data: b"path-cap".to_vec(),
        app_data_source: None,
        ratchets: None,
        enforce_ratchets: false,
        proof_strategy: ProofStrategy::None,
    });

    let first = ep.build_announce_at(&first_name, b"first-cap", 700);
    let path_response = ep
        .shared
        .path_response_at(second_destination, 700)
        .expect("owned destination answers a path request");
    let first_again = ep.build_announce_at(&first_name, b"first-cap", 700);
    let path_response_again = ep
        .shared
        .path_response_at(second_destination, 700)
        .expect("owned destination answers a second path request");

    assert_eq!(emitted_timebase(&first), 700);
    assert_eq!(emitted_timebase(&path_response), 700);
    assert_eq!(emitted_timebase(&first_again), 701);
    assert_eq!(emitted_timebase(&path_response_again), 701);
    assert_eq!(path_response.context, crate::path::CTX_PATH_RESPONSE);
}

/// A destination with an app data source answers each path request with app data built at
/// that response's time (RNS's callable default app data).
#[tokio::test]
async fn a_path_response_builds_app_data_from_the_registered_source() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x93; 64]));
    let name = DestinationName::new("retinue", ["app-data-source"]);
    let dest = name.destination_hash(ep.identity());
    ep.register(name.clone(), b"fixed");
    let app_data = |seconds| {
        let packet = ep.shared.path_response_at(dest, seconds).unwrap();
        Announce::decode(&packet).unwrap().app_data
    };
    assert_eq!(app_data(800), b"fixed");

    ep.set_app_data_source(&name, |seconds| seconds.to_be_bytes().to_vec())
        .unwrap();
    assert_eq!(app_data(801), 801_u64.to_be_bytes());
    assert_eq!(app_data(900), 900_u64.to_be_bytes());

    let unregistered = DestinationName::new("retinue", ["nowhere"]);
    assert!(
        ep.set_app_data_source(&unregistered, |_| Vec::new())
            .is_err()
    );
}

#[tokio::test]
async fn announce_facts_retain_ingress_route_and_observation_order() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x13; 64]));
    let interface = ep.attach_interface().id();
    let peer = PrivateIdentity::from_secret_bytes(&[0x14; 64]);
    let name = DestinationName::new("retinue", ["management-fact"]);
    let blob = AnnounceBlob::from_wire([0x22; 10]);
    let mut packet = announce::build(&peer, name.name_hash(), &blob, None, b"opaque");
    packet.hops = 2;
    packet.header_type = crate::packet::HeaderType::Type2;
    packet.transport = Some(AddressHash::from_bytes([0x55; 16]));
    let decoded = Announce::decode(&packet).unwrap();
    let destination = decoded.destination;

    process_verified_announce(&ep.shared, interface, packet, decoded);
    let fact = ep.next_announcement().await.unwrap();
    assert_eq!(fact.destination, destination);
    assert_eq!(fact.identity.hash(), peer.hash());
    assert_eq!(fact.app_data, b"opaque");
    assert_eq!(fact.interface, interface);
    assert_eq!(fact.hops, 2);
    assert_eq!(fact.transport, Some(AddressHash::from_bytes([0x55; 16])));
    assert_eq!(fact.sequence, 1);
}

/// RNS learns from a path response but never rebroadcasts it: it answers one requester.
#[tokio::test]
async fn a_path_response_is_learned_but_not_relayed() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4C; 64]));
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();
    let peer = PrivateIdentity::from_secret_bytes(&[0x4D; 64]);
    let (packet, announcement) = freshness_announce(
        &peer,
        "path-response",
        crate::path::CTX_PATH_RESPONSE,
        1,
        10,
        2,
    );
    process_verified_announce(&ep.shared, a.id(), packet, announcement.clone());

    let event = ep.next_announcement().await.unwrap();
    assert_eq!(event.destination, announcement.destination);
    assert_eq!(ep.route_to(announcement.destination), Some((a.id(), 2)));
    assert!(b.outbound.queues.pop().is_none(), "not relayed");
    assert!(a.outbound.queues.pop().is_none());
    assert_eq!(ep.routing_counters().forwarded_announces, 0);
}

/// A relay echoing one of our own announces back must not teach us a path to ourselves,
/// publish ourselves as a peer, or be relayed again.
#[tokio::test]
async fn an_echo_of_our_own_announce_is_dropped() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4E; 64]));
    let name = DestinationName::new("retinue", ["own-echo"]);
    let destination = name.destination_hash(ep.identity());
    ep.register(name.clone(), b"own");
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();

    let mut echo = ep.build_announce_at(&name, b"own", 4_000);
    echo.hops += 1;
    echo.header_type = crate::packet::HeaderType::Type2;
    echo.transport = Some(AddressHash::from_bytes([0x7E; 16]));
    route(&ep.shared, a.id(), echo);

    assert_eq!(ep.shared.announce_sequence.load(Ordering::Relaxed), 0);
    assert!(ep.route_to(destination).is_none());
    assert!(ep.resolve(destination).is_none());
    assert!(a.outbound.queues.pop().is_none());
    assert!(b.outbound.queues.pop().is_none());
    assert_eq!(ep.routing_counters().forwarded_announces, 0);
}

fn dest(n: u8) -> AddressHash {
    AddressHash::from_bytes([n; 16])
}

fn rate(target_ms: u64, grace: u16, penalty_ms: u64) -> AnnounceRate {
    AnnounceRate {
        target: Duration::from_millis(target_ms),
        grace,
        penalty: Duration::from_millis(penalty_ms),
    }
}

/// The default rule relays the first announce and five graces of a destination
/// re-announcing every 2 s, as a stock transport does.
#[test]
fn the_default_rate_relays_the_first_and_five_graces() {
    let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
    let default = a.policy().destination_rate().unwrap();
    assert_eq!(default, AnnounceRate::default());
    let relayed = (0..8)
        .filter(|i| a.observe_destination(dest(1), default, i * 2_000) == DestinationVerdict::Relay)
        .count();
    assert_eq!(relayed, 6);
}

/// Blocked until `last + target + penalty`; afterwards a slow arrival forgives one
/// violation, and the next fast one blocks again from the new `last`.
#[test]
fn a_block_runs_from_the_last_relay_and_violations_decay() {
    let mut a = AnnounceAdmission::new(AnnounceIngressPolicy::default());
    let r = rate(10_000, 1, 5_000);
    for (now, verdict) in [
        (0, DestinationVerdict::Relay),
        (1_000, DestinationVerdict::Relay),
        (2_000, DestinationVerdict::BlockRelay),
        (16_000, DestinationVerdict::BlockRelay),
        (16_001, DestinationVerdict::Relay),
        (17_000, DestinationVerdict::BlockRelay),
        (31_001, DestinationVerdict::BlockRelay),
    ] {
        assert_eq!(a.observe_destination(dest(1), r, now), verdict, "{now}");
    }
    assert_eq!(
        a.observe_destination(dest(2), r, 2_000),
        DestinationVerdict::Relay
    );
    assert!(
        AnnounceIngressPolicy {
            destination_target: Duration::ZERO,
            ..Default::default()
        }
        .destination_rate()
        .is_none()
    );
}

/// A transport relays a destination's path-updating announces under its ingress interface's
/// rate rule, else the endpoint's; a blocked one is still learned (`Transport.py` 2303-2338).
#[tokio::test]
async fn the_relay_rate_follows_the_ingress_interface_and_blocked_announces_are_learned() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x52; 64]));
    ep.enable_routing();
    let strict = ep.attach_interface().id();
    let plain = ep.attach_interface().id();
    let _out = ep.attach_interface();
    let strict_rate = AnnounceRate {
        grace: 0,
        ..Default::default()
    };
    assert!(ep.set_interface_announce_rate(strict, Some(strict_rate)));
    let peer = PrivateIdentity::from_secret_bytes(&[0x53; 64]);
    for timebase in [10, 11] {
        let (packet, _) = freshness_announce(&peer, "strict", 0, timebase as u8, timebase, 1);
        route(&ep.shared, strict, packet);
    }
    let (_, latest) = freshness_announce(&peer, "strict", 0, 11, 11, 1);
    assert_eq!(ep.route_to(latest.destination), Some((strict, 1)));
    assert_eq!(ep.routing_counters().relay_rate_limited_announces, 1);

    // The endpoint default allows five graces.
    let other = PrivateIdentity::from_secret_bytes(&[0x54; 64]);
    for timebase in 10..16 {
        let (packet, _) = freshness_announce(&other, "plain", 0, timebase as u8, timebase, 1);
        route(&ep.shared, plain, packet);
    }
    assert_eq!(ep.routing_counters().relay_rate_limited_announces, 1);
}

/// A copy of a verified announce costs no second signature check: with a live route it is a
/// freshness replay, refused unverified; without one it is a first sighting again, decoded
/// from the verified-announce cache.
#[tokio::test]
async fn a_copy_of_a_verified_announce_is_not_verified_again() {
    use crate::probe::{Probe, take};

    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x5C; 64]));
    let a = ep.attach_interface();
    let (packet, announce) = peer_announce(0x5D, "copies");
    take(Probe::AnnounceVerify);
    route(&ep.shared, a.id(), packet.clone());
    assert_eq!(take(Probe::AnnounceVerify), 1);
    assert!(ep.route_to(announce.destination).is_some());

    let mut relayed = packet;
    relayed.hops = 2;
    relayed.header_type = crate::packet::HeaderType::Type2;
    relayed.transport = Some(AddressHash::from_bytes([0x7E; 16]));
    route(&ep.shared, a.id(), relayed.clone());
    assert_eq!(take(Probe::AnnounceVerify), 0);
    assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);

    ep.shared.forget_path(announce.destination);
    route(&ep.shared, a.id(), relayed);
    assert_eq!(take(Probe::AnnounceVerify), 0);
    assert!(ep.route_to(announce.destination).is_some());
}

/// RNS learns an announce heard with up to 127 wire hops (`Transport.py` 2211) but
/// rebroadcasts only below `PATHFINDER_M` (`Transport.py` 1356): 126 goes on as 127, and 127
/// is learned and kept.
#[tokio::test(start_paused = true)]
async fn an_announce_is_relayed_only_below_the_hop_ceiling() {
    for (hops, relayed) in [(MAX_HOPS - 2, true), (MAX_HOPS - 1, false)] {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4F; 64]));
        let a = ep.attach_interface();
        let mut b = ep.attach_interface();
        ep.enable_routing();
        let peer = PrivateIdentity::from_secret_bytes(&[0x50; 64]);
        let (packet, announcement) = freshness_announce(&peer, "ceiling", 0, 1, 10, hops);
        process_verified_announce(&ep.shared, a.id(), packet, announcement.clone());

        assert_eq!(ep.route_to(announcement.destination), Some((a.id(), hops)));
        // Relays leave from the rebroadcast table, within its jitter window.
        let out = tokio::time::timeout(Duration::from_secs(1), b.next_outbound())
            .await
            .ok()
            .flatten();
        assert_eq!(
            out.map(|p| p.hops),
            relayed.then_some(MAX_HOPS - 1),
            "{hops}"
        );
        assert_eq!(ep.routing_counters().hop_limit_dropped, u64::from(!relayed));
    }
}

/// Each interface holds up to its own capacity, refuses an announce it could not relay on,
/// and releases the fewest-hops entry first; an interface with ingress control off holds
/// nothing (`Interface.py` 270-297; `Reticulum.py` 904-905).
#[tokio::test(start_paused = true)]
async fn held_announces_are_per_interface_and_released_fewest_hops_first() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x55; 64]));
    let noisy = ep.attach_interface().id();
    let other = ep.attach_interface().id();
    let serial = ep.attach_interface().id();
    let policy = AnnounceIngressPolicy {
        held_capacity: 2,
        new_interface_hz: 1,
        established_interface_hz: 1,
        burst_hold: Duration::from_millis(50),
        burst_penalty: Duration::from_millis(50),
        held_release_interval: Duration::from_millis(10),
        ..Default::default()
    };
    ep.set_announce_ingress_policy(policy);
    let off = AnnounceIngressPolicy {
        enabled: false,
        ..policy
    };
    assert!(ep.set_interface_ingress_policy(serial, Some(off)));
    let mut seed = 0x60;
    let mut announce_on = |iface, hops| {
        seed += 1;
        let peer = PrivateIdentity::from_secret_bytes(&[seed; 64]);
        let (packet, announce) = freshness_announce(&peer, "held", 0, 1, 10, hops);
        route(&ep.shared, iface, packet);
        announce.destination
    };
    // Two arrivals at one instant fill the 1 Hz budget; the rest are a burst.
    announce_on(noisy, 1);
    announce_on(noisy, 1);
    announce_on(noisy, MAX_HOPS - 2);
    let three = announce_on(noisy, 3);
    let one = announce_on(noisy, 1);
    announce_on(noisy, 2);
    for _ in 0..3 {
        announce_on(other, 1);
    }
    for _ in 0..5 {
        announce_on(serial, 1);
    }
    let counters = ep.announce_ingress_counters(noisy);
    assert_eq!((counters.held, counters.held_dropped), (2, 2));
    assert_eq!(ep.announce_ingress_counters(other).held, 1);
    assert_eq!(ep.announce_ingress_counters(serial).held, 0);

    while ep.announce_ingress_counters(noisy).released == 0 {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert!(ep.route_to(one).is_some(), "the nearest is released first");
    assert!(ep.route_to(three).is_none());
    while ep.route_to(three).is_none() {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
}

/// A path-updating announce is rate-counted on any ingress, relayed or not, as RNS counts
/// before it decides to rebroadcast (`Transport.py` 2299-2333).
#[tokio::test]
async fn the_relay_rate_counts_announces_from_a_filtered_ingress() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x56; 64]));
    let filtered = ep.attach_interface().id();
    let allowed = ep.attach_interface().id();
    let _out = ep.attach_interface();
    ep.set_routing_policy(RoutingPolicy {
        allowed_ingress: InterfaceSelector::Only(vec![allowed]),
        ..RoutingPolicy::transit()
    });
    ep.set_announce_ingress_policy(AnnounceIngressPolicy {
        destination_grace: 0,
        ..Default::default()
    });
    let peer = PrivateIdentity::from_secret_bytes(&[0x57; 64]);
    for (iface, timebase) in [(filtered, 10), (allowed, 11)] {
        let (packet, _) = freshness_announce(&peer, "mixed", 0, timebase as u8, timebase, 1);
        route(&ep.shared, iface, packet);
    }
    assert_eq!(ep.routing_counters().relay_rate_limited_announces, 1);
}

/// Each interface holds its own share, the endpoint at most its ceiling in all, and an
/// interface's share leaves with its admission row.
#[tokio::test]
async fn held_announces_have_an_endpoint_ceiling_and_leave_with_their_row() {
    let entry = |interface, seed| {
        let (packet, announce) = peer_announce(seed, "ceiling");
        HeldAnnounce {
            interface,
            packet,
            announce,
        }
    };
    let mut held = HeldAnnounces::default();
    assert!(held.hold(entry(1, 0x70), 2, 3));
    assert!(held.hold(entry(1, 0x71), 2, 3));
    assert!(
        !held.hold(entry(1, 0x72), 2, 3),
        "the interface's share is full"
    );
    assert!(
        held.hold(entry(1, 0x70), 2, 3),
        "a held destination is replaced"
    );
    assert!(held.hold(entry(2, 0x73), 2, 3));
    assert!(!held.hold(entry(3, 0x74), 2, 3), "the endpoint is full");
    assert!(!held.holds(3));
    held.purge(1);
    assert!(held.hold(entry(3, 0x74), 2, 3));
    assert!(held.take_nearest(2).is_some() && !held.holds(2));

    // A row evicted by a newcomer takes its held announces with it.
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x58; 64]));
    let noisy = ep.attach_interface().id();
    ep.set_announce_ingress_policy(AnnounceIngressPolicy {
        interface_capacity: 1,
        new_interface_hz: 1,
        ..Default::default()
    });
    for seed in 0x75..0x78 {
        route(&ep.shared, noisy, peer_announce(seed, "evicted").0);
    }
    assert!(ep.shared.held_announces.lock().unwrap().holds(noisy));
    let newcomer = ep.attach_interface().id();
    route(&ep.shared, newcomer, peer_announce(0x78, "evicted").0);
    assert!(!ep.shared.held_announces.lock().unwrap().holds(noisy));
}
