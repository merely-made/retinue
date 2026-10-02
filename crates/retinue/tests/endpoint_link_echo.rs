//! An endpoint's own link packets heard back, and a far-end packet heard twice.
//!
//! On a shared medium the first relay's retransmission of our own link packet reaches us
//! too. It carries hops+1 and the same packet hash, and it decrypts under the link's shared
//! key, so nothing in its contents marks it as ours. The same medium also hands us the far
//! end's packet twice: once directly, once from the relay.
//!
//! These drive two endpoints over raw interfaces with a tap on each direction, so a test can
//! take any packet a side put on the wire and play it back as the relay would: a clone with
//! hops+1. That is the shape lane V1 injected against RNS 1.5.4
//! (`testing/receipts/rns-1.5.4-link-echo-corroboration/`, cases C2-C4).

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, InterfaceSink, LinkStream, OutboundPackets};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::link::CTX_CHANNEL;
use retinue::packet::{Packet, PacketType};

const WAIT: Duration = Duration::from_secs(5);
/// Plain link data.
const CTX_DATA: u8 = 0x00;

/// Two endpoints wired back to back, with a copy of everything each one transmits.
struct Wire {
    /// Delivers into A, as if heard on A's interface.
    into_a: InterfaceSink,
    /// Delivers into B.
    into_b: InterfaceSink,
    /// Every packet A transmitted, after it was delivered to B.
    from_a: mpsc::UnboundedReceiver<Packet>,
    /// Every packet B transmitted, after it was delivered to A.
    from_b: mpsc::UnboundedReceiver<Packet>,
}

fn wire(a: &Endpoint, b: &Endpoint) -> Wire {
    let (a_out, into_a) = a.attach_interface().split();
    let (b_out, into_b) = b.attach_interface().split();
    let (a_copies, from_a) = mpsc::unbounded_channel();
    let (b_copies, from_b) = mpsc::unbounded_channel();
    tokio::spawn(tap(a_out, into_b.clone(), a_copies));
    tokio::spawn(tap(b_out, into_a.clone(), b_copies));
    Wire {
        into_a,
        into_b,
        from_a,
        from_b,
    }
}

/// Deliver first, then copy, so anything a test injects lands behind the original.
async fn tap(mut out: OutboundPackets, sink: InterfaceSink, copies: mpsc::UnboundedSender<Packet>) {
    while let Some(packet) = out.recv().await {
        if !sink.deliver(packet.clone()) {
            break;
        }
        let _ = copies.send(packet);
    }
}

/// The next link data packet on `link` with `context`, skipping everything else.
async fn next_on_link(
    copies: &mut mpsc::UnboundedReceiver<Packet>,
    link: AddressHash,
    context: u8,
) -> Packet {
    tokio::time::timeout(WAIT, async {
        loop {
            let packet = copies.recv().await.expect("the tap stays open");
            if packet.packet_type == PacketType::Data
                && packet.destination == link
                && packet.context == context
            {
                return packet;
            }
        }
    })
    .await
    .expect("the packet reaches the wire")
}

/// The relay's retransmission: the same packet one hop further on.
fn relayed(packet: &Packet) -> Packet {
    let mut copy = packet.clone();
    copy.hops += 1;
    assert_eq!(
        copy.hash(),
        packet.hash(),
        "hops are outside the packet hash"
    );
    copy
}

async fn read_n(stream: &mut LinkStream, n: usize) -> String {
    let mut buf = vec![0; n];
    tokio::time::timeout(WAIT, stream.read_exact(&mut buf))
        .await
        .expect("bytes arrive")
        .expect("stream stays open");
    String::from_utf8_lossy(&buf).into_owned()
}

/// `(own_echo_dropped, duplicate_dropped)`. Read after a later packet has reached the
/// application: one router task handles an interface's packets in order, so every packet
/// injected before it has been judged.
fn drops(endpoint: &Endpoint) -> (u64, u64) {
    let counters = endpoint.routing_counters();
    (counters.own_echo_dropped, counters.duplicate_dropped)
}

async fn write(stream: &mut LinkStream, bytes: &[u8]) {
    stream.write_all(bytes).await.unwrap();
    stream.flush().await.unwrap();
}

fn identity(seed: u8) -> PrivateIdentity {
    PrivateIdentity::from_secret_bytes(&[seed; 64])
}

/// A best-effort link from A to B: `(a, b, wire, a_stream, b_stream)`.
async fn best_effort() -> (Endpoint, Endpoint, Wire, LinkStream, LinkStream) {
    let a = Endpoint::new(identity(0x11));
    let b_id = identity(0x22);
    let b = Endpoint::new(b_id.clone());
    let name = DestinationName::new("retinue", ["echo"]);
    let dest = name.destination_hash(b_id.public());
    b.register(name, b"");
    let w = wire(&a, &b);
    let a_stream = tokio::time::timeout(WAIT, a.open(dest, *b_id.public()))
        .await
        .expect("link opens")
        .unwrap();
    let b_stream = tokio::time::timeout(WAIT, b.accept())
        .await
        .expect("link accepted")
        .unwrap();
    (a, b, w, a_stream, b_stream)
}

/// A reliable (Channel) link from A to B: `(a, b, wire, a_stream, b_stream)`.
async fn reliable() -> (Endpoint, Endpoint, Wire, LinkStream, LinkStream) {
    let a = Endpoint::new(identity(0x11));
    let b_id = identity(0x22);
    let b = Endpoint::new(b_id.clone());
    let name = DestinationName::new("retinue", ["echo"]);
    let dest = name.destination_hash(b_id.public());
    b.register_reliable(name, b"");
    let w = wire(&a, &b);
    let a_stream = tokio::time::timeout(WAIT, a.open_reliable(dest, *b_id.public()))
        .await
        .expect("link opens")
        .unwrap();
    let b_stream = tokio::time::timeout(WAIT, b.accept_reliable())
        .await
        .expect("link accepted")
        .unwrap();
    (a, b, w, a_stream, b_stream)
}

