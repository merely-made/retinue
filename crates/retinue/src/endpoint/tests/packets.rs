//! Single packets, receipts, the packet filter, the inbound lanes and interface counters.

use super::*;

/// The outbound receipt table is bounded: past `SINGLE_RECEIPTS` the oldest receipt is
/// culled and says so (RNS `Transport.py` 744-749).
#[tokio::test]
async fn the_oldest_receipt_is_culled_at_capacity() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0xB3; 64]));
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
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x65; 64]));
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
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
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
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
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

    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x6B; 64]));
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

fn path_request_packet(seq: u8) -> Packet {
    crate::path::path_request(AddressHash::from_bytes([seq; 16]), &[seq; 16])
}

fn data_packet(seq: u8) -> Packet {
    single_packet(AddressHash::from_bytes([0x44; 16]), vec![seq])
}

/// The router drains data, then announces, then path requests, then burst-limited path
/// requests, and only the last are marked ingress-limited (`Transport.py` 70-80).
#[tokio::test]
async fn inbound_lanes_drain_in_priority_order() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x50; 64]));
    let (calm, bursting) = (ep.attach_interface(), ep.attach_interface());
    bursting
        .stats
        .path_request_burst
        .store(true, Ordering::Relaxed);
    let (tx, mut rx) = super::super::inbound_queue::channel();
    tx.bind(&ep.shared);
    let (announce, _) = peer_announce(0x51, "lanes");
    tx.try_send(bursting.id(), path_request_packet(2)).unwrap();
    tx.try_send(calm.id(), path_request_packet(1)).unwrap();
    tx.try_send(calm.id(), announce).unwrap();
    tx.try_send(calm.id(), data_packet(1)).unwrap();
    assert_eq!(rx.heights(), [1, 1, 1, 1]);
    let mut order = Vec::new();
    for _ in 0..4 {
        let inbound = rx.recv().await.unwrap();
        let pkt = &inbound.packet;
        order.push((
            pkt.packet_type,
            pkt.destination_type,
            inbound.iface,
            inbound.ingress_limited,
        ));
    }
    let (calm, bursting) = (calm.id(), bursting.id());
    assert_eq!(
        order,
        [
            (PacketType::Data, DestinationType::Single, calm, false),
            (PacketType::Announce, DestinationType::Single, calm, false),
            (PacketType::Data, DestinationType::Plain, calm, false),
            (PacketType::Data, DestinationType::Plain, bursting, true),
        ]
    );
}

/// A full lane drops its own class only, counted per lane, and a path request from an
/// interface in a path-request burst takes the ingress-limited lane.
#[tokio::test]
async fn a_full_lane_drops_only_its_own_class() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x52; 64]));
    let wire = ep.attach_interface();
    let queue = &ep.shared.inbound_queue;
    let (announce, _) = peer_announce(0x53, "flood");
    for _ in 0..128 {
        queue.try_send(wire.id(), announce.clone()).unwrap();
    }
    assert_eq!(
        queue.try_send(wire.id(), announce),
        Err(super::super::inbound_queue::TrySendError::Full)
    );
    assert!(queue.try_send(wire.id(), data_packet(1)).is_ok());
    assert!(queue.try_send(wire.id(), path_request_packet(1)).is_ok());
    wire.stats.path_request_burst.store(true, Ordering::Relaxed);
    for seq in 0..9 {
        let _ = queue.try_send(wire.id(), path_request_packet(seq));
    }
    let counters = ep.routing_counters();
    assert_eq!(counters.inbound_announces_dropped, 1);
    assert_eq!(counters.inbound_ingress_limited_dropped, 1);
    assert_eq!(counters.inbound_data_dropped, 0);
    assert_eq!(counters.inbound_path_requests_dropped, 0);
    let stats = ep.interface_counters(wire.id()).unwrap();
    assert_eq!(stats.announces_rx, 129);
    assert_eq!(stats.path_requests_rx, 10);
}

