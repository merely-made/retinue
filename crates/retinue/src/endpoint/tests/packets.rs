//! Single packets, receipts, and the packet filter.

use super::*;

/// The outbound receipt table is bounded: past `SINGLE_RECEIPTS` the oldest receipt is
/// culled and says so (RNS `Transport.py` 744-749).
#[tokio::test]
async fn the_oldest_receipt_is_culled_at_capacity() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xB3; 64]));
    let _wire = ep.attach_interface();
    let (_, peer) = peer_announce(0xB4, "receipts");
    ep.shared.address_book.lock().unwrap().ingest(&peer);

    let mut receipts: Vec<_> = (0..=SINGLE_RECEIPTS)
        .map(|n| ep.send_single(peer.destination, &[n as u8; 8]).unwrap())
        .collect();
    assert_eq!(
        ep.shared.single_receipts.lock().unwrap().len(),
        SINGLE_RECEIPTS
    );
    let oldest = receipts.remove(0);
    assert_eq!(oldest.delivery().await, SingleDelivery::Culled);
}

/// A packet addressed through another transport is filtered before local dispatch, even a
/// link request naming one of our own destinations.
#[tokio::test]
async fn a_type_two_packet_for_another_transport_is_not_dispatched() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x65; 64]));
    let name = crate::destination::DestinationName::new("retinue", ["elsewhere"]);
    let dest = name.destination_hash(ep.identity());
    ep.register(name, b"");
    let a = ep.attach_interface();
    let (_, mut request) = link::PendingLink::open(
        dest,
        *ep.identity(),
        &[0x66; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: DEFAULT_LINK_MTU,
        },
    );
    request.header_type = crate::packet::HeaderType::Type2;
    request.transport = Some(AddressHash::from_bytes([0xEE; 16]));
    route(&ep.shared, a.id(), request.clone());
    assert!(
        a.outbound.queues.pop().is_none(),
        "no proof for another's packet"
    );
    assert_eq!(ep.routing_counters().filtered_packets, 1);

    request.header_type = crate::packet::HeaderType::Type1;
    request.transport = None;
    route(&ep.shared, a.id(), request);
    assert_eq!(
        a.outbound.queues.pop().map(|proof| proof.packet_type),
        Some(PacketType::Proof)
    );
}

/// A single packet heard twice, directly and from a relay, is delivered once; a transit
/// packet heard twice is carried once.
#[tokio::test]
async fn repeated_single_and_transit_packets_are_filtered() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
    let name = crate::destination::DestinationName::new("retinue", ["single"]);
    let dest = name.destination_hash(ep.identity());
    ep.register(name, b"");
    let a = ep.attach_interface();
    let b = ep.attach_interface();
    ep.enable_routing();

    let payload =
        crate::token::encrypt_to_identity(ep.identity(), &[7; KEY_LEN], &[8; IV_LEN], b"once");
    let single = single_packet(dest, payload);
    route(&ep.shared, a.id(), single.clone());
    route(&ep.shared, b.id(), single);
    assert_eq!(ep.accept_single().await.unwrap().data, b"once");
    assert!(
        tokio::time::timeout(Duration::from_millis(100), ep.accept_single())
            .await
            .is_err(),
        "the copy is not delivered"
    );

    let onward = AddressHash::from_bytes([0xD1; 16]);
    ep.shared.learn_path(onward, b.id(), 0, None);
    let mut transit = single_packet(onward, vec![5; 64]);
    transit.header_type = crate::packet::HeaderType::Type2;
    transit.transport = Some(ep.identity().hash());
    route(&ep.shared, a.id(), transit.clone());
    transit.hops = 3;
    route(&ep.shared, a.id(), transit);
    assert!(b.outbound.queues.pop().is_some());
    b.outbound.queues.delivery_complete();
    assert!(
        b.outbound.queues.pop().is_none(),
        "the loop's copy is dropped"
    );
    let counters = ep.routing_counters();
    assert_eq!(
        (counters.forwarded_packets, counters.filtered_packets),
        (1, 2)
    );
}

