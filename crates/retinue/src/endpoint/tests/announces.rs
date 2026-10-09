//! Announce timebases, key conflicts, observations, echoes, and relay admission.

use super::*;

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

#[test]
fn destination_admission_preserves_the_one_second_default_floor() {
    let mut admission = AnnounceAdmission::new(AnnounceIngressPolicy::default());
    let a = AddressHash::from_bytes([0x01; 16]);
    let b = AddressHash::from_bytes([0x02; 16]);
    assert_eq!(
        admission.observe_destination(a, 0),
        DestinationVerdict::Relay
    );
    assert_eq!(
        admission.observe_destination(b, 0),
        DestinationVerdict::Relay
    );
    assert_eq!(
        admission.observe_destination(a, 1),
        DestinationVerdict::BlockRelay,
        "a fresh re-announce is not rebroadcast"
    );
    let c = AddressHash::from_bytes([0x03; 16]);
    assert_eq!(
        admission.observe_destination(c, 1),
        DestinationVerdict::Relay
    );
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
#[tokio::test]
async fn an_announce_is_relayed_only_below_the_hop_ceiling() {
    for (hops, relayed) in [(MAX_HOPS - 2, true), (MAX_HOPS - 1, false)] {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4F; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let peer = PrivateIdentity::from_secret_bytes(&[0x50; 64]);
        let (packet, announcement) = freshness_announce(&peer, "ceiling", 0, 1, 10, hops);
        process_verified_announce(&ep.shared, a.id(), packet, announcement.clone());

        assert_eq!(ep.route_to(announcement.destination), Some((a.id(), hops)));
        let out = b.outbound.queues.pop();
        assert_eq!(
            out.map(|p| p.hops),
            relayed.then_some(MAX_HOPS - 1),
            "{hops}"
        );
        assert_eq!(ep.routing_counters().hop_limit_dropped, u64::from(!relayed));
    }
}
