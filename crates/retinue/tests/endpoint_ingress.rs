//! Ingress preservation: an accepted session reports the interface it arrived on.
//!
//! V3 of the 2026-07-24 low-power radio and managed-network plan. A policy layer
//! above Retinue can only make honest decisions if ingress is a *transport fact*
//! carried out of accept, not something inferred later. These tests pin that the
//! interface survives to every accepted form, and that concurrent accepts on
//! different interfaces do not exchange it.

use std::time::Duration;

use retinue::announce_admission::AnnounceIngressPolicy;
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;

async fn signed_announce(
    seed: u8,
    aspect: &'static str,
) -> (retinue::hash::AddressHash, retinue::Packet) {
    let identity = PrivateIdentity::from_secret_bytes(&[seed; 64]);
    let sender = Endpoint::new(identity.clone());
    let mut wire = sender.attach_interface();
    let name = DestinationName::new("flood", [aspect]);
    let destination = name.destination_hash(identity.public());
    sender.announce(&name, b"ingress receipt");
    let packet = tokio::time::timeout(Duration::from_secs(1), wire.next_outbound())
        .await
        .expect("sender queues an announce")
        .expect("sender remains live");
    (destination, packet)
}

/// Wait until `ep` can resolve `dest`, pumping announcements.
async fn await_resolve(ep: &Endpoint, dest: retinue::hash::AddressHash) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ep.resolve(dest).is_none() && tokio::time::Instant::now() < deadline {
        let _ = tokio::time::timeout(Duration::from_millis(300), ep.next_announcement()).await;
    }
    assert!(ep.resolve(dest).is_some(), "peer should learn the dest");
}

/// Two leaves on two interfaces of one hub: each accepted link reports the
/// interface it actually arrived on, the two differ, and the wrapper agrees with
/// the stream.
#[tokio::test]
async fn accepted_links_report_their_own_interface() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[9u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let hub_name = DestinationName::new("hub", ["svc"]);
    let hub_dest = hub_name.destination_hash(hub_id.public());
    hub.register(hub_name.clone(), b"svc");

    // Two leaves, each its own TCP connection => its own hub-side interface.
    let a = Endpoint::new(PrivateIdentity::from_secret_bytes(&[2u8; 64]));
    a.attach_tcp_client(addr).await.unwrap();
    let b = Endpoint::new(PrivateIdentity::from_secret_bytes(&[3u8; 64]));
    b.attach_tcp_client(addr).await.unwrap();

    for _ in 0..4 {
        hub.announce(&hub_name, b"svc");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&a, hub_dest).await;
    await_resolve(&b, hub_dest).await;

    let hub_identity = *hub_id.public();
    let _a_stream = a.open(hub_dest, hub_identity).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("first accept should not time out")
        .unwrap();

    let _b_stream = b.open(hub_dest, hub_identity).await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("second accept should not time out")
        .unwrap();

    // The wrapper's ingress and the stream's ingress are the same fact.
    assert_eq!(
        first.interface,
        first.stream.interface(),
        "Accepted.interface must agree with LinkStream::interface"
    );
    assert_eq!(second.interface, second.stream.interface());

    // Two leaves arrived on two interfaces: ingress is not crossed or collapsed.
    assert_ne!(
        first.interface, second.interface,
        "links from different interfaces must report different ingress"
    );

    // Both targeted the same destination, so destination alone cannot
    // distinguish them: ingress is the added fact.
    assert_eq!(first.destination, hub_dest);
    assert_eq!(second.destination, hub_dest);
}

