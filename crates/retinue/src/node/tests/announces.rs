//! Learning peers from announces, announcing, and answering path requests.

use super::*;

/// A real RNS announce teaches the node a peer it can then reach.
#[test]
fn a_real_announce_is_learned() {
    let mut n = node();
    let packet = fixture("announce_appdata.bin");
    let actions = n.ingest(IFACE, &packet, 0);

    assert_eq!(actions.len(), 1, "one learned destination");
    assert_eq!(n.peers().len(), 1);
    match actions.iter().next().unwrap() {
        Action::Learned { destination } => assert!(n.peers().knows(*destination)),
        other => panic!("expected Learned, got {other:?}"),
    }
}

/// Every RNS-generated invalid announce is refused, and none of them leaves a trace.
///
/// This is the oracle that matters most for a board: the fixtures were produced by real
/// RNS with one field corrupted each, so a node that accepted any of them would be
/// letting a peer populate its tables with unverified identity.
#[test]
fn every_invalid_announce_fixture_is_refused() {
    for name in [
        "announce_invalid_signature.bin",
        "announce_invalid_pubkey.bin",
        "announce_invalid_desthash.bin",
        "announce_invalid_namehash.bin",
        "announce_invalid_randhash.bin",
        "announce_invalid_appdata.bin",
    ] {
        let mut n = node();
        let actions = n.ingest(IFACE, &fixture(name), 0);
        assert!(actions.is_empty(), "{name} produced an action");
        assert_eq!(n.peers().len(), 0, "{name} populated the address book");
    }
}

/// A node announces itself promptly on boot, then holds off for its interval.
#[test]
fn announces_on_boot_then_waits_for_the_interval() {
    let mut n = node().with_announce_interval(1_000);
    let announce_blob = blob([0x55; RAND_HASH_LEN]);

    assert!(n.announce_due(0), "a fresh node is due on boot");
    let first = n.poll(0, IFACE, Some(&announce_blob));
    assert_eq!(first.len(), 1, "a fresh node announces without waiting");

    assert!(!n.announce_due(1), "the interval has not elapsed");
    assert!(
        n.poll(1, IFACE, Some(&announce_blob)).is_empty(),
        "not due yet"
    );
    assert!(
        n.poll(999, IFACE, Some(&announce_blob)).is_empty(),
        "still not due"
    );
    assert!(n.announce_due(1_000), "the interval has elapsed");
    assert_eq!(
        n.poll(1_000, IFACE, Some(&announce_blob)).len(),
        1,
        "due at the interval"
    );
}

#[test]
fn a_due_announce_without_a_blob_stays_due_until_supplied() {
    let mut n = node().with_announce_interval(1_000);

    assert!(n.announce_due(0));
    assert!(n.poll(0, IFACE, None).is_empty());
    assert!(n.announce_due(1), "missing blob must not consume due state");
    assert!(n.poll(1, IFACE, None).is_empty());

    let announce_blob = blob([0x56; RAND_HASH_LEN]);
    assert!(sent(&n.poll(1, IFACE, Some(&announce_blob))).is_some());
    assert!(!n.announce_due(2), "successful emission consumes due state");
    assert!(n.poll(2, IFACE, Some(&announce_blob)).is_empty());
}

/// Our own announce is a real one: it decodes, verifies, and names us.
#[test]
fn our_announce_round_trips_through_the_decoder() {
    let n = node().with_app_data(b"retinue-node");
    let packet = n.announce(&blob([0x22; RAND_HASH_LEN]), None);

    let decoded = Announce::decode(&packet).expect("our own announce must verify");
    assert_eq!(decoded.destination, n.destination());
    assert_eq!(decoded.app_data, b"retinue-node");
}

/// Two nodes learn each other from each other's announces, which is the whole of the
/// discovery half of a link.
#[test]
fn two_nodes_learn_each_other() {
    let mut a = Node::<32, 8>::new(
        PrivateIdentity::from_secret_bytes(&[0xA1; 64]),
        DestinationName::new("retinue", ["a"]).name_hash(),
    );
    let mut b = Node::<32, 8>::new(
        PrivateIdentity::from_secret_bytes(&[0xB2; 64]),
        DestinationName::new("retinue", ["b"]).name_hash(),
    );

    let from_a = a.announce(&blob([1; RAND_HASH_LEN]), None);
    let from_b = b.announce(&blob([2; RAND_HASH_LEN]), None);

    assert_eq!(b.ingest(IFACE, &from_a, 0).len(), 1);
    assert_eq!(a.ingest(IFACE, &from_b, 0).len(), 1);

    assert!(b.peers().knows(a.destination()), "b can now reach a");
    assert!(a.peers().knows(b.destination()), "a can now reach b");
}

/// A full address book keeps serving and stops learning, rather than growing.
#[test]
fn a_full_book_stops_learning_without_faulting() {
    let mut n = Node::<1, 8>::new(
        PrivateIdentity::from_secret_bytes(&[0x11; 64]),
        DestinationName::new("retinue", ["node"]).name_hash(),
    );
    assert_eq!(
        n.ingest(IFACE, &fixture("announce_appdata.bin"), 0).len(),
        1
    );

    // A different destination cannot be learned, and says nothing rather than faulting.
    let other = Node::<32, 8>::new(
        PrivateIdentity::from_secret_bytes(&[0xC3; 64]),
        DestinationName::new("retinue", ["other"]).name_hash(),
    )
    .announce(&blob([9; RAND_HASH_LEN]), None);
    assert!(n.ingest(IFACE, &other, 0).is_empty());
    assert_eq!(n.peers().len(), 1, "the established peer survives");
    assert_eq!(n.peers().refused(), 1, "and the refusal is counted");
}

