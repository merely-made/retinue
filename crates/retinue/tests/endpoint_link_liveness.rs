//! Link liveness on the endpoint: RTT, keepalives, stale teardown, the responder's handshake
//! deadline, the setup deadline, and RNS's 431-byte link MDU.
//!
//! The endpoints run on tokio's paused clock, so ten idle minutes cost nothing. The wire
//! between them is a tap that copies every packet and can be cut, which is how a peer
//! vanishes without a word.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, InterfaceSink, OutboundPackets, PayloadMode};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::link::{
    self, CTX_KEEPALIVE, CTX_LINKCLOSE, CTX_LRRTT, KEEPALIVE_REQUEST, KEEPALIVE_RESPONSE, LinkMode,
    LinkTrailer, PendingLink,
};
use retinue::link_liveness::handshake_timeout;
use retinue::packet::{DestinationType, Packet, PacketType};
use retinue::request::{Request, Response};

fn identity(seed: u8) -> PrivateIdentity {
    PrivateIdentity::from_secret_bytes(&[seed; 64])
}

/// Two endpoints back to back. Every packet each one sends is copied to its channel; while
/// `cut` is set nothing is delivered either way.
struct Wire {
    from_a: mpsc::UnboundedReceiver<Packet>,
    from_b: mpsc::UnboundedReceiver<Packet>,
    cut: Arc<AtomicBool>,
}

fn wire(a: &Endpoint, b: &Endpoint) -> Wire {
    let (a_out, into_a) = a.attach_interface().split();
    let (b_out, into_b) = b.attach_interface().split();
    let (a_copies, from_a) = mpsc::unbounded_channel();
    let (b_copies, from_b) = mpsc::unbounded_channel();
    let cut = Arc::new(AtomicBool::new(false));
    tokio::spawn(tap(a_out, into_b, a_copies, Arc::clone(&cut)));
    tokio::spawn(tap(b_out, into_a, b_copies, Arc::clone(&cut)));
    Wire {
        from_a,
        from_b,
        cut,
    }
}

async fn tap(
    mut out: OutboundPackets,
    sink: InterfaceSink,
    copies: mpsc::UnboundedSender<Packet>,
    cut: Arc<AtomicBool>,
) {
    while let Some(packet) = out.recv().await {
        if !cut.load(Ordering::Acquire) && !sink.deliver(packet.clone()) {
            break;
        }
        let _ = copies.send(packet);
    }
}

/// Every packet `out` sends, copied to a channel the test can drain without waiting.
fn collect(mut out: OutboundPackets) -> mpsc::UnboundedReceiver<Packet> {
    let (copies, packets) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(packet) = out.recv().await {
            let _ = copies.send(packet);
        }
    });
    packets
}

fn drain(copies: &mut mpsc::UnboundedReceiver<Packet>) -> Vec<Packet> {
    let mut packets = Vec::new();
    while let Ok(packet) = copies.try_recv() {
        packets.push(packet);
    }
    packets
}

fn keepalives(packets: &[Packet], sentinel: u8) -> usize {
    packets
        .iter()
        .filter(|p| p.context == CTX_KEEPALIVE && p.payload == [sentinel])
        .count()
}

/// The same request, with its data sized so that it packs to exactly `len` bytes.
fn request_packing_to(len: usize) -> Request {
    (0..len)
        .map(|n| Request::new(b"/mdu", vec![0x5a; n], 1_760_000_000.5))
        .find(|request| request.pack().len() == len)
        .expect("a data length packs to the target")
}

fn response_data_packing_to(len: usize) -> Vec<u8> {
    let id = AddressHash::from_bytes([1; 16]);
    (0..len)
        .map(|n| vec![0xa5; n])
        .find(|data| Response::pack_value(id, &Response::pack_binary_value(data)).len() == len)
        .expect("a data length packs to the target")
}