/// A stream reader waits for room in the data lane, but never in another.
#[tokio::test]
async fn only_the_data_lane_backpressures_a_sender() {
    let (tx, mut rx) = super::super::inbound_queue::channel();
    for seq in 0..1024 {
        tx.try_send(1, data_packet(seq as u8)).unwrap();
    }
    let (announce, _) = peer_announce(0x54, "backpressure");
    for _ in 0..129 {
        assert!(
            tx.send(1, announce.clone()).await,
            "a full announce lane drops"
        );
    }
    assert_eq!(rx.heights(), [1024, 128, 0, 0]);
    let waiting = tokio::spawn({
        let tx = tx.clone();
        async move { tx.send(1, data_packet(0xFF)).await }
    });
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(!waiting.is_finished(), "a full data lane waits");
    assert!(rx.recv().await.is_some());
    assert!(waiting.await.unwrap());
    drop(rx);
    assert!(
        !tx.send(1, data_packet(0)).await,
        "a stopped router is reported"
    );
}

/// The interface counters see frames both ways, classified, with the carrier's state.
#[tokio::test]
async fn interface_counters_classify_traffic() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x55; 64]));
    let wire = ep.attach_interface();
    let id = wire.id();
    let (out, sink) = wire.split();
    let (announce, _) = peer_announce(0x56, "counters");
    let frame = announce.encode();
    assert!(sink.deliver_frame(&frame).unwrap());
    let request = path_request_packet(7);
    out.encode(&request).unwrap();
    let mut flagged = data_packet(1).encode();
    flagged[0] |= 0x80;
    assert!(sink.deliver_frame(&flagged).unwrap());
    tokio::time::sleep(Duration::from_millis(50)).await;

    let c = ep.interface_counters(id).unwrap();
    assert_eq!(
        (c.rx_packets, c.rx_bytes),
        (2, (frame.len() + flagged.len()) as u64)
    );
    assert_eq!(
        (c.announces_rx, c.announce_rx_bytes),
        (1, frame.len() as u64)
    );
    assert_eq!((c.tx_packets, c.path_requests_tx), (1, 1));
    assert_eq!(c.path_request_tx_bytes, request.encoded_len() as u64);
    assert_eq!(c.ifac_violations, 1, "a flagged frame on a plain interface");
    assert!(c.online && c.mode == InterfaceMode::Full && !c.burst_active);
    assert!(alloc::format!("{c}").contains("\"ifac_violations\":1,"));
}

/// Frame failures are classified as stock does (`Transport.py` 1764-1794): an unflagged or
/// wrongly keyed frame is an IFAC violation; a headerless one, or one past the MTU under the
/// right key, is a protocol one.
#[tokio::test]
async fn frame_failures_are_classified_as_stock_does() {
    let ep = rns160(PrivateIdentity::from_secret_bytes(&[0x55; 64]));
    let ifac = crate::ifac::Ifac::for_stream(Some("stats"), None).unwrap();
    let wire = ep.attach_interface_with_ifac(2048, ifac).unwrap();
    let sink = wire.sink();
    let other = crate::ifac::Ifac::for_stream(Some("other"), None).unwrap();
    let plain = data_packet(1).encode();
    for frame in [plain.clone(), other.seal(&plain).unwrap()] {
        assert!(sink.deliver_frame(&frame).is_err());
    }
    // Past the interface's MTU but under the right key: IFAC passes, then decoding fails.
    // A wrongly keyed oversized frame is an IFAC violation, as stock checks IFAC first.
    let keyed = crate::ifac::Ifac::for_stream(Some("stats"), None).unwrap();
    let oversized = keyed.seal(&[0x00; crate::packet::MTU + 64]).unwrap();
    for frame in [vec![0x80, 0], oversized] {
        assert!(sink.deliver_frame(&frame).is_err());
    }
    let stats = ep.interface_counters(wire.id()).unwrap();
    assert_eq!((stats.ifac_violations, stats.protocol_violations), (2, 2));
    assert_eq!(stats.rx_packets, 4);
}
