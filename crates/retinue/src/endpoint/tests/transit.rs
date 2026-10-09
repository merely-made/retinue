//! Link bridges, the reverse table, and link MTU clamping.

use super::*;

/// The reverse table carries a proof once, only from the interface its packet left by,
/// within `REVERSE_TIMEOUT`, and holds at most `REVERSE_TABLE_CAPACITY` return paths.
#[tokio::test]
async fn reverse_table_is_consumed_on_use_expires_and_is_bounded() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xB1; 64]));
    let shared = &ep.shared;
    let packet = AddressHash::from_bytes([0xB2; 16]);

    shared.remember_reverse(packet, 1, 2);
    assert_eq!(shared.take_reverse(packet, 3), None, "wrong interface");
    assert_eq!(shared.take_reverse(packet, 2), None, "consumed by the miss");

    shared.remember_reverse(packet, 1, 2);
    assert_eq!(shared.take_reverse(packet, 2), Some(1));
    assert_eq!(shared.take_reverse(packet, 2), None, "consumed by the use");

    if let Some(lapsed) = Instant::now().checked_sub(REVERSE_TIMEOUT) {
        shared.remember_reverse(packet, 1, 2);
        shared
            .reverse_table
            .lock()
            .unwrap()
            .get_mut(&packet)
            .unwrap()
            .at = lapsed;
        assert_eq!(shared.take_reverse(packet, 2), None, "expired");
    }

    for byte in 0..=REVERSE_TABLE_CAPACITY as u8 {
        shared.remember_reverse(AddressHash::from_bytes([byte; 16]), 1, 2);
    }
    assert_eq!(
        shared.reverse_table.lock().unwrap().len(),
        REVERSE_TABLE_CAPACITY
    );
}

#[tokio::test]
async fn a_link_bridge_accepts_only_its_two_interfaces() {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x93; 64]));
    let a = endpoint.attach_interface();
    let b = endpoint.attach_interface();
    let c = endpoint.attach_interface();
    endpoint.enable_routing();

    let link_id = AddressHash::from_bytes([0xA3; 16]);
    endpoint.shared.link_transport.lock().unwrap().insert(
        link_id,
        LinkBridge {
            from: a.id(),
            out: b.id(),
            destination: AddressHash::from_bytes([0xA4; 16]),
            seen: Instant::now(),
            proof_deadline: None,
        },
    );
    let packet = Packet {
        ifac: false,
        header_type: crate::packet::HeaderType::Type1,
        context_flag: false,
        propagation: crate::packet::Propagation::Transport,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: link_id,
        context: 0,
        payload: b"bridged data".to_vec(),
    };

    route(&endpoint.shared, a.id(), packet.clone());
    let to_b = b
        .outbound
        .queues
        .pop()
        .expect("first bridge end forwards to second");
    assert_eq!(to_b.destination, link_id);
    assert_eq!(to_b.hops, 1);
    b.outbound.queues.delivery_complete();

    route(&endpoint.shared, b.id(), packet.clone());
    let to_a = a
        .outbound
        .queues
        .pop()
        .expect("second bridge end forwards to first");
    assert_eq!(to_a.destination, link_id);
    assert_eq!(to_a.hops, 1);
    a.outbound.queues.delivery_complete();

    let seen_before_foreign = endpoint.shared.link_transport.lock().unwrap()[&link_id].seen;
    route(&endpoint.shared, c.id(), packet);
    assert!(a.outbound.queues.pop().is_none());
    assert!(b.outbound.queues.pop().is_none());
    assert!(c.outbound.queues.pop().is_none());
    assert_eq!(
        endpoint.shared.link_transport.lock().unwrap()[&link_id].seen,
        seen_before_foreign
    );
    let counters = endpoint.routing_counters();
    assert_eq!(counters.forwarded_packets, 2);
    assert_eq!(counters.policy_rejected, 1);
}