/// Two links from the *same* leaf report the same interface: ingress identifies
/// the bearer, not the session.
#[tokio::test]
async fn links_from_one_peer_share_an_interface() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[11u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let hub_name = DestinationName::new("hub", ["twice"]);
    let hub_dest = hub_name.destination_hash(hub_id.public());
    hub.register(hub_name.clone(), b"twice");

    let leaf = Endpoint::new(PrivateIdentity::from_secret_bytes(&[12u8; 64]));
    leaf.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&hub_name, b"twice");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&leaf, hub_dest).await;

    let hub_identity = *hub_id.public();
    let _one = leaf.open(hub_dest, hub_identity).await.unwrap();
    let first = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("first accept")
        .unwrap();
    let _two = leaf.open(hub_dest, hub_identity).await.unwrap();
    let second = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("second accept")
        .unwrap();

    assert_eq!(
        first.interface, second.interface,
        "two links over one bearer share ingress"
    );
}

/// The reliable accept path surfaces a bare stream rather than a wrapper, so it
/// is the path most likely to lose ingress. One leaf over one TCP connection
/// opens both a best-effort and a reliable link: both must report the same
/// interface, which is what "the paths do not diverge" means concretely.
#[tokio::test]
async fn reliable_accept_preserves_the_same_ingress_as_best_effort() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[21u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    // Two destinations on one hub: one best-effort, one reliable. A reliable
    // link is only dispatched as such if its destination was registered
    // reliable, so the two cannot share a name.
    let be_name = DestinationName::new("hub", ["besteffort"]);
    let be_dest = be_name.destination_hash(hub_id.public());
    hub.register(be_name.clone(), b"be");
    let rel_name = DestinationName::new("hub", ["reliable"]);
    let rel_dest = rel_name.destination_hash(hub_id.public());
    hub.register_reliable(rel_name.clone(), b"rel");

    let leaf = Endpoint::new(PrivateIdentity::from_secret_bytes(&[22u8; 64]));
    let leaf_iface = leaf.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&be_name, b"be");
        hub.announce(&rel_name, b"rel");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&leaf, be_dest).await;
    await_resolve(&leaf, rel_dest).await;

    let hub_identity = *hub_id.public();

    // Best-effort first: this is the hub-side interface for this leaf.
    let _be_client = leaf.open(be_dest, hub_identity).await.unwrap();
    let be_accepted = tokio::time::timeout(Duration::from_secs(5), hub.accept_on_any())
        .await
        .expect("best-effort accept should not time out")
        .unwrap();

    // Reliable over the same bearer must report that same interface.
    let rel_client = leaf.open_reliable(rel_dest, hub_identity).await.unwrap();
    let rel_accepted = tokio::time::timeout(Duration::from_secs(10), hub.accept_reliable_on_any())
        .await
        .expect("reliable accept should not time out")
        .unwrap();

    assert_eq!(
        rel_accepted.interface, be_accepted.interface,
        "a reliable accept must report the same ingress as a best-effort accept \
         over the same bearer"
    );
    assert_eq!(
        rel_accepted.destination, rel_dest,
        "reliable dispatch retains the destination it targeted"
    );
    // And the initiator's own reliable stream knows the interface it went out on.
    assert_eq!(
        rel_client.interface(),
        leaf_iface,
        "an outbound reliable stream reports the interface it was opened over"
    );
}

/// An outbound best-effort stream reports the interface it was opened over, so
/// both directions of a session can be attributed to a bearer.
#[tokio::test]
async fn outbound_stream_reports_its_interface() {
    let hub_id = PrivateIdentity::from_secret_bytes(&[31u8; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let hub_name = DestinationName::new("hub", ["out"]);
    let hub_dest = hub_name.destination_hash(hub_id.public());
    hub.register(hub_name.clone(), b"out");

    let leaf = Endpoint::new(PrivateIdentity::from_secret_bytes(&[32u8; 64]));
    let iface = leaf.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&hub_name, b"out");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&leaf, hub_dest).await;

    let stream = leaf.open(hub_dest, *hub_id.public()).await.unwrap();
    assert_eq!(
        stream.interface(),
        iface,
        "an outbound stream reports the interface it was opened over"
    );
}

