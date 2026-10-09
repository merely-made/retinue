//! A bounded, attributed announce burst on one bearer.

use super::*;

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

/// A verified multi-destination burst on one bearer is bounded and released later; another
/// bearer remains admissible, and a repeat destination is learned locally but not relayed.
/// A host ingress receipt, not an airtime or firmware-memory measurement.
///
/// The clock is paused: admission reads `tokio::time::Instant`, so the 1 ms burst spacing
/// against a 20 ms interface period is exact on every OS.
#[tokio::test(start_paused = true)]
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

    // Accelerated from the production 3/10 Hz defaults, keeping the burst/release relation.
    let policy = AnnounceIngressPolicy {
        held_capacity: 4,
        burst_hold: Duration::from_millis(20),
        burst_penalty: Duration::from_millis(20),
        held_release_interval: Duration::from_millis(5),
        new_interface_hz: 50,
        established_interface_hz: 50,
        ..AnnounceIngressPolicy::default()
    };
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
        // Admission measures frequency, so each arrival needs its own instant.
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

    let repeat_identity = PrivateIdentity::from_secret_bytes(&[99; 64]);
    let repeat_sender = Endpoint::new(repeat_identity.clone());
    let mut repeat_wire = repeat_sender.attach_interface();
    let repeat_name = DestinationName::new("flood", ["repeat"]);
    let repeat_destination = repeat_name.destination_hash(repeat_identity.public());
    // A repeat must share one sender so its per-destination freshness timebase advances.
    repeat_sender.announce(&repeat_name, b"ingress receipt");
    let first = tokio::time::timeout(Duration::from_secs(1), repeat_wire.next_outbound())
        .await
        .expect("repeat sender queues a first announce")
        .expect("repeat sender remains live");
    repeat_sender.announce(&repeat_name, b"ingress receipt");
    let second = tokio::time::timeout(Duration::from_secs(1), repeat_wire.next_outbound())
        .await
        .expect("repeat sender queues a second announce")
        .expect("repeat sender remains live");
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