/// Ruling 51 (a): A's own link data, played back by a relay, is not A's received data, and
/// the far end's data after it still arrives.
#[tokio::test]
async fn own_link_data_echoed_by_a_relay_is_not_received() {
    let (a, b, mut w, mut a_stream, mut b_stream) = best_effort().await;
    let link = a_stream.link_id();

    write(&mut a_stream, b"from A").await;
    let own = next_on_link(&mut w.from_a, link, CTX_DATA).await;
    assert!(w.into_a.deliver(relayed(&own)));
    assert_eq!(read_n(&mut b_stream, 6).await, "from A", "B hears A once");

    write(&mut b_stream, b"from B").await;
    assert_eq!(
        read_n(&mut a_stream, 6).await,
        "from B",
        "A must not surface its own payload as received data"
    );
    assert_eq!(drops(&a), (1, 0), "the echo counts once, as A's own");
    assert_eq!(drops(&b), (0, 0), "B's delivered packet is not counted");

    // The memory matches packets, not payloads: the far end sending the bytes A sent, under
    // its own IV, is a different packet and is delivered.
    write(&mut b_stream, b"from A").await;
    assert_eq!(read_n(&mut a_stream, 6).await, "from A");
    assert_eq!(drops(&a), (1, 0), "a delivered packet is not counted");

    // The responder is covered the same way.
    let reply = next_on_link(&mut w.from_b, link, CTX_DATA).await;
    assert!(w.into_b.deliver(relayed(&reply)));
    write(&mut a_stream, b"next A").await;
    assert_eq!(read_n(&mut b_stream, 6).await, "next A");
    assert_eq!(drops(&b), (1, 0));
    assert_eq!(drops(&a), (1, 0));
}

/// Ruling 51 (b): A's own Channel message, played back by a relay, is not delivered to A,
/// and the far end's Channel sequence 0, which arrives after the echo, still is.
#[tokio::test]
async fn own_channel_message_echoed_by_a_relay_is_not_received() {
    let (a, b, mut w, mut a_stream, mut b_stream) = reliable().await;
    let link = a_stream.link_id();

    write(&mut a_stream, b"from A").await;
    let own = next_on_link(&mut w.from_a, link, CTX_CHANNEL).await;
    assert!(w.into_a.deliver(relayed(&own)));
    assert_eq!(read_n(&mut b_stream, 6).await, "from A", "B hears A once");

    write(&mut b_stream, b"from B").await;
    assert_eq!(
        read_n(&mut a_stream, 6).await,
        "from B",
        "A must not take its own sequence 0 for the far end's, nor lose the far end's"
    );
    assert_eq!(drops(&a), (1, 0), "the echo counts once, as A's own");

    // A late copy, after B has proved the original, is still A's own. Both directions then
    // carry on at sequence 1.
    assert!(w.into_a.deliver(relayed(&own)));
    write(&mut a_stream, b"next A").await;
    assert_eq!(read_n(&mut b_stream, 6).await, "next A");
    write(&mut b_stream, b"next B").await;
    assert_eq!(read_n(&mut a_stream, 6).await, "next B");
    assert_eq!(drops(&a), (2, 0), "the late copy counts once more");
    // B's duplicate count is A's IDENTIFY re-sends, which depend on timing.
    assert_eq!(drops(&b).0, 0, "B's delivered packets are not counted");
}

/// Ruling 52: the far end's link data heard twice more, verbatim and via a relay, reaches
/// the application once.
#[tokio::test]
async fn far_end_link_data_heard_twice_is_received_once() {
    let (a, b, mut w, mut a_stream, mut b_stream) = best_effort().await;
    let link = a_stream.link_id();

    write(&mut b_stream, b"from B").await;
    let theirs = next_on_link(&mut w.from_b, link, CTX_DATA).await;
    assert!(w.into_a.deliver(theirs.clone()));
    assert!(w.into_a.deliver(relayed(&theirs)));
    write(&mut b_stream, b"second").await;

    assert_eq!(
        read_n(&mut a_stream, 12).await,
        "from Bsecond",
        "each far-end packet reaches the application once"
    );
    assert_eq!(drops(&a), (0, 2), "each copy counts once, as a duplicate");
    assert_eq!(drops(&b), (0, 0), "A's delivered packets are not counted");
}

/// Ruling 52, Channel: the far end's Channel message heard twice more, verbatim and via a
/// relay, reaches the application once.
#[tokio::test]
async fn far_end_channel_message_heard_twice_is_received_once() {
    let (a, b, mut w, mut a_stream, mut b_stream) = reliable().await;
    let link = a_stream.link_id();

    write(&mut b_stream, b"from B").await;
    let theirs = next_on_link(&mut w.from_b, link, CTX_CHANNEL).await;
    assert!(w.into_a.deliver(theirs.clone()));
    assert!(w.into_a.deliver(relayed(&theirs)));
    write(&mut b_stream, b"second").await;

    assert_eq!(
        read_n(&mut a_stream, 12).await,
        "from Bsecond",
        "each far-end Channel message reaches the application once"
    );
    assert_eq!(drops(&a), (0, 2), "each copy counts once, as a duplicate");
    // B's duplicate count is A's IDENTIFY re-sends, which depend on timing; see the
    // reliable link's IDENTIFY_MAX_SENDS. Its own-echo count is exact.
    assert_eq!(drops(&b).0, 0, "A's delivered packets are not counted");
}
