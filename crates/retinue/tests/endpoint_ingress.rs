//! Ingress preservation: an accepted session reports the interface it arrived on.
//!
//! V3 of the 2026-07-24 low-power radio and managed-network plan. A policy layer
//! above Retinue can only make honest decisions if ingress is a *transport fact*
//! carried out of accept, not something inferred later. These tests pin that the
//! interface survives to every accepted form, and that concurrent accepts on
//! different interfaces do not exchange it.

use std::time::Duration;

use retinue::announce_admission::AnnounceIngressPolicy;
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;

async fn signed_announce(
    seed: u8,
    aspect: &'static str,
) -> (retinue::hash::AddressHash, retinue::Packet) {
    let identity = PrivateIdentity::from_secret_bytes(&[seed; 64]);
    let sender = Endpoint::new(identity.clone());
    let mut wire = sender.attach_interface();
    let name = DestinationName::new("flood", [aspect]);
    let destination = name.destination_hash(identity.public());
    sender.announce(&name, b"ingress receipt");
    let packet = tokio::time::timeout(Duration::from_secs(1), wire.next_outbound())
        .await
        .expect("sender queues an announce")
        .expect("sender remains live");
    (destination, packet)
}

/// Wait until `ep` can resolve `dest`, pumping announcements.
async fn await_resolve(ep: &Endpoint, dest: retinue::hash::AddressHash) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ep.resolve(dest).is_none() && tokio::time::Instant::now() < deadline {
        let _ = tokio::time::timeout(Duration::from_millis(300), ep.next_announcement()).await;
    }
    assert!(ep.resolve(dest).is_some(), "peer should learn the dest");
}

/// Two leaves on two interfaces of one hub: each accepted link reports the
/// interface it actually arrived on, the two differ, and the wrapper agrees with
/// the stream.
#[tokio::test]
async fn accepted_links_report_their_own_interface() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[9u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let hub_name = DestinationName::new("hub", ["svc"]);
    let hub_dest = hub_name.destination_hash(hub_id.public());
    hub.register(hub_name.clone(), b"svc");

    // Two leaves, each its own TCP connection => its own hub-side interface.
    let a = Endpoint::new(PrivateIdentity::from_secret_bytes(&[2u8; 64]));
    a.attach_tcp_client(addr).await.unwrap();
    let b = Endpoint::new(PrivateIdentity::from_secret_bytes(&[3u8; 64]));
    b.attach_tcp_client(addr).await.unwrap();

    for _ in 0..4 {
        hub.announce(&hub_name, b"svc");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&a, hub_dest).await;
    await_resolve(&b, hub_dest).await;

    let hub_identity = *hub_id.public();
    let _a_stream = a.open(hub_dest, hub_identity).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("first accept should not time out")
        .unwrap();

    let _b_stream = b.open(hub_dest, hub_identity).await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("second accept should not time out")
        .unwrap();

    // The wrapper's ingress and the stream's ingress are the same fact.
    assert_eq!(
        first.interface,
        first.stream.interface(),
        "Accepted.interface must agree with LinkStream::interface"
    );
    assert_eq!(second.interface, second.stream.interface());

    // Two leaves arrived on two interfaces: ingress is not crossed or collapsed.
    assert_ne!(
        first.interface, second.interface,
        "links from different interfaces must report different ingress"
    );

    // Both targeted the same destination, so destination alone cannot
    // distinguish them: ingress is the added fact.
    assert_eq!(first.destination, hub_dest);
    assert_eq!(second.destination, hub_dest);
}

/// Two links from the *same* leaf report the same interface: ingress identifies
/// the bearer, not the session.
#[tokio::test]
async fn links_from_one_peer_share_an_interface() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[11u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let hub_name = DestinationName::new("hub", ["twice"]);
    let hub_dest = hub_name.destination_hash(hub_id.public());
    hub.register(hub_name.clone(), b"twice");

    let leaf = Endpoint::new(PrivateIdentity::from_secret_bytes(&[12u8; 64]));
    leaf.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&hub_name, b"twice");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&leaf, hub_dest).await;

    let hub_identity = *hub_id.public();
    let _one = leaf.open(hub_dest, hub_identity).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("first accept")
        .unwrap();
    let _two = leaf.open(hub_dest, hub_identity).await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("second accept")
        .unwrap();

    assert_eq!(
        first.interface, second.interface,
        "two links over one bearer share ingress"
    );
}