/// RNS's link MDU at MTU 500 is 431 bytes. A request and a response of exactly that size
/// each travel as one packet; the previous 367-byte ceiling refused the request and turned
/// the response into a Resource.
#[tokio::test]
async fn a_431_byte_request_and_response_each_fit_one_packet() {
    let server_id = identity(0x41);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(identity(0x42));
    let name = DestinationName::new("retinue", ["mdu"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    let mut w = wire(&client, &server);

    let request = request_packing_to(431);
    let reply = response_data_packing_to(431);
    let expected = reply.clone();
    let responder = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let received = accepted.session.receive_request().await.unwrap();
            assert_eq!(
                received.request.data.len(),
                request_packing_to(431).data.len()
            );
            accepted
                .session
                .respond_auto(received.request_id, reply)
                .await
                .unwrap()
        }
    });
    let mut session = tokio::time::timeout(
        Duration::from_secs(5),
        client.open_resource(destination, *server_id.public()),
    )
    .await
    .unwrap()
    .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), session.request(&request))
        .await
        .unwrap()
        .expect("a 431-byte request is within the link MDU");
    assert_eq!(response.data, expected);
    assert_eq!(responder.await.unwrap(), PayloadMode::Data);
    // One packet, leaving RNS's one-byte IFAC reserve.
    let one_packet =
        |p: &Packet, context| p.context == context && p.encoded_len() < retinue::packet::MTU;
    assert!(
        drain(&mut w.from_a)
            .iter()
            .any(|p| one_packet(p, link::CTX_REQUEST))
    );
    assert!(
        drain(&mut w.from_b)
            .iter()
            .any(|p| one_packet(p, link::CTX_RESPONSE))
    );
    // And one byte more goes as a Resource instead (`Link.py` 503-506).
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        session.request_raw(&request_packing_to(432).pack()),
    )
    .await;
    // The responder has closed the link, so the request may end before the tap has
    // copied the advertisement: give it a moment.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let sent = drain(&mut w.from_a);
    assert!(sent.iter().any(|p| p.context == link::CTX_RESOURCE_ADV));
    assert!(!sent.iter().any(|p| p.context == link::CTX_REQUEST));
}

/// An idle link stays up for as long as both peers are there: the initiator asks every
/// keepalive interval, the responder answers, and neither side goes stale. Before
/// keepalives, nothing crossed the wire and an RNS peer dropped the link within half a
/// minute.
#[tokio::test(start_paused = true)]
async fn an_idle_reliable_link_is_kept_alive_for_ten_minutes() {
    let server_id = identity(0x51);
    let server = Endpoint::new(server_id.clone());
    let client = Endpoint::new(identity(0x52));
    let name = DestinationName::new("retinue", ["idle"]);
    let destination = name.destination_hash(server_id.public());
    server.register_reliable(name, b"");
    let mut w = wire(&client, &server);

    let mut opened = client
        .open_reliable(destination, *server_id.public())
        .await
        .unwrap();
    let mut accepted = server.accept_reliable().await.unwrap();
    tokio::time::sleep(Duration::from_millis(1)).await;
    let rtt = drain(&mut w.from_a)
        .into_iter()
        .filter(|p| p.context == CTX_LRRTT)
        .count();
    assert_eq!(rtt, 1, "the initiator reports its RTT once");

    tokio::time::sleep(Duration::from_secs(600)).await;

    let requests = keepalives(&drain(&mut w.from_a), KEEPALIVE_REQUEST);
    let responses = keepalives(&drain(&mut w.from_b), KEEPALIVE_RESPONSE);
    // A sub-millisecond RTT gives the 5 s minimum interval, checked on a 0.5 s tick.
    assert!(
        (100..=120).contains(&requests),
        "{requests} keepalive requests"
    );
    assert!(
        responses * 10 >= requests * 9,
        "{responses} answers to {requests}"
    );
    assert_eq!(client.link_facts().len(), 1);
    assert_eq!(server.link_facts().len(), 1);

    opened.write_all(b"still here").await.unwrap();
    let mut buf = [0; 10];
    accepted.read_exact(&mut buf).await.unwrap();
    assert_eq!(&buf, b"still here");
}