/// A verified multi-destination burst on one bearer is bounded and released later; another
/// bearer remains admissible, and a repeat destination is learned locally but not relayed.
/// This is a host ingress receipt, not a radio-airtime or firmware-memory measurement.
///
/// The runtime clock is paused: the admission state machine reads `tokio::time::Instant`, so
/// every `sleep` below advances virtual time exactly and the burst spacing (1 ms against a
/// 20 ms interface period) is the same on every OS, whatever the host timer granularity.
#[tokio::test(start_paused = true)]
async fn announce_ingress_burst_is_bounded_attributed_and_does_not_silence_a_neighbor() {
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[71u8; 64]));
    hub.enable_routing();
    let noisy = hub.attach_interface();
    let noisy_id = noisy.id();
    let noisy_sink = noisy.sink();
    let quiet = hub.attach_interface();
    let quiet_id = quiet.id();
    let quiet_sink = quiet.sink();
    let _egress = hub.attach_interface();

    // Keep the production 3/10 Hz defaults in the policy tests. This accelerated receipt
    // preserves the same burst/release relationship without waiting several real seconds.
    let policy = AnnounceIngressPolicy {
        held_capacity: 4,
        burst_hold: Duration::from_millis(20),
        burst_penalty: Duration::from_millis(20),
        held_release_interval: Duration::from_millis(5),
        new_interface_hz: 50,
        established_interface_hz: 50,
        ..AnnounceIngressPolicy::default()
    };
    hub.set_announce_ingress_policy(policy);

    let mut burst_destinations = Vec::new();
    for (seed, aspect) in [
        (81, "one"),
        (82, "two"),
        (83, "three"),
        (84, "four"),
        (85, "five"),
        (86, "six"),
        (87, "seven"),
        (88, "eight"),
        (89, "nine"),
        (90, "ten"),
    ] {
        let (destination, packet) = signed_announce(seed, aspect).await;
        burst_destinations.push(destination);
        assert!(noisy_sink.deliver(packet));
        // The state machine measures frequency rather than packet-loop iterations. Advancing
        // the paused clock gives every verified arrival a distinct, exactly 1 ms apart, observation.
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let (quiet_destination, quiet_packet) = signed_announce(91, "quiet").await;
    assert!(quiet_sink.deliver(quiet_packet));

    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline {
        let noisy_counters = hub.announce_ingress_counters(noisy_id);
        if noisy_counters.released >= 1 && hub.resolve(quiet_destination).is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(2)).await;
    }

    let noisy_counters = hub.announce_ingress_counters(noisy_id);
    assert!(
        noisy_counters.held >= 4,
        "the burst must enter the bounded hold queue"
    );
    assert!(
        noisy_counters.held_dropped >= 1,
        "the queue ceiling must reject excess verified announces"
    );
    assert!(
        noisy_counters.released >= 1,
        "at least one held announce must return after the burst penalty"
    );
    assert!(
        hub.resolve(quiet_destination).is_some(),
        "a quiet neighboring bearer must remain admissible"
    );
    assert_eq!(
        hub.announce_ingress_counters(quiet_id).held,
        0,
        "the noisy bearer must not attribute its burst to the quiet neighbor"
    );
    assert!(
        burst_destinations
            .iter()
            .any(|destination| hub.resolve(*destination).is_some()),
        "the receipt must include a released burst destination, not only the quiet neighbor"
    );

    let repeat_identity = PrivateIdentity::from_secret_bytes(&[99; 64]);
    let repeat_sender = Endpoint::new(repeat_identity.clone());
    let mut repeat_wire = repeat_sender.attach_interface();
    let repeat_name = DestinationName::new("flood", ["repeat"]);
    let repeat_destination = repeat_name.destination_hash(repeat_identity.public());
    // A repeat must share one sender so its per-destination freshness timebase advances.
    repeat_sender.announce(&repeat_name, b"ingress receipt");
    let first = tokio::time::timeout(Duration::from_secs(1), repeat_wire.next_outbound())
        .await
        .expect("repeat sender queues a first announce")
        .expect("repeat sender remains live");
    repeat_sender.announce(&repeat_name, b"ingress receipt");
    let second = tokio::time::timeout(Duration::from_secs(1), repeat_wire.next_outbound())
        .await
        .expect("repeat sender queues a second announce")
        .expect("repeat sender remains live");
    assert!(quiet_sink.deliver(first));
    tokio::time::sleep(Duration::from_millis(3)).await;
    assert!(quiet_sink.deliver(second));
    let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
    while tokio::time::Instant::now() < deadline
        && hub.routing_counters().relay_rate_limited_announces == 0
    {
        tokio::time::sleep(Duration::from_millis(2)).await;
    }
    assert!(
        hub.resolve(repeat_destination).is_some(),
        "destination rate pressure never suppresses a valid local learn"
    );
    assert!(
        hub.routing_counters().relay_rate_limited_announces >= 1,
        "the fresh repeat is not re-broadcast after its destination rate block"
    );
}