/// The reliable accept path surfaces a bare stream rather than a wrapper, so it
/// is the path most likely to lose ingress. One leaf over one TCP connection
/// opens both a best-effort and a reliable link: both must report the same
/// interface, which is what "the paths do not diverge" means concretely.
#[tokio::test]
async fn reliable_accept_preserves_the_same_ingress_as_best_effort() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[21u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    // Two destinations on one hub: one best-effort, one reliable. A reliable
    // link is only dispatched as such if its destination was registered
    // reliable, so the two cannot share a name.
    let be_name = DestinationName::new("hub", ["besteffort"]);
    let be_dest = be_name.destination_hash(hub_id.public());
    hub.register(be_name.clone(), b"be");
    let rel_name = DestinationName::new("hub", ["reliable"]);
    let rel_dest = rel_name.destination_hash(hub_id.public());
    hub.register_reliable(rel_name.clone(), b"rel");

    let leaf = Endpoint::new(PrivateIdentity::from_secret_bytes(&[22u8; 64]));
    let leaf_iface = leaf.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&be_name, b"be");
        hub.announce(&rel_name, b"rel");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&leaf, be_dest).await;
    await_resolve(&leaf, rel_dest).await;

    let hub_identity = *hub_id.public();

    // Best-effort first: this is the hub-side interface for this leaf.
    let _be_client = leaf.open(be_dest, hub_identity).await.unwrap();
    let be_accepted = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("best-effort accept should not time out")
        .unwrap();

    // Reliable over the same bearer must report that same interface.
    let rel_client = leaf.open_reliable(rel_dest, hub_identity).await.unwrap();
    let rel_accepted = tokio::time::timeout(Duration::from_secs(10), hub.accept_reliable_on_any())
        .await
        .expect("reliable accept should not time out")
        .unwrap();

    assert_eq!(
        rel_accepted.interface, be_accepted.interface,
        "a reliable accept must report the same ingress as a best-effort accept \
         over the same bearer"
    );
    assert_eq!(
        rel_accepted.destination, rel_dest,
        "reliable dispatch retains the destination it targeted"
    );
    // And the initiator's own reliable stream knows the interface it went out on.
    assert_eq!(
        rel_client.interface(),
        leaf_iface,
        "an outbound reliable stream reports the interface it was opened over"
    );
}

/// An outbound best-effort stream reports the interface it was opened over, so
/// both directions of a session can be attributed to a bearer.
#[tokio::test]
async fn outbound_stream_reports_its_interface() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[31u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let hub_name = DestinationName::new("hub", ["out"]);
    let hub_dest = hub_name.destination_hash(hub_id.public());
    hub.register(hub_name.clone(), b"out");

    let leaf = Endpoint::new(PrivateIdentity::from_secret_bytes(&[32u8; 64]));
    let iface = leaf.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&hub_name, b"out");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&leaf, hub_dest).await;

    let stream = leaf.open(hub_dest, *hub_id.public()).await.unwrap();
    assert_eq!(
        stream.interface(),
        iface,
        "an outbound stream reports the interface it was opened over"
    );
}

/// A verified multi-destination burst on one bearer is bounded and released later; another
/// bearer remains admissible, and a repeat destination is learned locally but not relayed.
/// This is a host ingress receipt, not a radio-airtime or firmware-memory measurement.
#[tokio::test]
async fn announce_ingress_burst_is_bounded_attributed_and_does_not_silence_a_neighbor() {
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[71u8; 64]));
    hub.enable_routing();
    let noisy = hub.attach_interface();
    let noisy_id = noisy.id();
    let noisy_sink = noisy.sink();
    let quiet = hub.attach_interface();
    let quiet_id = quiet.id();
    let quiet_sink = quiet.sink();
    let _egress = hub.attach_interface();

    let mut policy = AnnounceIngressPolicy::default();
    policy.held_capacity = 4;
    policy.burst_hold = Duration::from_millis(20);
    policy.burst_penalty = Duration::from_millis(20);
    policy.held_release_interval = Duration::from_millis(5);
    // Keep the production 3/10 Hz defaults in the policy tests. This accelerated receipt
    // preserves the same burst/release relationship without waiting several real seconds.
    policy.new_interface_hz = 50;
    policy.established_interface_hz = 50;
    hub.set_announce_ingress_policy(policy);

    let mut burst_destinations = Vec::new();
    for (seed, aspect) in [
        (81, "one"),
        (82, "two"),
        (83, "three"),
        (84, "four"),
        (85, "five"),
        (86, "six"),
        (87, "seven"),
        (88, "eight"),
        (89, "nine"),
        (90, "ten"),
    ] {
        let (destination, packet) = signed_announce(seed, aspect).await;
        burst_destinations.push(destination);
        assert!(noisy_sink.deliver(packet));
        // The state machine measures frequency rather than packet-loop iterations. Advancing
        // the scheduler here gives every verified arrival a distinct monotonic observation.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let (quiet_destination, quiet_packet) = signed_announce(91, "quiet").await;
    assert!(quiet_sink.deliver(quiet_packet));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline {
        let noisy_counters = hub.announce_ingress_counters(noisy_id);
        if noisy_counters.released >= 1 && hub.resolve(quiet_destination).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let noisy_counters = hub.announce_ingress_counters(noisy_id);
    assert!(
        noisy_counters.held >= 4,
        "the burst must enter the bounded hold queue"
    );
    assert!(
        noisy_counters.held_dropped >= 1,
        "the queue ceiling must reject excess verified announces"
    );
    assert!(
        noisy_counters.released >= 1,
        "at least one held announce must return after the burst penalty"
    );
    assert!(
        hub.resolve(quiet_destination).is_some(),
        "a quiet neighboring bearer must remain admissible"
    );
    assert_eq!(
        hub.announce_ingress_counters(quiet_id).held,
        0,
        "the noisy bearer must not attribute its burst to the quiet neighbor"
    );
    assert!(
        burst_destinations
            .iter()
            .any(|destination| hub.resolve(*destination).is_some()),
        "the receipt must include a released burst destination, not only the quiet neighbor"
    );

    let (repeat_destination, first) = signed_announce(99, "repeat").await;
    let (_, second) = signed_announce(99, "repeat").await;
    assert!(quiet_sink.deliver(first));
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert!(quiet_sink.deliver(second));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline
        && hub.routing_counters().relay_rate_limited_announces == 0
    {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        hub.resolve(repeat_destination).is_some(),
        "destination rate pressure never suppresses a valid local learn"
    );
    assert!(
        hub.routing_counters().relay_rate_limited_announces >= 1,
        "the fresh repeat is not re-broadcast after its destination rate block"
    );
}
