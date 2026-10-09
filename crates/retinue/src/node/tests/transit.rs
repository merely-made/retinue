//! Relaying announces and packets, the reverse table and the packet filter.

use super::*;
use crate::packet::MAX_HOPS;

/// A transport node relays both sides of a link setup: the announce makes the route
/// visible, the type-2 request reaches the destination, and the remembered link bridge
/// returns its proof. This is the smallest real transport transaction, not a broadcast
/// counter that could pass without carrying a packet.
#[test]
fn transport_relays_announce_request_and_proof() {
    let (mut source, mut destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());

    let announce = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
    let relayed_announce = relayed(&mut relay, IFACE, &announce, 0)
        .expect("a transport node re-broadcasts a verified announce");
    assert_eq!(relayed_announce.header_type, HeaderType::Type2);
    assert_eq!(relayed_announce.transport, Some(relay.identity.hash()));
    assert_eq!(relayed_announce.hops, 1);
    source.ingest(IFACE, &relayed_announce, 1);
    assert!(
        source.peers().knows(destination.destination()),
        "the source learned the destination through the relay"
    );

    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x99; 64], 1)
            .expect("the announced destination is linkable"),
    )
    .unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(relay.identity.hash());
    let forwarded_request = sent(&relay.ingest(IFACE, &request, 2))
        .expect("the type-2 request is carried toward its route");
    assert_eq!(forwarded_request.header_type, HeaderType::Type1);
    assert_eq!(forwarded_request.transport, None);
    assert_eq!(forwarded_request.hops, 1);

    let proof = sent(&destination.ingest(IFACE, &forwarded_request, 3))
        .expect("the destination accepts the transported request");
    let forwarded_proof = sent(&relay.ingest(IFACE, &proof, 4))
        .expect("the remembered bridge carries the proof back");
    assert_eq!(forwarded_proof.hops, 1);
    assert!(
        link_up(&source.ingest(IFACE, &forwarded_proof, 5)).is_some(),
        "the source completes the transported link"
    );
    let counters = relay.transport_counters();
    assert_eq!(counters.forwarded_announces, 1);
    assert_eq!(counters.forwarded_packets, 2);

    // An explicit interruption reports the transit obligation without
    // pretending to close either remote endpoint. Late link data loses its
    // return path, while learned destination/freshness state is retained.
    let id = source.links[0].0.id();
    let late = sent(
        &source
            .send(id, IFACE, b"after-relay-loss", &[0xA1; 16])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(relay.pause_assessment().transit_bridges, 1);
    let report = relay
        .force_interrupt(InterruptionPermission::AllowSessionLoss, || {
            panic!("transit-only node has no local link to close")
        })
        .unwrap();
    assert_eq!(report.transit_bridges.as_slice(), [id]);
    assert!(report.closed_links.is_empty());
    assert_eq!(relay.pause_assessment().transit_bridges, 0);
    assert!(relay.ingest(IFACE, &late, 6).is_empty());
    assert!(relay.peers().knows(destination.destination()));
}

/// A relay remembers the way back for every packet it carries, and carries the proof
/// back once, only from the interface the packet left by, within `REVERSE_TIMEOUT`
/// (RNS `Transport.py` 2104-2110, 863-870, 2733-2744).
#[test]
fn transport_carries_a_single_packet_proof_back_once() {
    const OUT: InterfaceId = IFACE + 1;
    let (_, destination) = pair();
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    relay.ingest(
        OUT,
        &destination.announce(&blob([0x77; RAND_HASH_LEN]), None),
        0,
    );

    let carry = |relay: &mut Node<32, 8, 4, 4>, byte: u8, now: u64| {
        let packet = Packet {
            ifac: false,
            header_type: HeaderType::Type2,
            context_flag: false,
            propagation: crate::packet::Propagation::Transport,
            destination_type: crate::packet::DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport: Some(relay.identity.hash()),
            destination: destination.destination(),
            context: 0,
            payload: vec![byte; 48],
        };
        let actions = relay.ingest(IFACE, &packet, now);
        assert!(matches!(
            actions.iter().next(),
            Some(Action::Send { interface: OUT, .. })
        ));
        crate::proof::proof_packet(&destination.identity, &packet.full_hash(), true)
    };

    // Carried back from the egress interface, once.
    let proof = carry(&mut relay, 1, 1);
    let actions = relay.ingest(OUT, &proof, 2);
    let Some(Action::Send { interface, packet }) = actions.iter().next() else {
        panic!("the proof is carried back");
    };
    assert_eq!(*interface, IFACE);
    assert_eq!(packet.hops, proof.hops + 1);
    assert_eq!(packet.payload, proof.payload);
    assert!(
        relay.ingest(OUT, &proof, 3).is_empty(),
        "the entry was consumed"
    );

    // A proof from any other interface consumes the entry without being carried.
    let proof = carry(&mut relay, 2, 4);
    assert!(relay.ingest(IFACE + 2, &proof, 5).is_empty());
    assert!(relay.ingest(OUT, &proof, 6).is_empty());

    // The way back is forgotten after REVERSE_TIMEOUT.
    let proof = carry(&mut relay, 3, 10);
    assert!(relay.ingest(OUT, &proof, 10 + REVERSE_TIMEOUT).is_empty());
    assert_eq!(relay.transport_counters().forwarded_packets, 4);
}

