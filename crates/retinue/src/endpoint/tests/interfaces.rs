//! Interface admission, link packet memory, and detaching.

use super::*;

/// The link packet memory holds its bound, forgets oldest first, and leaves alone the
/// contexts whose repeats are legitimate: every keepalive request shares one hash, and a
/// re-sent resource part repeats its own.
#[test]
fn link_packet_memory_is_bounded_and_skips_keepalives_and_resources() {
    let mut window = HashWindow::new(2);
    let [a, b, c] = [1u8, 2, 3].map(|n| AddressHash::from_bytes([n; 16]));
    assert!(window.insert(a) && window.insert(b) && !window.insert(a));
    assert!(window.insert(c));
    assert_eq!(window.order.len(), 2);
    assert!(!window.contains(&a), "the oldest is forgotten first");
    assert!(window.contains(&b) && window.contains(&c));

    let link_packet = |context| Packet {
        ifac: false,
        header_type: crate::packet::HeaderType::Type1,
        context_flag: false,
        propagation: crate::packet::Propagation::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: AddressHash::from_bytes([9; 16]),
        context,
        payload: vec![link::KEEPALIVE_REQUEST],
    };
    let mut memory = LinkPacketMemory::new();
    for context in [link::CTX_KEEPALIVE, link::CTX_RESOURCE] {
        let packet = link_packet(context);
        memory.note_sent(&packet);
        assert_eq!(memory.admit(&packet), LinkPacketAdmission::New);
        assert_eq!(memory.admit(&packet), LinkPacketAdmission::New);
    }
    let data = link_packet(0);
    assert_eq!(memory.admit(&data), LinkPacketAdmission::New);
    assert_eq!(memory.admit(&data), LinkPacketAdmission::Duplicate);
    let channel = link_packet(CTX_CHANNEL);
    memory.note_sent(&channel);
    assert_eq!(
        memory.admit(&channel),
        LinkPacketAdmission::OwnEcho,
        "our own Channel packet is ours"
    );
}

#[test]
fn ifac_overhead_counts_against_interface_frame_admission() {
    let queues = Arc::new(OutboundQueues::new(
        QueueWeights::DEFAULT,
        QueueDepths::DEFAULT,
    ));
    let packet = Packet {
        ifac: false,
        header_type: crate::packet::HeaderType::Type1,
        context_flag: false,
        propagation: crate::packet::Propagation::Broadcast,
        destination_type: DestinationType::Plain,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: AddressHash::from_bytes([0x51; 16]),
        context: 0,
        payload: b"frame admission".to_vec(),
    };
    let actual = packet.encoded_len() + 8;
    let interface = Iface {
        id: 1,
        outbound: queues,
        frame_limit: Arc::new(AtomicUsize::new(actual - 1)),
        wire_overhead: 8,
        mode: InterfaceMode::Full,
    };

    assert_eq!(
        interface.push(packet, TrafficClass::Interactive),
        QueueAdmission::FrameLimit {
            actual,
            limit: actual - 1,
        }
    );
}

#[tokio::test]
async fn an_inbound_link_fact_keeps_unknown_remote_unknown() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x15; 64]));
    let interface = ep.attach_interface().id();
    let destination =
        DestinationName::new("retinue", ["management-link"]).destination_hash(ep.identity());
    let (_, request) = link::PendingLink::open(
        destination,
        *ep.identity(),
        &[0x17; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: DEFAULT_LINK_MTU,
        },
    );
    let (link, _) = link::accept(
        &request,
        &ep.shared.identity,
        &[0x18; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: DEFAULT_LINK_MTU,
        },
    )
    .unwrap();
    let _stream = register_stream(
        &ep.shared,
        link,
        interface,
        Liveness::responder(0, 0),
        LinkDirection::Inbound,
        LinkRemoteFact::default(),
    )
    .unwrap();

    let facts = ep.link_facts();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].direction, LinkDirection::Inbound);
    assert_eq!(facts[0].remote, LinkRemoteFact::default());
    assert_eq!(facts[0].interface, interface);
}

/// A peer that connects and drops repeatedly -- a flapping link, a daemon being
/// restarted -- used to leave its interface record and queues behind on every cycle.
#[tokio::test]
async fn a_dropped_tcp_peer_leaves_no_interface_behind() {
    let server = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x36; 64]));
    let addr = server
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .expect("listener");
    let settled = server.shared.interfaces.lock().unwrap().len();

    for _ in 0..6 {
        let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x37; 64]));
        client.attach_tcp_client(addr).await.expect("connect");
        // Dropping the client closes its socket, which the server's reader observes.
        drop(client);
        tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    }

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        server.shared.interfaces.lock().unwrap().len(),
        settled,
        "six connect/drop cycles must leave nothing accumulated",
    );
}

/// Attaching was one-way, so a peer that reconnects repeatedly grew the interface list
/// and its queues without bound, and the scheduler kept visiting records for carriers
/// that were long gone.
#[tokio::test]
async fn detaching_an_interface_forgets_it() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x33; 64]));
    let before = ep.shared.interfaces.lock().unwrap().len();

    let iface = ep.attach_interface();
    let id = iface.id;
    assert_eq!(ep.shared.interfaces.lock().unwrap().len(), before + 1);

    ep.detach_interface(id);
    assert_eq!(
        ep.shared.interfaces.lock().unwrap().len(),
        before,
        "a detached interface leaves no record behind",
    );

    // And reconnecting the same carrier does not stack up.
    for _ in 0..8 {
        let again = ep.attach_interface();
        ep.detach_interface(again.id);
    }
    assert_eq!(
        ep.shared.interfaces.lock().unwrap().len(),
        before,
        "eight reconnects leave nothing accumulated",
    );
}