/// A raw interface into `hub` that sends crafted link requests and collects the proofs.
struct RequestWire {
    out: retinue::endpoint::OutboundPackets,
    sink: retinue::endpoint::InterfaceSink,
}

impl RequestWire {
    fn new(hub: &Endpoint) -> Self {
        let (out, sink) = hub.attach_interface().split();
        Self { out, sink }
    }

    /// Send one link request for `destination` with ephemeral `seed`, returning the
    /// initiator half that can verify its proof.
    fn request(
        &self,
        destination: retinue::hash::AddressHash,
        server: retinue::identity::Identity,
        seed: u16,
    ) -> retinue::link::PendingLink {
        use retinue::link::{LinkMode, LinkTrailer, PendingLink};
        let mut ephemeral = [0x77u8; 64];
        ephemeral[..2].copy_from_slice(&seed.to_be_bytes());
        let (pending, request) = PendingLink::open(
            destination,
            server,
            &ephemeral,
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        assert!(
            self.sink.deliver(request),
            "the hub's router took the request"
        );
        pending
    }

    /// Every proof the hub sends until the wire has been quiet for a moment.
    async fn proofs(&mut self) -> Vec<retinue::Packet> {
        let mut proofs = Vec::new();
        while let Ok(Some(pkt)) =
            tokio::time::timeout(Duration::from_millis(300), self.out.recv()).await
        {
            if pkt.packet_type == retinue::packet::PacketType::Proof {
                proofs.push(pkt);
            }
        }
        proofs
    }

    /// The next `n` proofs, without waiting for the wire to go quiet.
    async fn proofs_n(&mut self, n: usize) -> Vec<retinue::Packet> {
        let mut proofs = Vec::new();
        while proofs.len() < n {
            let pkt = tokio::time::timeout(Duration::from_secs(2), self.out.recv())
                .await
                .expect("the hub proves the request")
                .expect("the hub is live");
            if pkt.packet_type == retinue::packet::PacketType::Proof {
                proofs.push(pkt);
            }
        }
        proofs
    }
}

fn inbound_links(ep: &Endpoint) -> usize {
    ep.link_facts()
        .iter()
        .filter(|f| f.direction == retinue::endpoint::LinkDirection::Inbound)
        .count()
}

/// Prove whichever of `pending` each proof answers, returning the established links.
fn establish(
    pending: &[retinue::link::PendingLink],
    proofs: &[retinue::Packet],
) -> Vec<retinue::link::Link> {
    proofs
        .iter()
        .map(|proof| {
            pending
                .iter()
                .find_map(|p| (p.link_id() == proof.destination).then(|| p.prove(proof)))
                .expect("the proof answers one of our requests")
                .expect("a genuine proof")
        })
        .collect()
}

impl RequestWire {
    /// Activate `link` on the hub the way an initiator does: send its RTT packet.
    fn activate(&self, link: &retinue::link::Link, iv: u8) {
        assert!(self.sink.deliver(link.rtt_packet(0.01, &[iv; 16])));
    }
}

/// Wait until `ep` holds `n` inbound links.
async fn await_inbound_links(ep: &Endpoint, n: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while inbound_links(ep) != n && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(inbound_links(ep), n);
}

fn has_link(ep: &Endpoint, link: &retinue::link::Link) -> bool {
    ep.link_facts().iter().any(|f| f.id == link.id())
}

/// A flood of link requests cannot raise the inbound link count past its caps (review #18).
///
/// Every valid request used to get a proof, tasks, and buffers, without limit. Now a request
/// at a cap displaces the oldest link that never activated, so the count holds, and is
/// refused only when every link it could displace is active. A slot frees when its link
/// closes.
#[tokio::test]
async fn a_link_request_flood_stays_at_the_inbound_caps() {
    use retinue::endpoint::InboundLinkLimits;

    let hub_id = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let hub = Endpoint::new(hub_id.clone());
    hub.set_inbound_link_limits(InboundLinkLimits {
        total: 5,
        per_destination: 3,
    });
    let busy = DestinationName::new("flood", ["busy"]);
    let quiet = DestinationName::new("flood", ["quiet"]);
    let busy_dest = busy.destination_hash(hub_id.public());
    let quiet_dest = quiet.destination_hash(hub_id.public());
    hub.register(busy, b"");
    hub.register(quiet, b"");
    let server = *hub_id.public();

    let mut wire = RequestWire::new(&hub);
    for seed in 0..20 {
        wire.request(busy_dest, server, seed);
    }
    assert_eq!(wire.proofs().await.len(), 20, "each displaces a stale one");
    assert_eq!(
        inbound_links(&hub),
        3,
        "one destination stops at its own cap"
    );
    assert_eq!(hub.routing_counters().inbound_links_evicted, 17);

    // Two quiet links that activate fill the total cap.
    let pending: Vec<_> = (100..102)
        .map(|seed| wire.request(quiet_dest, server, seed))
        .collect();
    let mut quiet_links = establish(&pending, &wire.proofs().await);
    for (i, link) in quiet_links.iter().enumerate() {
        wire.activate(link, i as u8);
    }
    assert_eq!(
        inbound_links(&hub),
        5,
        "the total cap binds across destinations"
    );

    // At the total cap, a third quiet request displaces a stale busy link.
    let pending = [wire.request(quiet_dest, server, 200)];
    quiet_links.extend(establish(&pending, &wire.proofs().await));
    wire.activate(&quiet_links[2], 2);
    assert_eq!(hub.routing_counters().inbound_links_evicted, 18);
    assert_eq!(inbound_links(&hub), 5);

    // Every quiet link is active now, so a fourth is refused outright.
    wire.request(quiet_dest, server, 201);
    assert!(wire.proofs().await.is_empty(), "nothing stale to displace");
    assert_eq!(hub.routing_counters().inbound_links_refused, 1);
    assert!(
        quiet_links.iter().all(|link| has_link(&hub, link)),
        "active links are never displaced"
    );

    // Close one from the initiator side: its slot frees for a new request.
    assert!(wire.sink.deliver(quiet_links[0].close_packet(&[0x42; 16])));
    await_inbound_links(&hub, 4).await;
    wire.request(quiet_dest, server, 202);
    assert_eq!(wire.proofs().await.len(), 1, "and a new request takes it");
    assert_eq!(inbound_links(&hub), 5);
}

/// Requests that never complete the handshake cannot lock a destination out: a genuine
/// initiator still gets a link, and keeps it however many stale requests follow.
#[tokio::test]
async fn half_open_requests_cannot_lock_out_a_genuine_initiator() {
    use retinue::endpoint::InboundLinkLimits;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let hub_id = PrivateIdentity::from_secret_bytes(&[0x63; 64]);
    let hub = Endpoint::new(hub_id.clone());
    hub.set_inbound_link_limits(InboundLinkLimits {
        total: 8,
        per_destination: 4,
    });
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let name = DestinationName::new("flood", ["guarded"]);
    let dest = name.destination_hash(hub_id.public());
    hub.register(name.clone(), b"");

    let mut wire = RequestWire::new(&hub);
    for seed in 0..4 {
        wire.request(dest, *hub_id.public(), seed);
    }
    assert_eq!(wire.proofs().await.len(), 4);
    assert_eq!(inbound_links(&hub), 4, "the destination is at its cap");

    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x64; 64]));
    client.attach_tcp_client(addr).await.unwrap();
    for _ in 0..4 {
        hub.announce(&name, b"");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&client, dest).await;
    let mut stream =
        tokio::time::timeout(Duration::from_secs(5), client.open(dest, *hub_id.public()))
            .await
            .expect("link setup finishes")
            .expect("the genuine initiator gets a link");
    stream.write_all(b"hello").await.unwrap();

    // The application finds the genuine link among the queued ones (displaced stale ones
    // read as ended), and its data arrives.
    let mut served = None;
    for _ in 0..5 {
        let accepted = tokio::time::timeout(Duration::from_secs(1), hub.accept())
            .await
            .expect("a queued link is ready")
            .unwrap();
        if accepted.link_id() == stream.link_id() {
            served = Some(accepted);
            break;
        }
    }
    let mut served = served.expect("the genuine link was accepted");
    let mut got = [0u8; 5];
    tokio::time::timeout(Duration::from_secs(5), served.read_exact(&mut got))
        .await
        .expect("data arrives")
        .unwrap();
    assert_eq!(&got, b"hello");

    // More stale requests displace each other, never the active link.
    for seed in 10..30 {
        wire.request(dest, *hub_id.public(), seed);
    }
    assert_eq!(wire.proofs().await.len(), 20);
    assert_eq!(inbound_links(&hub), 4);
    stream.write_all(b"again").await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), served.read_exact(&mut got))
        .await
        .expect("data still arrives")
        .unwrap();
    assert_eq!(&got, b"again");
}