/// The reverse table is bounded by `ROUTES`: the oldest way back gives way.
#[test]
fn reverse_table_is_bounded_by_routes() {
    let (_, destination) = pair();
    let mut relay = Node::<32, 8, 4, 2>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    relay.ingest(
        IFACE + 1,
        &destination.announce(&blob([0x77; RAND_HASH_LEN]), None),
        0,
    );
    let mut proofs = vec![];
    for byte in 0..3u8 {
        let packet = Packet {
            ifac: false,
            header_type: HeaderType::Type2,
            context_flag: false,
            propagation: crate::packet::Propagation::Transport,
            destination_type: crate::packet::DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport: Some(relay.identity.hash()),
            destination: destination.destination(),
            context: 0,
            payload: vec![byte; 48],
        };
        assert!(sent(&relay.ingest(IFACE, &packet, u64::from(byte) + 1)).is_some());
        proofs.push(crate::proof::proof_packet(
            &destination.identity,
            &packet.full_hash(),
            false,
        ));
    }
    assert_eq!(relay.reverse.len(), 2);
    assert!(relay.ingest(IFACE + 1, &proofs[0], 5).is_empty());
    assert!(sent(&relay.ingest(IFACE + 1, &proofs[1], 5)).is_some());
    assert!(sent(&relay.ingest(IFACE + 1, &proofs[2], 5)).is_some());
}

/// Two transit packets that are different are both new; the same one heard again is a
/// loop. The window is its own two generations, not the route table's slots.
#[test]
fn the_transit_filter_outlasts_the_route_table() {
    let mut relay = node().with_transport_config(TransportConfig::transit());
    let packet = |n: u8| Packet {
        packet_type: PacketType::Data,
        header_type: HeaderType::Type1,
        transport: None,
        destination: AddressHash::from_bytes([n; 16]),
        context: 0,
        payload: vec![n],
        ..fixture("announce_appdata.bin")
    };
    assert!(relay.transit_is_new(&packet(0), 0));
    for n in 1..=TRANSPORT_DEDUP_HASHES as u8 {
        assert!(relay.transit_is_new(&packet(n), 1));
    }
    assert!(
        !relay.transit_is_new(&packet(0), 2),
        "a full generation turns over, it is not forgotten"
    );
    let mut keepalive = packet(0);
    keepalive.context = link::CTX_KEEPALIVE;
    assert!(relay.transit_is_new(&keepalive, 3));
    assert!(relay.transit_is_new(&keepalive, 3), "keepalives repeat");
    assert!(
        relay.transit_is_new(&packet(0), 2 * TRANSPORT_DEDUP_TIMEOUT + 2),
        "a quiet relay forgets after two timeouts"
    );
}

/// A packet addressed through another transport is not this node's to handle, even when
/// it names this node's destination: RNS filters it before dispatch.
#[test]
fn a_type_two_packet_for_another_transport_is_dropped() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let mut request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(AddressHash::from_bytes([0xEE; 16]));
    assert!(b.ingest(IFACE, &request, 0).is_empty());
    assert_eq!(b.transport_counters().filtered_packets, 1);
    request.transport = Some(b.identity.hash());
    assert!(sent(&b.ingest(IFACE, &request, 0)).is_some());
}

/// A leaf learns routes without relaying: it addresses its first relay and reaches a
/// destination two relays away, yet carries nothing for anyone else.
#[test]
fn non_transit_sender_addresses_its_first_relay_and_forwards_nothing() {
    let (mut source, mut destination) = pair();
    assert_eq!(source.transport_config(), TransportConfig::none());
    let transit = |seed, name| {
        Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[seed; 64]),
            DestinationName::new("retinue", [name]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit())
    };
    // source - near - far - destination
    let mut near = transit(0x48, "near");
    let mut far = transit(0x49, "far");

    let announce = destination.announce(&blob([0x79; RAND_HASH_LEN]), None);
    let via_far = relayed(&mut far, IFACE, &announce, 0).unwrap();
    let via_near = relayed(&mut near, IFACE, &via_far, 1).unwrap();
    let heard = source.ingest(IFACE, &via_near, 2);
    assert!(sent(&heard).is_none(), "a leaf does not re-broadcast");
    let hop = source.next_hop(destination.destination(), 2).unwrap();
    assert_eq!((hop.via, hop.hops), (Some(near.identity.hash()), 2));

    let request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x9B; 64], 2)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request.header_type, HeaderType::Type2);
    assert_eq!(request.transport, Some(near.identity.hash()));
    let at_far = sent(&near.ingest(IFACE, &request, 3)).unwrap();
    assert_eq!(at_far.transport, Some(far.identity.hash()));
    let at_destination = sent(&far.ingest(IFACE, &at_far, 4)).unwrap();
    let proof = sent(&destination.ingest(IFACE, &at_destination, 5)).unwrap();
    let proof = sent(&far.ingest(IFACE, &proof, 6)).unwrap();
    let proof = sent(&near.ingest(IFACE, &proof, 7)).unwrap();
    let id = link_up(&source.ingest(IFACE, &proof, 8)).expect("the link comes up");

    let data = sent(&source.send(id, IFACE, b"two relays", &[0xB1; 16]).unwrap()).unwrap();
    let data = sent(&near.ingest(IFACE, &data, 9)).unwrap();
    let data = sent(&far.ingest(IFACE, &data, 10)).unwrap();
    assert!(
        destination.ingest(IFACE, &data, 11).iter().any(
            |action| matches!(action, Action::Data { payload, .. } if payload == b"two relays")
        )
    );

    // A request naming the leaf as its transport, for a destination it has a route to.
    let mut through_source = request.clone();
    through_source.transport = Some(source.identity.hash());
    assert!(sent(&source.ingest(IFACE, &through_source, 12)).is_none());
    let counters = source.transport_counters();
    assert_eq!(
        (counters.forwarded_announces, counters.forwarded_packets),
        (0, 0)
    );
}

