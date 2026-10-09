//! Link establishment, data, own-echo and duplicate filtering, and closing.

use super::*;

/// Two nodes establish a link in the shape a radio carries it: announce, learn, open,
/// accept, prove.
#[test]
fn two_nodes_establish_a_link() {
    let (mut a, mut b) = pair();

    // Discovery first: a must have heard b announce before it can address b.
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);

    let opened = a
        .open_link(b.destination(), IFACE, &[0x31; 64], 0)
        .expect("b is known, so a link can be opened");
    let request = sent(&opened).expect("a link request goes out");

    let accepted = b.ingest(IFACE, &request, 0);
    let proof = sent(&accepted).expect("b answers with a proof");
    assert_eq!(b.link_count(), 1, "b holds the link immediately");
    assert!(link_up(&accepted).is_some(), "b reports the link up");

    let completed = a.ingest(IFACE, &proof, 0);
    assert_eq!(a.link_count(), 1, "a holds the link once proved");
    let id = link_up(&completed).expect("a reports the link up");

    assert!(
        a.has_link(id) && b.has_link(id),
        "one link, one id, both sides"
    );
}

#[test]
fn exact_idle_expiry_reclaims_a_real_link_and_its_resource_sender() {
    let (mut a, _b, id) = linked();
    assert!(
        a.publish(
            id,
            IFACE,
            b"retained resource",
            [0xA5; crate::resource::RANDOM_HASH_LEN],
            &[0x5A; crate::token::IV_LEN],
            0,
        )
        .is_some()
    );
    assert!(a.transfer_active(id));

    let report = a.expire_sessions(LINK_IDLE_TIMEOUT);
    assert_eq!(report.links.as_slice(), [id]);
    assert_eq!(report.inbound_resources.as_slice(), []);
    assert_eq!(report.outbound_resources.as_slice(), [id]);
    assert!(!a.has_link(id));
    assert!(!a.transfer_active(id));
    assert_eq!(a.expired_links(), 1);
}

/// A retransmitted link request is answered with the SAME proof, not a second link.
///
/// A lossy medium creates this constantly: the initiator does not hear the proof and
/// asks again. Accepting twice would leave the two sides holding different keys for
/// what the initiator believes is one link, which fails later and confusingly.
#[test]
fn a_retransmitted_request_is_answered_with_the_same_proof() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();

    let first = sent(&b.ingest(IFACE, &request, 0)).expect("first proof");
    let second = sent(&b.ingest(IFACE, &request, 0)).expect("second proof");

    assert_eq!(first, second, "the same proof, byte for byte");
    assert_eq!(b.link_count(), 1, "and still exactly one link");
}

/// Data crosses an established link and arrives decrypted.
#[test]
fn data_crosses_an_established_link() {
    let (mut a, mut b, id) = linked();

    let out = a
        .send(id, IFACE, b"hello over the air", &[7; crate::token::IV_LEN])
        .expect("a can send on a link it holds");
    let packet = sent(&out).expect("a data packet goes out");

    let received = b.ingest(IFACE, &packet, 0);
    let found = received.iter().find_map(|x| match x {
        Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
        _ => None,
    });
    match found {
        Some((link_id, payload)) => {
            assert_eq!(link_id, id);
            assert_eq!(payload.as_slice(), b"hello over the air");
        }
        None => panic!("expected decrypted Data"),
    }
}