/// A link's packets count only on the link's own interface (`Link.py` 938-941,
/// `Transport.py` 2573-2574). The copy refused elsewhere is not remembered as seen, so the
/// same packet arriving on the right interface is still delivered (`Transport.py` 2585-2593).
#[tokio::test]
async fn link_data_counts_only_on_the_links_interface() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
    let name = crate::destination::DestinationName::new("retinue", ["bound"]);
    let dest = name.destination_hash(ep.identity());
    let (a, b) = (ep.attach_interface(), ep.attach_interface());
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: DEFAULT_LINK_MTU,
    };
    let (pending, request) = link::PendingLink::open(dest, *ep.identity(), &[0x68; 64], trailer);
    let (server, proof) =
        link::accept(&request, &ep.shared.identity, &[0x69; 64], trailer).unwrap();
    let client = pending.prove(&proof).unwrap();
    let mut stream = register_stream(
        &ep.shared,
        server,
        a.id(),
        Liveness::responder(0, 0),
        LinkDirection::Inbound,
        LinkRemoteFact::default(),
    )
    .unwrap();

    let data = client.data_packet(b"bound", &[0x6A; IV_LEN]);
    route(&ep.shared, b.id(), data.clone());
    assert_eq!(ep.routing_counters().filtered_packets, 1);
    route(&ep.shared, a.id(), data);
    let mut got = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut got))
        .await
        .expect("delivered on the link's interface")
        .unwrap();
    assert_eq!(&got, b"bound");
}

/// An endpoint with a resource session, the far end's link, and two IDENTIFYs from that
/// end, already routed.
fn identify_fixture(
    direction: LinkDirection,
) -> (
    Endpoint,
    Interface,
    link::Link,
    super::super::resource_session::ResourceSession,
) {
    use super::super::resource_session::register_resource_session;

    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x6B; 64]));
    let iface = ep.attach_interface();
    let name = crate::destination::DestinationName::new("retinue", ["identify"]);
    let trailer = LinkTrailer {
        mode: LinkMode::Aes256Cbc,
        mtu: DEFAULT_LINK_MTU,
    };
    let dest = name.destination_hash(ep.identity());
    let (pending, request) = link::PendingLink::open(dest, *ep.identity(), &[0x6C; 64], trailer);
    let (server, proof) =
        link::accept(&request, &ep.shared.identity, &[0x6D; 64], trailer).unwrap();
    let client = pending.prove(&proof).unwrap();
    let session = register_resource_session(
        &ep.shared,
        server,
        iface.id(),
        Liveness::responder(0, 0),
        direction,
        LinkRemoteFact::default(),
    )
    .unwrap();
    for (seed, iv) in [(0x6E, 1), (0x6F, 2)] {
        let who = PrivateIdentity::from_secret_bytes(&[seed; 64]);
        route(
            &ep.shared,
            iface.id(),
            client.identify_packet(&who, &[iv; IV_LEN]),
        );
    }
    (ep, iface, client, session)
}

/// A resource session keeps the first identity its peer proves; a later IDENTIFY does not
/// replace it (`Link.py` 973-990), whether a request or a payload follows.
#[tokio::test]
async fn a_later_identify_does_not_replace_the_first() {
    let first = *PrivateIdentity::from_secret_bytes(&[0x6E; 64]).public();

    let (ep, iface, client, mut session) = identify_fixture(LinkDirection::Inbound);
    route(
        &ep.shared,
        iface.id(),
        client.request_packet(b"\x93", &[3; IV_LEN]),
    );
    let received = tokio::time::timeout(Duration::from_secs(2), session.receive_raw_request())
        .await
        .expect("the request arrives")
        .unwrap();
    assert_eq!(received.peer, Some(first));

    // Only a responder takes an IDENTIFY (`Link.py` 973).
    for (direction, peer) in [
        (LinkDirection::Inbound, Some(first)),
        (LinkDirection::Outbound, None),
    ] {
        let (ep, iface, client, mut session) = identify_fixture(direction);
        route(
            &ep.shared,
            iface.id(),
            client.data_packet(b"payload", &[3; IV_LEN]),
        );
        tokio::time::timeout(Duration::from_secs(2), session.receive())
            .await
            .expect("the payload arrives")
            .unwrap();
        assert_eq!(session.identified_peer, peer, "{direction:?}");
    }
}