/// A routing endpoint with a radio side `a` and an IFAC'd 255-byte side `b`, a learned
/// destination behind `b`, and that destination's link request, addressed through the
/// endpoint and asking for 500 bytes.
fn transit_fixture() -> (
    Endpoint,
    Interface,
    Interface,
    PrivateIdentity,
    link::PendingLink,
    Packet,
) {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x94; 64]));
    let a = endpoint.attach_interface();
    let ifac = Ifac::with_default_size(Some("transit"), None).unwrap();
    let b = endpoint.attach_interface_with_ifac(255, ifac).unwrap();
    endpoint.enable_routing();

    let responder = PrivateIdentity::from_secret_bytes(&[0x95; 64]);
    let (_, announce) = freshness_announce(&responder, "transit", 0, 1, 1, 0);
    endpoint
        .shared
        .address_book
        .lock()
        .unwrap()
        .ingest(&announce);
    endpoint.shared.path_table.lock().unwrap().insert(
        announce.destination,
        PathEntry {
            iface: b.id(),
            transport: None,
            hops: 0,
            learned: Instant::now(),
            mode: InterfaceMode::default(),
        },
    );
    let (pending, mut request) = link::PendingLink::open(
        announce.destination,
        *responder.public(),
        &[0x96; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    request.header_type = crate::packet::HeaderType::Type2;
    request.transport = Some(endpoint.identity().hash());
    (endpoint, a, b, responder, pending, request)
}

fn link_data(link_id: AddressHash) -> Packet {
    Packet {
        ifac: false,
        header_type: crate::packet::HeaderType::Type1,
        context_flag: false,
        propagation: crate::packet::Propagation::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: link_id,
        context: 0,
        payload: b"link data".to_vec(),
    }
}

/// A carried request is clamped to the smaller side less its access code (255 - 8), and
/// keeps its link id. Its bridge waits for the proof, with the per-hop deadline.
#[tokio::test]
async fn a_carried_link_request_is_clamped_to_both_sides() {
    let (endpoint, a, b, _, pending, request) = transit_fixture();
    route(&endpoint.shared, a.id(), request);
    let carried = b.outbound.queues.pop().expect("the request is carried");
    assert_eq!(link::link_id(&carried).unwrap(), pending.link_id());
    assert_eq!(
        link::request_trailer(&carried).unwrap().map(|t| t.mtu),
        Some(247)
    );
    let bridge = endpoint.shared.link_transport.lock().unwrap()[&pending.link_id()];
    let allowance = bridge.proof_deadline.unwrap() - bridge.seen;
    assert_eq!(
        allowance,
        Duration::from_millis(crate::node::LINK_ESTABLISHMENT_TIMEOUT_PER_HOP)
    );
}

/// A request whose MTU must be lowered under a link mode this node cannot encode is
/// dropped and takes no slot; one that already fits is carried as it is.
#[tokio::test]
async fn a_carried_request_that_cannot_be_clamped_is_dropped() {
    let (endpoint, a, b, _, pending, mut request) = transit_fixture();
    request.payload[link::LINK_KEYS_LEN] = 0xe0;
    route(&endpoint.shared, a.id(), request.clone());
    assert!(b.outbound.queues.pop().is_none());
    assert!(endpoint.shared.link_transport.lock().unwrap().is_empty());

    let trailer = request.payload.len() - link::TRAILER_LEN;
    request.payload[trailer..].copy_from_slice(&[0xe0, 0, 200]);
    route(&endpoint.shared, a.id(), request);
    let carried = b
        .outbound
        .queues
        .pop()
        .expect("a fitting request is carried");
    assert_eq!(carried.payload[trailer..], [0xe0, 0, 200]);
    assert!(
        endpoint
            .shared
            .link_transport
            .lock()
            .unwrap()
            .contains_key(&pending.link_id())
    );
}

/// With the destination's identity unknown, the proof cannot be checked; it is carried
/// only from the destination's side.
#[tokio::test]
async fn without_the_destinations_identity_only_its_side_proves() {
    let (endpoint, a, b, responder, pending, request) = transit_fixture();
    endpoint
        .shared
        .address_book
        .lock()
        .unwrap()
        .forget(request.destination);
    route(&endpoint.shared, a.id(), request);
    let carried = b.outbound.queues.pop().unwrap();
    b.outbound.queues.delivery_complete();
    let trailer = link::request_trailer(&carried).unwrap().unwrap();
    let (_, proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();

    route(&endpoint.shared, a.id(), proof.clone());
    assert!(a.outbound.queues.pop().is_none() && b.outbound.queues.pop().is_none());
    route(&endpoint.shared, b.id(), proof);
    assert!(a.outbound.queues.pop().is_some());
    let bridge = endpoint.shared.link_transport.lock().unwrap()[&pending.link_id()];
    assert!(bridge.proof_deadline.is_none());
}

/// A proof this node does not send on, here for the hop ceiling, leaves its bridge
/// unvalidated; RNS marks a bridge validated only as it transmits the proof.
#[tokio::test]
async fn a_proof_validates_its_bridge_only_once_carried() {
    let (endpoint, a, b, responder, pending, request) = transit_fixture();
    route(&endpoint.shared, a.id(), request);
    let carried = b.outbound.queues.pop().unwrap();
    b.outbound.queues.delivery_complete();
    let trailer = link::request_trailer(&carried).unwrap().unwrap();
    let (_, mut proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();
    proof.hops = MAX_HOPS;
    route(&endpoint.shared, b.id(), proof);
    assert!(a.outbound.queues.pop().is_none());
    assert_eq!(endpoint.routing_counters().hop_limit_dropped, 1);
    let bridge = endpoint.shared.link_transport.lock().unwrap()[&pending.link_id()];
    assert!(bridge.proof_deadline.is_some());
}

/// Nothing crosses a carried link before its destination proves it: early data, a proof
/// signed by another identity, and the real proof heard from the initiator's side are all
/// dropped. The real proof from the destination's side validates the bridge.
#[tokio::test]
async fn a_carried_link_waits_for_its_destinations_proof() {
    let (endpoint, a, b, responder, pending, request) = transit_fixture();
    let link_id = pending.link_id();
    route(&endpoint.shared, a.id(), request);
    let carried = b.outbound.queues.pop().unwrap();
    b.outbound.queues.delivery_complete();

    route(&endpoint.shared, a.id(), link_data(link_id));
    let trailer = link::request_trailer(&carried).unwrap().unwrap();
    let impostor = PrivateIdentity::from_secret_bytes(&[0x97; 64]);
    let (_, forged) = link::accept(&carried, &impostor, &[0x98; 64], trailer).unwrap();
    route(&endpoint.shared, b.id(), forged);
    let (_, proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();
    route(&endpoint.shared, a.id(), proof.clone());
    assert!(a.outbound.queues.pop().is_none());
    assert!(b.outbound.queues.pop().is_none());
    assert_eq!(endpoint.routing_counters().policy_rejected, 3);

    route(&endpoint.shared, b.id(), proof);
    let proved = a.outbound.queues.pop().expect("the proof is carried back");
    a.outbound.queues.delivery_complete();
    let link = pending.prove(&proved).expect("the initiator accepts it");
    assert_eq!(link.mtu(), 247);

    route(&endpoint.shared, a.id(), link_data(link_id));
    assert!(
        b.outbound.queues.pop().is_some(),
        "validated links carry data"
    );
}

/// At capacity a request displaces the stalest unproved link; with every slot validated,
/// it is refused and not carried.
#[tokio::test]
async fn a_full_transit_table_never_evicts_a_validated_link() {
    let (endpoint, a, b, responder, pending, request) = transit_fixture();
    let now = Instant::now();
    let validated = |byte| {
        (
            AddressHash::from_bytes([byte; 16]),
            LinkBridge {
                from: a.id(),
                out: b.id(),
                destination: AddressHash::from_bytes([0; 16]),
                seen: now,
                proof_deadline: None,
            },
        )
    };
    endpoint
        .shared
        .link_transport
        .lock()
        .unwrap()
        .extend((1..=LINK_TRANSPORT_CAPACITY as u8).map(validated));

    route(&endpoint.shared, a.id(), request.clone());
    assert!(b.outbound.queues.pop().is_none());
    assert_eq!(endpoint.routing_counters().policy_rejected, 1);
    assert!(
        !endpoint
            .shared
            .link_transport
            .lock()
            .unwrap()
            .contains_key(&pending.link_id())
    );

    let unproved = AddressHash::from_bytes([1; 16]);
    endpoint
        .shared
        .link_transport
        .lock()
        .unwrap()
        .get_mut(&unproved)
        .unwrap()
        .proof_deadline = Some(now + LINK_TRANSPORT_TTL);
    // A fresh request: a byte-identical retransmission of the refused one is now a
    // duplicate, filtered before transit as RNS's packet hashlist filters it.
    let (fresh, mut fresh_request) = link::PendingLink::open(
        request.destination,
        *responder.public(),
        &[0x97; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    fresh_request.header_type = crate::packet::HeaderType::Type2;
    fresh_request.transport = request.transport;
    route(&endpoint.shared, a.id(), fresh_request);
    assert!(b.outbound.queues.pop().is_some());
    let bridges = endpoint.shared.link_transport.lock().unwrap();
    assert_eq!(bridges.len(), LINK_TRANSPORT_CAPACITY);
    assert!(bridges.contains_key(&fresh.link_id()));
    assert!(!bridges.contains_key(&unproved));
}

/// With every slot validated, a link unheard for the RNS link timeout yields to a new
/// request, so finished links cannot lock a transport node out of carrying more.
#[tokio::test]
async fn an_idle_validated_link_yields_to_a_new_request() {
    let (endpoint, a, b, _, pending, request) = transit_fixture();
    let now = Instant::now();
    let Some(idle_since) = now.checked_sub(LINK_TRANSPORT_IDLE) else {
        return; // A monotonic clock this young cannot express an idle link.
    };
    let validated = |byte: u8, seen| {
        (
            AddressHash::from_bytes([byte; 16]),
            LinkBridge {
                from: a.id(),
                out: b.id(),
                destination: AddressHash::from_bytes([0; 16]),
                seen,
                proof_deadline: None,
            },
        )
    };
    endpoint.shared.link_transport.lock().unwrap().extend(
        (1..=LINK_TRANSPORT_CAPACITY as u8)
            .map(|byte| validated(byte, if byte == 1 { idle_since } else { now })),
    );
    route(&endpoint.shared, a.id(), request);
    assert!(b.outbound.queues.pop().is_some());
    let bridges = endpoint.shared.link_transport.lock().unwrap();
    assert_eq!(bridges.len(), LINK_TRANSPORT_CAPACITY);
    assert!(bridges.contains_key(&pending.link_id()));
    assert!(!bridges.contains_key(&AddressHash::from_bytes([1; 16])));
}

/// A destination answers with no more than the interface that heard the request carries.
#[tokio::test]
async fn a_responder_clamps_the_link_to_its_receiving_interface() {
    let identity = PrivateIdentity::from_secret_bytes(&[0x9A; 64]);
    let endpoint = Endpoint::new(identity.clone());
    let name = DestinationName::new("retinue", ["clamped"]);
    endpoint.register(name.clone(), b"");
    let narrow = endpoint.attach_interface_with_frame_limit(300).unwrap();
    let (pending, request) = link::PendingLink::open(
        name.destination_hash(identity.public()),
        *identity.public(),
        &[0x9B; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    route(&endpoint.shared, narrow.id(), request);
    let proof = narrow.outbound.queues.pop().expect("the request is proved");
    assert_eq!(pending.prove(&proof).unwrap().mtu(), 300);
}

/// A signalled MTU of 0 means RNS's default, and a request below the smallest workable
/// link is held to that floor.
#[tokio::test]
async fn a_responder_floors_what_it_offers() {
    let identity = PrivateIdentity::from_secret_bytes(&[0x9C; 64]);
    let endpoint = Endpoint::new(identity.clone());
    let name = DestinationName::new("retinue", ["floored"]);
    endpoint.register(name.clone(), b"");
    let iface = endpoint.attach_interface();
    for (seed, signalled, offered) in [
        (0x9D, 0, endpoint.shared.link_mtu.load(Ordering::Relaxed)),
        (0x9E, 1, crate::node::MIN_LOGICAL_MTU),
    ] {
        let (_, mut request) = link::PendingLink::open(
            name.destination_hash(identity.public()),
            *identity.public(),
            &[seed; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        let trailer = request.payload.len() - link::TRAILER_LEN;
        request.payload[trailer..].copy_from_slice(
            &LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: signalled,
            }
            .encode(),
        );
        route(&endpoint.shared, iface.id(), request);
        let proof = iface.outbound.queues.pop().expect("the request is proved");
        iface.outbound.queues.delivery_complete();
        let echoed = &proof.payload[proof.payload.len() - link::TRAILER_LEN..];
        let echoed = LinkTrailer::decode(echoed.try_into().unwrap()).unwrap();
        assert_eq!(echoed.mtu, offered);
    }
}