/// On a shared medium the first relay's retransmission of our own link data reaches us
/// too. It decrypts under the shared link key, but it is our own payload and not data
/// from the far end, and hearing it is not evidence that the peer is alive. Genuine data
/// from the far end, relayed the same way, is still delivered.
#[test]
fn a_senders_own_data_overheard_from_a_relay_is_not_received() {
    let (mut source, mut destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());

    let announce = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
    let relayed_announce = sent(&relay.ingest(IFACE, &announce, 0)).unwrap();
    source.ingest(IFACE, &relayed_announce, 1);
    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x99; 64], 1)
            .unwrap(),
    )
    .unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(relay.identity.hash());
    let forwarded_request = sent(&relay.ingest(IFACE, &request, 2)).unwrap();
    let proof = sent(&destination.ingest(IFACE, &forwarded_request, 3)).unwrap();
    let forwarded_proof = sent(&relay.ingest(IFACE, &proof, 4)).unwrap();
    let id = link_up(&source.ingest(IFACE, &forwarded_proof, 5)).unwrap();

    let data_from = |actions: &Actions<8>| {
        actions.iter().find_map(|action| match action {
            Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
            _ => None,
        })
    };

    // The source sends. The relay retransmits, and the source hears the relay.
    let own = sent(
        &source
            .send(id, IFACE, b"from the source", &[0x51; 16])
            .unwrap(),
    )
    .unwrap();
    let retransmitted = sent(&relay.ingest(IFACE, &own, 6)).expect("the relay carries it on");
    assert_eq!(retransmitted.hops, 1);
    assert_eq!(
        data_from(&source.ingest(IFACE, &retransmitted, 7)),
        None,
        "a node must not surface its own payload as received data"
    );
    assert_eq!(
        source.pause_assessment().latest_link_activity,
        Some(5),
        "our own echo is not the far end being heard from"
    );

    // The same retransmission, as the far end hears it, is genuine delivery.
    assert_eq!(
        data_from(&destination.ingest(IFACE, &retransmitted, 7)),
        Some((id, b"from the source".to_vec()))
    );

    // And data the far end sends, relayed back, still reaches the source.
    let reply = sent(
        &destination
            .send(id, IFACE, b"from the far end", &[0x52; 16])
            .unwrap(),
    )
    .unwrap();
    let relayed_reply = sent(&relay.ingest(IFACE, &reply, 8)).unwrap();
    assert_eq!(
        data_from(&source.ingest(IFACE, &relayed_reply, 9)),
        Some((id, b"from the far end".to_vec())),
        "genuine data from the far end is still delivered"
    );
    assert_eq!(source.pause_assessment().latest_link_activity, Some(9));
    assert_eq!(
        data_from(&destination.ingest(IFACE, &relayed_reply, 9)),
        None,
        "the responder does not surface its own relayed reply either"
    );

    // The filter matches packets, not payloads: the far end sending the very bytes we
    // sent, under its own IV, is a different packet and is delivered.
    let same_bytes = sent(
        &destination
            .send(id, IFACE, b"from the source", &[0x53; 16])
            .unwrap(),
    )
    .unwrap();
    let relayed_same_bytes = sent(&relay.ingest(IFACE, &same_bytes, 10)).unwrap();
    assert_eq!(
        data_from(&source.ingest(IFACE, &relayed_same_bytes, 11)),
        Some((id, b"from the source".to_vec())),
        "equal plaintext from the far end is not mistaken for our own"
    );

    // The one collision is the far end reusing our IV for our plaintext, which yields our
    // packet byte for byte. That breaks the shared key's IV rule, and is refused as ours.
    let iv_reuse = sent(
        &destination
            .send(id, IFACE, b"from the source", &[0x51; 16])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(iv_reuse.hash(), own.hash());
    assert_eq!(data_from(&source.ingest(IFACE, &iv_reuse, 12)), None);
}

/// A shared medium hands us the far end's packet more than once: directly, and again
/// from a relay with hops+1 and the same hash. The application sees it once, and the
/// far end's next packet still arrives.
#[test]
fn a_far_end_packet_heard_twice_is_received_once() {
    let (mut a, mut b, id) = linked();
    let data_from = |actions: &Actions<8>| {
        actions.iter().find_map(|action| match action {
            Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
            _ => None,
        })
    };

    let theirs = sent(&b.send(id, IFACE, b"from b", &[0x61; 16]).unwrap()).unwrap();
    let mut via_relay = theirs.clone();
    via_relay.hops += 1;
    assert_eq!(via_relay.hash(), theirs.hash());

    assert_eq!(
        data_from(&a.ingest(IFACE, &theirs, 1)),
        Some((id, b"from b".to_vec()))
    );
    assert_eq!(
        data_from(&a.ingest(IFACE, &theirs, 2)),
        None,
        "a verbatim duplicate is not delivered again"
    );
    assert_eq!(
        data_from(&a.ingest(IFACE, &via_relay, 3)),
        None,
        "the relay's copy is not delivered again"
    );
    assert_eq!(
        a.pause_assessment().latest_link_activity,
        Some(1),
        "a copy is no newer evidence of the far end than the original"
    );

    let next = sent(&b.send(id, IFACE, b"second", &[0x62; 16]).unwrap()).unwrap();
    assert_eq!(
        data_from(&a.ingest(IFACE, &next, 4)),
        Some((id, b"second".to_vec()))
    );
}