/// Data and link packets are not yet handled, and must be dropped rather than
/// mishandled. This pins the boundary so the next gate's work is visible as a change.
#[test]
fn packets_this_gate_does_not_handle_are_dropped() {
    let mut n = node();
    // The same bytes that would be learned as an announce, relabelled. Nothing is
    // learned, because the type decides the handling and this gate handles one type.
    let packet = Packet {
        packet_type: PacketType::Data,
        ..fixture("announce_appdata.bin")
    };
    let actions = n.ingest(IFACE, &packet, 0);
    assert!(actions.is_empty());
    assert_eq!(n.peers().len(), 0);
}

/// A shell that could not send the announce can say so, and the next poll announces
/// again instead of waiting out the whole interval.
#[test]
fn a_failed_announce_can_be_retried_before_the_interval() {
    let (mut a, _b) = pair();

    assert!(
        sent(&a.poll(0, IFACE, Some(&blob([1; RAND_HASH_LEN])))).is_some(),
        "the first poll announces"
    );
    assert!(
        a.poll(1_000, IFACE, Some(&blob([2; RAND_HASH_LEN])))
            .is_empty(),
        "and the next is not due for a whole interval"
    );
    assert!(!a.announce_due(1_000), "the interval is not elapsed yet");

    // The shell reports that the frame never reached the air.
    a.retry_announce();
    assert!(
        a.announce_due(1_001),
        "retry makes the announce due immediately"
    );
    assert!(
        sent(&a.poll(1_001, IFACE, Some(&blob([3; RAND_HASH_LEN])))).is_some(),
        "so the node announces again rather than waiting out the interval"
    );
}

/// A node answers a path request for itself with its last announce as a path response,
/// on the requesting interface only, once per tag; it ignores tagless requests, requests
/// relayed past their first hop, and requests for anyone else.
#[test]
fn path_requests_for_this_node_are_answered_once_on_their_interface() {
    let mut n = node();
    let me = n.destination();
    let request = |tag: u8| crate::path::path_request(me, &[tag; crate::path::TAG_LEN]);
    assert!(
        n.ingest(2, &request(1), 0).is_empty(),
        "nothing announced yet, so nothing to answer with"
    );

    let announced = sent(&n.poll(0, 0, Some(&blob([0x5C; RAND_HASH_LEN])))).unwrap();
    let actions = n.ingest(2, &request(2), 1);
    let [Action::Send { interface, packet }] = actions.iter().collect::<Vec<_>>()[..] else {
        panic!("one send expected, got {actions:?}");
    };
    assert_eq!(*interface, 2, "answered on the requesting interface");
    assert_eq!(packet.packet_type, PacketType::Announce);
    assert_eq!(packet.context, crate::path::CTX_PATH_RESPONSE);
    assert_eq!(packet.payload, announced.payload);
    assert!(Announce::decode(packet).is_ok());

    assert!(n.ingest(3, &request(2), 2).is_empty(), "a repeated tag");
    assert!(sent(&n.ingest(3, &request(3), 2)).is_some(), "a new tag");

    let mut tagless = request(4);
    tagless.payload.truncate(crate::hash::ADDRESS_HASH_LEN);
    assert!(n.ingest(2, &tagless, 3).is_empty());
    let mut far = request(5);
    far.hops = 2;
    assert!(n.ingest(2, &far, 3).is_empty());
    let mut near = request(6);
    near.hops = 1;
    assert!(sent(&n.ingest(2, &near, 3)).is_some());
    assert_eq!(n.transport_counters().filtered_packets, 3);

    let other = crate::path::path_request(AddressHash::from_bytes([0xCD; 16]), &[7; 16]);
    assert!(n.ingest(2, &other, 4).is_empty());
}

/// A relay rebroadcasts a node's own announce, so the node hears itself. The echo must not
/// make the node its own peer, while genuine peers still learn it through the relay.
#[test]
fn own_announce_echoed_by_a_relay_is_not_a_peer() {
    let (mut a, mut b) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x46; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());

    let announce = a.announce(&blob([0x31; RAND_HASH_LEN]), None);
    let echo = relayed(&mut relay, IFACE, &announce, 0)
        .expect("the relay still rebroadcasts the announce for others");
    assert_eq!(relay.transport_counters().forwarded_announces, 1);

    let heard = a.ingest(IFACE, &echo, 1);
    assert_eq!(a.peers().len(), 0, "a node is not its own peer");
    assert!(!a.peers().knows(a.destination()));
    assert!(
        heard.is_empty(),
        "the echo of our own announce does nothing"
    );
    assert_eq!(a.route_count(), 0, "no route to ourselves is learned");

    b.ingest(IFACE, &echo, 1);
    assert!(
        b.peers().knows(a.destination()),
        "a genuine peer is learned"
    );
    assert_eq!(b.peers().len(), 1);
    assert!(relay.peers().knows(a.destination()));
}
