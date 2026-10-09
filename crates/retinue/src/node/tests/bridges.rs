//! Carried links: MTU clamping, proof validation and bridge-slot pressure.

use super::*;

/// Where a set of actions sends its first packet, and what.
fn sent_on<const N: usize>(actions: &Actions<N>) -> Option<(InterfaceId, Packet)> {
    actions.iter().find_map(|a| match a {
        Action::Send { interface, packet } => Some((*interface, packet.clone())),
        _ => None,
    })
}

const SOURCE_SIDE: InterfaceId = 1;
const DESTINATION_SIDE: InterfaceId = 2;

/// A transit relay between `source` (on [`SOURCE_SIDE`]) and `destination` (on
/// [`DESTINATION_SIDE`]), the relayed announce learned by the source, and the source's
/// type-2 request already forwarded. Returns the relay and the forwarded request.
fn relayed_request(
    relay_mtu: u32,
    source: &mut Node<32, 8, 4>,
    destination: &Node<32, 8, 4>,
) -> (Node<32, 8, 4, 4>, Packet) {
    let mut relay = Node::<32, 8, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    relay.set_logical_mtu(relay_mtu).unwrap();
    let announce = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
    let relayed = relayed(&mut relay, DESTINATION_SIDE, &announce, 0).unwrap();
    source.ingest(IFACE, &relayed, 1);
    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x99; 64], 1)
            .unwrap(),
    )
    .unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(relay.identity.hash());
    let (out, forwarded) = sent_on(&relay.ingest(SOURCE_SIDE, &request, 2)).unwrap();
    assert_eq!(out, DESTINATION_SIDE);
    assert_eq!(link::link_id(&forwarded), link::link_id(&request));
    (relay, forwarded)
}

/// A relay that carries 247 bytes lowers the signalled MTU of a 255-byte request, and
/// both ends then agree on 247 rather than on frames the relay would refuse.
#[test]
fn a_relay_clamps_the_link_mtu_to_what_it_carries() {
    let (mut source, mut destination) = pair();
    let (mut relay, forwarded) = relayed_request(247, &mut source, &destination);
    assert_eq!(
        link::request_trailer(&forwarded).unwrap().map(|t| t.mtu),
        Some(247)
    );

    let proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
    assert_eq!(destination.links[0].0.mtu(), 247);
    let (back, proof) = sent_on(&relay.ingest(DESTINATION_SIDE, &proof, 4)).unwrap();
    assert_eq!(back, SOURCE_SIDE);
    assert!(link_up(&source.ingest(IFACE, &proof, 5)).is_some());
    assert_eq!(source.links[0].0.mtu(), 247);
}

/// A destination offers the smaller of its own budget and the request's, so a 247-byte
/// initiator is not held to a 255-byte link it cannot carry.
#[test]
fn a_responder_offers_no_more_than_was_requested() {
    let (mut a, mut b) = pair();
    a.set_logical_mtu(247).unwrap();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
    let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
    assert_eq!(b.links[0].0.mtu(), 247);
    link_up(&a.ingest(IFACE, &proof, 0)).unwrap();
    assert_eq!(a.links[0].0.mtu(), 247);
}

/// A bridge carries nothing until the destination proves it: early link traffic, a proof
/// signed by someone else, and a genuine proof from the initiator's side are all dropped.
#[test]
fn a_bridge_carries_nothing_until_the_destination_proves_it() {
    let (mut source, mut destination) = pair();
    let (mut relay, forwarded) = relayed_request(LINK_MTU, &mut source, &destination);
    let link_id = link::link_id(&forwarded).unwrap();
    let early = Packet {
        packet_type: PacketType::Data,
        header_type: HeaderType::Type1,
        transport: None,
        destination: link_id,
        payload: b"before the proof".to_vec(),
        ..fixture("announce_appdata.bin")
    };
    assert!(relay.ingest(SOURCE_SIDE, &early, 3).is_empty());

    let impostor = PrivateIdentity::from_secret_bytes(&[0x66; 64]);
    let (_, forged) = link::accept(
        &forwarded,
        &impostor,
        &[0x67; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: LINK_MTU,
        },
    )
    .unwrap();
    assert!(relay.ingest(DESTINATION_SIDE, &forged, 3).is_empty());

    let proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
    assert!(relay.ingest(SOURCE_SIDE, &proof, 4).is_empty());
    assert_eq!(relay.transport_counters().unvalidated_link_packets, 3);
    assert!(relay.bridges[0].proof_deadline.is_some());

    let (back, proof) = sent_on(&relay.ingest(DESTINATION_SIDE, &proof, 5)).unwrap();
    assert_eq!(back, SOURCE_SIDE);
    assert!(relay.bridges[0].proof_deadline.is_none());
    link_up(&source.ingest(IFACE, &proof, 6)).unwrap();
    let data = sent(&source.send(link_id, IFACE, b"after", &[0xA1; 16]).unwrap()).unwrap();
    let (out, _) = sent_on(&relay.ingest(SOURCE_SIDE, &data, 7)).unwrap();
    assert_eq!(out, DESTINATION_SIDE);
}