/// The own-echo and duplicate windows hold their own bounds, not the route table's: a
/// node with four routes still knows its sixteenth-latest packet, and forgets the oldest
/// only past the small profile's constant.
#[test]
fn link_packet_windows_follow_their_own_constants_not_routes() {
    use crate::capacity::small::{DUPLICATE_HASHES, OWN_ECHO_HASHES};
    let node = |seed, name| {
        Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[seed; 64]),
            DestinationName::new("retinue", [name]).name_hash(),
        )
    };
    let (mut a, mut b) = (node(0x11, "a"), node(0x22, "b"));
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
    let id = link_up(&a.ingest(IFACE, &proof, 0)).unwrap();
    let delivered = |actions: &Actions<8>| {
        actions
            .iter()
            .any(|action| matches!(action, Action::Data { .. }))
    };
    let relayed = |packet: &Packet| {
        let mut copy = packet.clone();
        copy.hops += 1;
        copy
    };

    let ours: Vec<Packet> = (0..=OWN_ECHO_HASHES as u8)
        .map(|i| sent(&a.send(id, IFACE, b"ours", &[i; 16]).unwrap()).unwrap())
        .collect();
    assert!(
        !delivered(&a.ingest(IFACE, &relayed(&ours[1]), 1)),
        "an echo deeper than ROUTES is still ours"
    );
    assert!(
        delivered(&a.ingest(IFACE, &relayed(&ours[0]), 1)),
        "past the bound the oldest is forgotten"
    );

    let theirs: Vec<Packet> = (0..=DUPLICATE_HASHES as u8)
        .map(|i| sent(&b.send(id, IFACE, b"theirs", &[0x80 + i; 16]).unwrap()).unwrap())
        .collect();
    for packet in &theirs {
        assert!(delivered(&a.ingest(IFACE, packet, 2)));
    }
    assert!(
        !delivered(&a.ingest(IFACE, &relayed(&theirs[1]), 3)),
        "a copy deeper than ROUTES is still a copy"
    );
    assert!(
        delivered(&a.ingest(IFACE, &relayed(&theirs[0]), 3)),
        "past the bound the oldest is forgotten"
    );
}

/// Each dropped own echo and each dropped copy counts once, in its own counter, and a
/// delivered packet counts in neither.
#[test]
fn own_echo_and_duplicate_drops_are_counted_separately() {
    let (mut a, mut b, id) = linked();
    let delivered = |actions: &Actions<8>| {
        actions
            .iter()
            .any(|action| matches!(action, Action::Data { .. }))
    };
    let counts = |node: &Node<32, 8, 4>| {
        let c = node.transport_counters();
        (c.own_echo_dropped, c.duplicate_dropped)
    };
    let relayed = |packet: &Packet| {
        let mut copy = packet.clone();
        copy.hops += 1;
        copy
    };
    assert_eq!(counts(&a), (0, 0));

    let ours = sent(&a.send(id, IFACE, b"ours", &[0x71; 16]).unwrap()).unwrap();
    assert!(delivered(&b.ingest(IFACE, &ours, 1)));
    assert_eq!(counts(&b), (0, 0), "a delivered packet is not counted");
    assert!(!delivered(&a.ingest(IFACE, &relayed(&ours), 1)));
    assert_eq!(counts(&a), (1, 0));
    assert!(!delivered(&a.ingest(IFACE, &relayed(&ours), 2)));
    assert_eq!(counts(&a), (2, 0), "each echo counts once");

    let theirs = sent(&b.send(id, IFACE, b"theirs", &[0x72; 16]).unwrap()).unwrap();
    assert!(delivered(&a.ingest(IFACE, &theirs, 3)));
    assert_eq!(counts(&a), (2, 0), "a delivered packet is not counted");
    assert!(!delivered(&a.ingest(IFACE, &theirs, 4)));
    assert_eq!(counts(&a), (2, 1));
    assert!(!delivered(&a.ingest(IFACE, &relayed(&theirs), 5)));
    assert_eq!(counts(&a), (2, 2), "each copy counts once");

    let next = sent(&b.send(id, IFACE, b"next", &[0x73; 16]).unwrap()).unwrap();
    assert!(delivered(&a.ingest(IFACE, &next, 6)));
    assert_eq!(counts(&a), (2, 2));
    assert_eq!(counts(&b), (0, 0));
}