#[test]
fn foreign_interface_cannot_refresh_or_poison_a_link_bridge() {
    let mut relay = Node::<8, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x45; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let link_id = AddressHash::from_bytes([0xA3; 16]);
    relay.remember_bridge(link_id, AddressHash::from_bytes([0xA4; 16]), 1, 2, 20, 10);
    relay.bridges[0].proof_deadline = None;
    let mut packet = Packet {
        packet_type: PacketType::Data,
        header_type: HeaderType::Type1,
        transport: None,
        destination: link_id,
        payload: b"bridged data".to_vec(),
        ..fixture("announce_appdata.bin")
    };

    assert!(relay.ingest(3, &packet, 20).is_empty());
    assert_eq!(relay.bridges[0].seen, 10);
    assert!(relay.transit_filter.current.is_empty());

    let to_two = relay.ingest(1, &packet, 21);
    assert!(
        to_two
            .iter()
            .any(|action| matches!(action, Action::Send { interface: 2, .. }))
    );
    assert_eq!(relay.bridges[0].seen, 21);

    packet.payload = b"return data".to_vec();
    let to_one = relay.ingest(2, &packet, 22);
    assert!(
        to_one
            .iter()
            .any(|action| matches!(action, Action::Send { interface: 1, .. }))
    );
    assert_eq!(relay.transport_counters().forwarded_packets, 2);
}

fn transit_relay() -> Node<32, 8, 4, 4> {
    Node::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit())
}

/// RNS learns an announce heard with up to 127 wire hops (`Transport.py` 2211) but
/// rebroadcasts only below `PATHFINDER_M` (`Transport.py` 1356): 126 goes on as 127, and 127
/// is learned and kept.
#[test]
fn an_announce_is_relayed_only_below_the_hop_ceiling() {
    let (_, destination) = pair();
    let mut relay = transit_relay();
    let mut announce = destination.announce(&blob([0x78; RAND_HASH_LEN]), None);
    announce.hops = MAX_HOPS - 1;
    let heard = relay.ingest(IFACE, &announce, 0);
    assert!(sent(&heard).is_none());
    assert!(heard.iter().any(|a| matches!(a, Action::Learned { .. })));
    assert_eq!(relay.route_count(), 1);
    assert_eq!(relay.transport_counters().hop_limit_dropped, 1);
    assert_eq!(relay.next_rebroadcast(), None, "nor scheduled");

    let mut relay = transit_relay();
    announce.hops = MAX_HOPS - 2;
    let relayed = relayed(&mut relay, IFACE, &announce, 0).expect("relayed below the ceiling");
    assert_eq!(relayed.hops, MAX_HOPS - 1);
}

/// A carried type-2 packet goes on only while its incremented hops stay below the ceiling,
/// and one refused at it does not shadow a later copy in the transit filter.
#[test]
fn a_transport_packet_is_carried_only_below_the_hop_ceiling() {
    let (mut source, destination) = pair();
    let mut relay = transit_relay();
    let announce = destination.announce(&blob([0x79; RAND_HASH_LEN]), None);
    source.ingest(IFACE, &relayed(&mut relay, IFACE, &announce, 0).unwrap(), 1);
    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x9A; 64], 1)
            .unwrap(),
    )
    .unwrap();
    assert_eq!(request.transport, Some(relay.identity.hash()));

    request.hops = MAX_HOPS - 1;
    assert!(sent(&relay.ingest(IFACE, &request, 2)).is_none());
    assert_eq!(relay.transport_counters().hop_limit_dropped, 1);
    request.hops = MAX_HOPS - 2;
    let carried = sent(&relay.ingest(IFACE, &request, 3)).expect("carried below the ceiling");
    assert_eq!(carried.hops, MAX_HOPS - 1);
}
