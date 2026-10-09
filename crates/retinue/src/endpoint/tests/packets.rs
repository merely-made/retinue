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