/// A link request for another destination is ignored by a non-transport node and must
/// never be answered as if its destination were local.
#[test]
fn a_link_request_for_another_destination_is_ignored() {
    let (mut a, b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();

    let mut c = Node::<32, 8, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0xCC; 64]),
        DestinationName::new("retinue", ["c"]).name_hash(),
    );
    assert!(c.ingest(IFACE, &request, 0).is_empty());
    assert_eq!(c.link_count(), 0);
}

/// A peer closing the link drops it and reports it.
#[test]
fn a_peer_closing_the_link_drops_it() {
    let (mut a, b, id) = linked();
    let close = b
        .links
        .iter()
        .find(|(l, _, _)| l.id() == id)
        .map(|(l, _, _)| l.close_packet(&[3; crate::token::IV_LEN]))
        .unwrap();

    let actions = a.ingest(IFACE, &close, 0);
    assert!(actions.iter().any(|x| matches!(x, Action::LinkDown { .. })));
    assert_eq!(a.link_count(), 0, "the link is gone");
    assert!(!a.has_link(id));
}

/// A full link table refuses new peers and keeps the ones it has.
#[test]
fn a_full_link_table_refuses_and_counts() {
    let mut server = Node::<32, 8, 1>::new(
        PrivateIdentity::from_secret_bytes(&[0x22; 64]),
        DestinationName::new("retinue", ["b"]).name_hash(),
    );
    let mut first = Node::<32, 8, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x11; 64]),
        DestinationName::new("retinue", ["a"]).name_hash(),
    );
    let mut second = Node::<32, 8, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0xDD; 64]),
        DestinationName::new("retinue", ["d"]).name_hash(),
    );
    let ann = server.announce(&blob([2; RAND_HASH_LEN]), None);
    first.ingest(IFACE, &ann, 0);
    second.ingest(IFACE, &ann, 0);

    let r1 = sent(
        &first
            .open_link(server.destination(), IFACE, &[0x31; 64], 0)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        server.ingest(IFACE, &r1, 0).len(),
        2,
        "accepted: proof plus LinkUp"
    );

    let r2 = sent(
        &second
            .open_link(server.destination(), IFACE, &[0x41; 64], 0)
            .unwrap(),
    )
    .unwrap();
    assert!(
        server.ingest(IFACE, &r2, 0).is_empty(),
        "refused, and nothing goes to the wire"
    );
    assert_eq!(server.link_count(), 1, "the established link survives");
    assert_eq!(server.refused_links(), 1, "and the refusal is counted");
}

/// Link traffic this gate does not handle is dropped rather than mishandled.
#[test]
fn unhandled_link_traffic_is_dropped() {
    let (mut a, b, id) = linked();
    let keepalive = b
        .links
        .iter()
        .find(|(l, _, _)| l.id() == id)
        .map(|(l, _, _)| l.keepalive_packet(0xff))
        .unwrap();
    assert!(a.ingest(IFACE, &keepalive, 0).is_empty());
    assert!(a.has_link(id), "and the link survives being spoken to");
}

/// A link's packets count only on the interface it came up on (`Link.py` 938-941), and a
/// copy refused elsewhere does not shadow the genuine one in the duplicate memory.
#[test]
fn link_data_counts_only_on_the_links_interface() {
    let (mut a, mut b, id) = linked();
    let data_from = |actions: &Actions<8>| {
        actions.iter().find_map(|action| match action {
            Action::Data { payload, .. } => Some(payload.clone()),
            _ => None,
        })
    };

    let theirs = sent(&b.send(id, IFACE, b"from b", &[0x71; 16]).unwrap()).unwrap();
    assert_eq!(data_from(&a.ingest(IFACE + 1, &theirs, 1)), None);
    assert_eq!(a.transport_counters().filtered_packets, 1);
    assert_eq!(a.pause_assessment().latest_link_activity, Some(0));
    assert_eq!(
        data_from(&a.ingest(IFACE, &theirs, 2)),
        Some(b"from b".to_vec())
    );

    // The responder holds its side to the interface the request came in on.
    let ours = sent(&a.send(id, IFACE, b"from a", &[0x72; 16]).unwrap()).unwrap();
    assert_eq!(data_from(&b.ingest(IFACE + 1, &ours, 3)), None);
    assert_eq!(
        data_from(&b.ingest(IFACE, &ours, 4)),
        Some(b"from a".to_vec())
    );
}