/// Links waiting for `accept` are bounded per destination, so a destination nobody accepts
/// from cannot accumulate links without limit, and does not block another destination.
#[tokio::test]
async fn unaccepted_links_are_bounded_per_destination() {
    use retinue::endpoint::InboundLinkLimits;

    let hub_id = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let hub = Endpoint::new(hub_id.clone());
    hub.set_inbound_link_limits(InboundLinkLimits {
        total: 1_000,
        per_destination: 1_000,
    });
    let name = DestinationName::new("flood", ["unread"]);
    let dest = name.destination_hash(hub_id.public());
    hub.register(name, b"");
    let other = DestinationName::new("flood", ["other"]);
    let other_dest = other.destination_hash(hub_id.public());
    hub.register(other, b"");

    let mut wire = RequestWire::new(&hub);
    for seed in 0..100 {
        wire.request(dest, *hub_id.public(), seed);
    }
    let admitted = wire.proofs().await.len();
    assert_eq!(admitted, 64, "the destination's backlog holds 64 links");
    assert_eq!(hub.routing_counters().inbound_links_refused, 36);

    // Another destination of the same kind is unaffected.
    wire.request(other_dest, *hub_id.public(), 300);
    assert_eq!(
        wire.proofs().await.len(),
        1,
        "the backlog is per destination"
    );

    // Taking one from the queue makes room for one more.
    let _stream = tokio::time::timeout(Duration::from_secs(1), hub.accept())
        .await
        .expect("a queued link is ready")
        .unwrap();
    wire.request(dest, *hub_id.public(), 500);
    assert_eq!(wire.proofs().await.len(), 1);
}