/// An unproved bridge lapses at its proof deadline: one per-hop allowance for each hop
/// still ahead, as RNS gives a carried request. A late proof is not carried.
#[test]
fn an_unproved_bridge_lapses_at_its_proof_deadline() {
    let (mut source, mut destination) = pair();
    let (mut relay, forwarded) = relayed_request(LINK_MTU, &mut source, &destination);
    let proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
    let deadline = 2 + LINK_ESTABLISHMENT_TIMEOUT_PER_HOP;
    assert_eq!(transit_proof_timeout(0), LINK_ESTABLISHMENT_TIMEOUT_PER_HOP);
    assert_eq!(relay.bridges[0].proof_deadline, Some(deadline));
    assert!(relay.ingest(DESTINATION_SIDE, &proof, deadline).is_empty());
    assert!(relay.bridges.is_empty());
    assert_eq!(relay.transport_counters().expired_bridges, 1);
}

/// At capacity a new request displaces the stalest unproved bridge, never a validated
/// one; with every slot validated it is refused and not carried.
#[test]
fn forged_requests_cannot_displace_a_validated_bridge() {
    let mut relay = Node::<8, 8, 4, 2>::new(
        PrivateIdentity::from_secret_bytes(&[0x45; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let id = |byte| AddressHash::from_bytes([byte; 16]);
    assert!(relay.remember_bridge(id(1), id(0), 1, 2, 100, 0));
    relay.bridges[0].proof_deadline = None;
    assert!(relay.remember_bridge(id(2), id(0), 1, 2, 100, 1));
    assert!(relay.remember_bridge(id(3), id(0), 1, 2, 100, 2));
    assert_eq!(relay.transport_counters().evicted_bridges, 1);
    assert!(relay.bridges.iter().any(|b| b.link_id == id(1)));
    assert!(!relay.bridges.iter().any(|b| b.link_id == id(2)));

    // A replayed request leaves a validated bridge as it was.
    assert!(relay.remember_bridge(id(1), id(0), 3, 3, 100, 3));
    let validated = relay.bridges.iter().find(|b| b.link_id == id(1)).unwrap();
    assert_eq!((validated.from, validated.out, validated.seen), (1, 2, 0));
    assert!(validated.proof_deadline.is_none());

    relay
        .bridges
        .iter_mut()
        .for_each(|b| b.proof_deadline = None);
    assert!(!relay.remember_bridge(id(4), id(0), 1, 2, 100, 4));
    assert_eq!(relay.transport_counters().refused_bridges, 1);
    assert_eq!(relay.bridges.len(), 2);
}

/// The source's next type-2 link request to `destination` through `relay`.
fn next_request(
    source: &mut Node<32, 8, 4>,
    destination: &Node<32, 8, 4>,
    relay: &Node<32, 8, 4, 4>,
    seed: u8,
    now: u64,
) -> Packet {
    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[seed; 64], now)
            .unwrap(),
    )
    .unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(relay.identity.hash());
    request
}

/// A relay whose every slot holds a recently heard validated link carries no new request
/// at all; once those links have gone unheard for the RNS link timeout, the stalest one
/// yields and the request is carried.
#[test]
fn idle_validated_bridges_yield_but_live_ones_refuse_new_requests() {
    let (mut source, destination) = pair();
    let (mut relay, _) = relayed_request(LINK_MTU, &mut source, &destination);
    let id = |byte| AddressHash::from_bytes([byte; 16]);
    for byte in 1..4 {
        assert!(relay.remember_bridge(id(byte), id(0), 1, 2, 100, 2));
    }
    relay
        .bridges
        .iter_mut()
        .for_each(|b| b.proof_deadline = None);
    relay.bridges[3].seen = 3;

    let request = next_request(&mut source, &destination, &relay, 0x9A, 10);
    assert!(sent(&relay.ingest(SOURCE_SIDE, &request, 10)).is_none());
    assert_eq!(relay.transport_counters().refused_bridges, 1);
    assert_eq!(relay.bridges.len(), 4);

    let late = 2 + LINK_TRANSPORT_IDLE;
    let request = next_request(&mut source, &destination, &relay, 0x9B, late);
    let (out, carried) = sent_on(&relay.ingest(SOURCE_SIDE, &request, late)).unwrap();
    assert_eq!(out, DESTINATION_SIDE);
    assert_eq!(relay.transport_counters().evicted_bridges, 1);
    let carried_id = link::link_id(&carried).unwrap();
    assert!(relay.bridges.iter().any(|b| b.link_id == carried_id));
    assert!(
        relay.bridges.iter().any(|b| b.seen == 3),
        "the most recently heard idle link stays"
    );
}

/// A request the relay cannot send, for want of action space, takes no bridge slot.
#[test]
fn an_unsent_request_takes_no_bridge_slot() {
    let (mut source, destination) = pair();
    let mut relay = Node::<32, 0, 4, 4>::new(
        PrivateIdentity::from_secret_bytes(&[0x44; 64]),
        DestinationName::new("retinue", ["relay"]).name_hash(),
    )
    .with_transport_config(TransportConfig::transit());
    let announce = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
    relay.ingest(DESTINATION_SIDE, &announce, 0);
    assert_eq!(relay.routes.len(), 1);
    source.ingest(IFACE, &announce, 1);
    let mut request = sent(
        &source
            .open_link(destination.destination(), IFACE, &[0x9F; 64], 1)
            .unwrap(),
    )
    .unwrap();
    request.header_type = HeaderType::Type2;
    request.transport = Some(relay.identity.hash());
    assert_eq!(relay.ingest(SOURCE_SIDE, &request, 2).overflowed(), 1);
    assert!(relay.bridges.is_empty());
}

/// A request whose MTU must be lowered under a link mode this relay cannot encode is
/// dropped and takes no slot; one that already fits passes as it is.
#[test]
fn a_relay_drops_a_request_it_cannot_clamp() {
    let (mut source, destination) = pair();
    let (mut relay, _) = relayed_request(247, &mut source, &destination);
    let mut request = next_request(&mut source, &destination, &relay, 0x9C, 3);
    request.payload[link::LINK_KEYS_LEN] = 0xe0;
    assert!(relay.ingest(SOURCE_SIDE, &request, 3).is_empty());
    assert_eq!(relay.transport_counters().undecodable_link_requests, 1);
    assert_eq!(relay.bridges.len(), 1);

    let mut fitting = next_request(&mut source, &destination, &relay, 0x9D, 4);
    let trailer = fitting.payload.len() - link::TRAILER_LEN;
    fitting.payload[trailer..].copy_from_slice(&[0xe0, 0, 200]);
    let (_, carried) = sent_on(&relay.ingest(SOURCE_SIDE, &fitting, 4)).unwrap();
    assert_eq!(carried.payload[trailer..], [0xe0, 0, 200]);
}

/// With the destination's identity unknown, a bridge cannot check the proof's signature;
/// the proof is still carried only from the destination's side.
#[test]
fn without_the_destinations_identity_only_its_side_proves() {
    let (mut source, mut destination) = pair();
    let (mut relay, forwarded) = relayed_request(LINK_MTU, &mut source, &destination);
    relay.book.forget(destination.destination());
    let proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
    assert!(relay.ingest(SOURCE_SIDE, &proof, 4).is_empty());
    assert!(relay.bridges[0].proof_deadline.is_some());
    let (back, _) = sent_on(&relay.ingest(DESTINATION_SIDE, &proof, 5)).unwrap();
    assert_eq!(back, SOURCE_SIDE);
    assert!(relay.bridges[0].proof_deadline.is_none());
}

/// A proof the relay does not send on, here for the hop ceiling, leaves its bridge
/// unvalidated; RNS marks a bridge validated only as it transmits the proof.
#[test]
fn a_proof_validates_its_bridge_only_once_carried() {
    let (mut source, mut destination) = pair();
    let (mut relay, forwarded) = relayed_request(LINK_MTU, &mut source, &destination);
    let mut proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
    proof.hops = relay.transport.max_hops;
    assert!(relay.ingest(DESTINATION_SIDE, &proof, 4).is_empty());
    assert_eq!(relay.transport_counters().hop_limit_dropped, 1);
    assert!(relay.bridges[0].proof_deadline.is_some());
}

/// A destination treats a signalled MTU of 0 as RNS's 500-byte default, and holds a
/// request below the smallest workable budget to that floor.
#[test]
fn a_responder_floors_what_it_offers() {
    let (mut a, mut b) = pair();
    a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
    for (signalled, offered) in [(0, LINK_MTU), (1, MIN_LOGICAL_MTU)] {
        let mut request = sent(
            &a.open_link(b.destination(), IFACE, &[signalled as u8 + 0x40; 64], 0)
                .unwrap(),
        )
        .unwrap();
        let trailer = request.payload.len() - link::TRAILER_LEN;
        request.payload[trailer..].copy_from_slice(
            &LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: signalled,
            }
            .encode(),
        );
        sent(&b.ingest(IFACE, &request, 0)).unwrap();
        let id = link::link_id(&request).unwrap();
        let link = b.links.iter().find(|(l, ..)| l.id() == id).unwrap();
        assert_eq!(link.0.mtu(), offered);
    }
}