/// A peer that vanishes is noticed: two silent keepalive intervals make the link stale,
/// and after the grace it is torn down with a LINKCLOSE and the stream's read fails with a
/// timeout rather than hanging, or ending as if the peer had finished.
#[tokio::test(start_paused = true)]
async fn a_vanished_peer_times_out_both_stream_kinds() {
    let server_id = identity(0x61);
    let server = Endpoint::new(server_id.clone());
    let client = Endpoint::new(identity(0x62));
    let best = DestinationName::new("retinue", ["vanish-best"]);
    let reliable = DestinationName::new("retinue", ["vanish-reliable"]);
    let best_dest = best.destination_hash(server_id.public());
    let reliable_dest = reliable.destination_hash(server_id.public());
    server.register(best, b"");
    server.register_reliable(reliable, b"");
    let mut w = wire(&client, &server);

    let mut plain = client.open(best_dest, *server_id.public()).await.unwrap();
    let mut stream = client
        .open_reliable(reliable_dest, *server_id.public())
        .await
        .unwrap();
    let _accepted = (
        server.accept().await.unwrap(),
        server.accept_reliable().await.unwrap(),
    );
    w.cut.store(true, Ordering::Release);
    let cut_at = tokio::time::Instant::now();

    let mut buf = Vec::new();
    let limit = Duration::from_secs(60);
    let error = tokio::time::timeout(limit, plain.read_to_end(&mut buf))
        .await
        .expect("the read ends")
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    let error = tokio::time::timeout(limit, stream.read_to_end(&mut buf))
        .await
        .expect("the read ends")
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    let waited = cut_at.elapsed();
    // Stale after two 5 s intervals, then rtt × 4 + 5 s of grace.
    assert!(
        waited >= Duration::from_secs(15) && waited < Duration::from_secs(17),
        "{waited:?}"
    );
    let closes = drain(&mut w.from_a)
        .into_iter()
        .filter(|p| p.context == CTX_LINKCLOSE)
        .count();
    assert_eq!(closes, 2, "each link is closed on the wire");
    assert!(client.link_facts().is_empty());
    // The first write may still be buffered; the relay then sees the loss and stops.
    let _ = plain.write_all(b"late").await;
    tokio::time::sleep(Duration::from_millis(1)).await;
    assert!(plain.write_all(b"later").await.is_err());
}

/// A link request whose initiator proves nothing after the proof (no RTT packet) is held
/// for RNS's handshake allowance, then dropped without a LINKCLOSE.
#[tokio::test(start_paused = true)]
async fn an_inbound_link_without_an_rtt_is_dropped_at_the_handshake_deadline() {
    let server_id = identity(0x71);
    let server = Endpoint::new(server_id.clone());
    let name = DestinationName::new("retinue", ["no-rtt"]);
    let destination = name.destination_hash(server_id.public());
    server.register(name, b"");
    let (out, into) = server.attach_interface().split();
    let mut out = collect(out);

    let (_pending, request) = PendingLink::open(
        destination,
        *server_id.public(),
        &[0x72; 64],
        LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        },
    );
    let started = tokio::time::Instant::now();
    assert!(into.deliver(request));
    let mut stream = server.accept().await.unwrap();
    tokio::task::yield_now().await;
    let proof = out.try_recv().unwrap();
    assert_eq!(proof.packet_type, PacketType::Proof);

    let mut buf = Vec::new();
    let error = tokio::time::timeout(Duration::from_secs(600), stream.read_to_end(&mut buf))
        .await
        .expect("the read ends")
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    let waited = started.elapsed();
    let deadline = Duration::from_millis(handshake_timeout(0));
    assert!(
        waited >= deadline && waited < deadline + Duration::from_secs(1),
        "{waited:?}"
    );
    assert!(server.link_facts().is_empty());
    while let Ok(packet) = out.try_recv() {
        assert_ne!(
            packet.context, CTX_LINKCLOSE,
            "a link that never activated is not closed"
        );
    }
}

/// Setup waits Node's deadline (12 s with no route) plus the first-hop airtime, not a
/// fixed 15 s. On timeout the endpoint asks for a path to the destination.
#[tokio::test(start_paused = true)]
async fn link_setup_uses_the_node_deadline_then_requests_a_path() {
    let client = Endpoint::new(identity(0x81));
    let interface = client.attach_interface();
    let id = interface.id();
    let (out, _into) = interface.split();
    let mut out = collect(out);
    let silent = identity(0x82);
    let destination = DestinationName::new("retinue", ["silent"]).destination_hash(silent.public());

    let started = tokio::time::Instant::now();
    let Err(error) = client.open(destination, *silent.public()).await else {
        panic!("nobody proves");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(started.elapsed(), Duration::from_secs(12));
    tokio::task::yield_now().await;
    let sent = drain(&mut out);
    assert_eq!(
        sent.last().unwrap().destination_type,
        DestinationType::Plain
    );
    assert_eq!(
        retinue::path::parse_request(sent.last().unwrap()),
        Some(destination),
        "the endpoint asks for a path after the failed setup",
    );

    // A slow first hop extends the deadline by its airtime. The path request is within its
    // 20 s budget, so it is not repeated.
    client.set_first_hop_airtime(id, Duration::from_secs(3));
    let started = tokio::time::Instant::now();
    let Err(error) = client.open(destination, *silent.public()).await else {
        panic!("nobody proves");
    };
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert_eq!(started.elapsed(), Duration::from_secs(15));
    tokio::task::yield_now().await;
    for packet in drain(&mut out) {
        assert_eq!(packet.packet_type, PacketType::LinkRequest);
    }
}