/// The caps hold for reliable links too, and a reliable link's slot frees when its driver
/// gives up on a peer that stopped answering.
#[tokio::test]
async fn reliable_links_are_capped_and_free_their_slot_when_the_driver_gives_up() {
    use retinue::endpoint::InboundLinkLimits;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let hub_id = PrivateIdentity::from_secret_bytes(&[0x65; 64]);
    let hub = Endpoint::new(hub_id.clone());
    hub.set_inbound_link_limits(InboundLinkLimits {
        total: 8,
        per_destination: 2,
    });
    // A short RTT estimate keeps the give-up (five backed-off tries) under a second.
    hub.set_reliable_initial_rtt(Duration::from_millis(10));
    let name = DestinationName::new("flood", ["reliable"]);
    let dest = name.destination_hash(hub_id.public());
    hub.register_reliable(name, b"");

    let mut wire = RequestWire::new(&hub);
    let pending: Vec<_> = (0..2)
        .map(|seed| wire.request(dest, *hub_id.public(), seed))
        .collect();
    // Activate promptly: the hub times its channel by the request-to-RTT-packet delay.
    let links = establish(&pending, &wire.proofs_n(2).await);
    for (i, link) in links.iter().enumerate() {
        wire.activate(link, i as u8);
    }
    wire.request(dest, *hub_id.public(), 2);
    assert!(
        wire.proofs().await.is_empty(),
        "both reliable slots are active"
    );
    assert_eq!(hub.routing_counters().inbound_links_refused, 1);

    // Write to one; the wire never proves it, so the driver gives up and closes the link.
    let mut stream = tokio::time::timeout(Duration::from_secs(1), hub.accept_reliable())
        .await
        .expect("a queued link is ready")
        .unwrap();
    stream.write_all(b"anyone there?").await.unwrap();
    let mut sink = Vec::new();
    let err = tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut sink))
        .await
        .expect("the driver gives up")
        .expect_err("the reader sees the failure");
    assert_eq!(err.kind(), std::io::ErrorKind::TimedOut);
    await_inbound_links(&hub, 1).await;

    let pending = [wire.request(dest, *hub_id.public(), 3)];
    assert_eq!(
        establish(&pending, &wire.proofs().await).len(),
        1,
        "the freed slot takes a new request"
    );
}

/// A best-effort reader that falls too far behind gets an error, not a stream with a hole.
#[tokio::test]
async fn an_overrun_best_effort_stream_fails_instead_of_losing_bytes_silently() {
    use tokio::io::AsyncReadExt;

    let hub_id = PrivateIdentity::from_secret_bytes(&[0x66; 64]);
    let hub = Endpoint::new(hub_id.clone());
    let name = DestinationName::new("flood", ["slow-reader"]);
    let dest = name.destination_hash(hub_id.public());
    hub.register(name, b"");

    let mut wire = RequestWire::new(&hub);
    let pending = [wire.request(dest, *hub_id.public(), 0)];
    let link = establish(&pending, &wire.proofs().await).remove(0);

    // Far more than the stream's buffer and queue hold, with nobody reading.
    let chunk = [0x5au8; 400];
    let mut sent = 0usize;
    for n in 0u32..1_000 {
        let mut iv = [0u8; 16];
        iv[..4].copy_from_slice(&n.to_be_bytes());
        assert!(wire.sink.deliver(link.data_packet(&chunk, &iv)));
        // Let the router take it, so the router's own queue is not what overflows.
        for _ in 0..4 {
            tokio::task::yield_now().await;
        }
        sent += chunk.len();
        if hub.routing_counters().link_queue_dropped > 0 {
            break;
        }
    }
    assert_eq!(wire.sink.dropped(), 0, "the router kept up");
    assert!(
        hub.routing_counters().link_queue_dropped > 0,
        "the link queue overflowed"
    );
    await_inbound_links(&hub, 0).await;

    let mut stream = hub.accept().await.unwrap();
    let mut got = Vec::new();
    let err = tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut got))
        .await
        .expect("the stream ends")
        .expect_err("with an error, not a clean end");
    assert_eq!(err.kind(), std::io::ErrorKind::Other);
    assert!(got.len() < sent, "bytes past the overrun were lost");
    assert!(got.iter().all(|b| *b == 0x5a));
}
