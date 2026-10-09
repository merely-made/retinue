//! Interface admission, link packet memory, and detaching.

use std::string::String;
use std::sync::atomic::{AtomicBool, AtomicU64};

use socket2::SockRef;
use tokio::net::{TcpListener, TcpStream};

use super::super::iface_policy::IfacePolicy;
use super::super::pump::{self, PumpEnd};
use super::super::sockopt;
use super::super::{ListenPolicy, TcpClient};
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
        online: Arc::new(AtomicBool::new(true)),
        unsendable: AtomicU64::new(0),
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

/// A peer that connects and drops repeatedly leaves no interface record behind.
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

/// Detaching forgets the interface, so reconnects do not accumulate records.
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

/// Poll `ready` for up to `within`.
async fn eventually(within: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + within;
    while !ready() {
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    true
}

/// A port nothing listens on.
async fn dead_port() -> u16 {
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    probe.local_addr().unwrap().port()
}

/// RNS's keepalive profiles, read back from the kernel (`TCPInterface.py` 83-95).
#[tokio::test]
async fn stream_sockets_take_the_rns_keepalive_profile() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    for (i2p, idle, interval, probes, _user_timeout) in
        [(false, 5, 2, 12, 24), (true, 10, 9, 5, 45)]
    {
        let stream = TcpStream::connect(addr).await.unwrap();
        sockopt::tune(&stream, i2p).unwrap();
        let socket = SockRef::from(&stream);
        assert!(socket.keepalive().unwrap() && socket.tcp_nodelay().unwrap());
        assert_eq!(
            socket.tcp_keepalive_time().unwrap(),
            Duration::from_secs(idle)
        );
        assert_eq!(
            socket.tcp_keepalive_interval().unwrap(),
            Duration::from_secs(interval)
        );
        assert_eq!(socket.tcp_keepalive_retries().unwrap(), probes);
        #[cfg(any(target_os = "linux", target_os = "android"))]
        assert_eq!(
            socket.tcp_user_timeout().unwrap(),
            Some(Duration::from_secs(_user_timeout))
        );
    }
}

/// A dialed interface outlives its connection: same id, routes kept, online again on redial.
#[tokio::test]
async fn a_dialed_interface_keeps_its_id_and_routes_across_reconnects() {
    let hub = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = hub.local_addr().unwrap().port();
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x61; 64]));
    let id = ep
        .attach_tcp(TcpClient {
            reconnect_wait: Duration::from_millis(50),
            ..TcpClient::new("localhost", port)
        })
        .await
        .unwrap();
    let (first, _) = hub.accept().await.unwrap();
    assert!(ep.interface_online(id));
    let dest = AddressHash::from_bytes([0x62; 16]);
    ep.shared.path_table.lock().unwrap().insert(
        dest,
        PathEntry {
            iface: id,
            transport: None,
            hops: 1,
            learned: Instant::now(),
            mode: InterfaceMode::Full,
        },
    );

    drop(first);
    assert!(eventually(Duration::from_secs(2), || !ep.interface_online(id)).await);
    ep.shared
        .broadcast(single_packet(dest, b"while down".to_vec()));
    assert_eq!(
        ep.outbound_queue_depth(),
        0,
        "an offline interface admits nothing"
    );
    assert_eq!(
        ep.route_to(dest).map(|(iface, _)| iface),
        Some(id),
        "the route survives"
    );

    let (_second, _) = hub.accept().await.unwrap();
    assert!(eventually(Duration::from_secs(2), || ep.interface_online(id)).await);
    assert_eq!(ep.interface_ids(), [id]);
    assert_eq!(ep.route_to(dest).map(|(iface, _)| iface), Some(id));
}

/// What callers queue right after attaching goes out: only a carrier coming back online
/// after an outage discards a backlog.
#[tokio::test]
async fn an_announce_queued_right_after_attaching_is_sent() {
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x69; 64]));
    let addr = hub
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let name = DestinationName::new("retinue", ["attach-then-announce"]);
    let dialer = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x6a; 64]));
    dialer.register(name.clone(), b"");
    dialer.attach_tcp_client(addr).await.unwrap();
    dialer.announce(&name, b"dialed");
    let streamed = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x6b; 64]));
    streamed.register(name.clone(), b"");
    streamed.attach_stream(TcpStream::connect(addr).await.unwrap());
    streamed.announce(&name, b"streamed");

    let mut heard = Vec::new();
    for _ in 0..2 {
        let fact = tokio::time::timeout(Duration::from_secs(3), hub.next_announcement())
            .await
            .expect("the hub hears both announces")
            .unwrap();
        heard.push(fact.app_data);
    }
    heard.sort();
    assert_eq!(heard, [b"dialed".to_vec(), b"streamed".to_vec()]);
}

/// A TCP interface with IFAC still carries a full 500-byte packet: the access code rides on
/// top of the protocol MTU, as on a stock stream carrier.
#[tokio::test]
async fn tcp_interfaces_with_ifac_carry_the_full_mtu() {
    let ifac = Ifac::for_stream(Some("mtu"), None).unwrap();
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x6c; 64]));
    let addr = hub
        .listen_tcp_with_ifac("127.0.0.1:0".parse().unwrap(), ifac.clone())
        .await
        .unwrap();
    let dialer = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x6d; 64]));
    let dialed = dialer
        .attach_tcp_client_with_ifac(addr, ifac.clone())
        .await
        .unwrap();
    let streamed = dialer.attach_stream_with_ifac(TcpStream::connect(addr).await.unwrap(), ifac);
    for id in [dialed, streamed] {
        assert_eq!(
            dialer.shared.link_mtu_on(id),
            Some(crate::packet::MTU as u32)
        );
    }
    let spawned = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(id) = hub.interface_ids().first().copied() {
                return id;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        hub.shared.link_mtu_on(spawned),
        Some(crate::packet::MTU as u32)
    );
}

/// An unreachable hub is attached offline, then forgotten after the configured tries.
#[tokio::test]
async fn an_unreachable_hub_is_retried_then_given_up() {
    let port = dead_port().await;
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x63; 64]));
    let addr = ([127, 0, 0, 1], port).into();
    assert!(
        ep.attach_tcp_client(addr).await.is_err(),
        "the strict dial still fails"
    );
    let id = ep
        .attach_tcp(TcpClient {
            reconnect_wait: Duration::from_millis(20),
            max_reconnect_tries: Some(2),
            ..TcpClient::new("127.0.0.1", port)
        })
        .await
        .expect("an unreachable hub is not an error");
    assert_eq!(ep.interface_ids(), [id]);
    assert!(!ep.interface_online(id));
    assert!(eventually(Duration::from_secs(2), || ep.interface_count() == 0).await);
}

/// A peer that stops reading ends the pump after the dead time; EOF ends it at once.
#[tokio::test(start_paused = true)]
async fn a_stalled_writer_or_a_closed_reader_ends_the_pump() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x64; 64]));
    let iface = ep.attach_interface();
    let id = iface.id();
    let (mut out, _sink) = iface.split();
    for n in 0..8 {
        ep.shared.broadcast(single_packet(
            AddressHash::from_bytes([n; 16]),
            vec![n; 200],
        ));
    }
    let (near, _far) = tokio::io::duplex(64);
    let (reader, writer) = tokio::io::split(near);
    let started = tokio::time::Instant::now();
    assert_eq!(
        pump::run(&ep.shared, id, &mut out, reader, writer).await,
        PumpEnd::WriteStall
    );
    assert_eq!(started.elapsed(), pump::DEAD_TIME);

    let (near, far) = tokio::io::duplex(64);
    drop(far);
    let (reader, writer) = tokio::io::split(near);
    out.discard();
    assert_eq!(
        pump::run(&ep.shared, id, &mut out, reader, writer).await,
        PumpEnd::Eof
    );
}

/// A queued packet that cannot be encoded is dropped and counted, and the next one is sent.
#[tokio::test]
async fn an_unencodable_packet_is_counted_and_skipped() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x65; 64]));
    let ifac = Ifac::for_stream(Some("pump"), None).unwrap();
    let iface = ep.attach_interface_with_ifac(600, ifac.clone()).unwrap();
    let id = iface.id();
    let (mut out, _sink) = iface.split();
    let dest = AddressHash::from_bytes([0x66; 16]);
    // Past the frame limit, so only a direct push gets it queued.
    assert!(
        out.queues
            .push(single_packet(dest, vec![0; 600]), TrafficClass::Interactive)
    );
    assert!(out.queues.push(
        single_packet(dest, b"fits".to_vec()),
        TrafficClass::Interactive
    ));

    let (near, mut far) = tokio::io::duplex(4096);
    let (reader, writer) = tokio::io::split(near);
    let peer = async move {
        let mut deframer = crate::iface::hdlc::Deframer::new();
        let mut buf = [0u8; 1024];
        loop {
            let n = far.read(&mut buf).await.unwrap();
            if let Some(frame) = deframer.push(&buf[..n]).pop() {
                return Packet::decode(&ifac.open(&frame).unwrap()).unwrap();
            }
        }
    };
    let (end, received) = tokio::join!(pump::run(&ep.shared, id, &mut out, reader, writer), peer);
    assert_eq!(end, PumpEnd::Eof);
    assert_eq!(received.payload, b"fits");
    let unsendable = ep
        .shared
        .with_iface(id, |i| i.unsendable.load(Ordering::Relaxed));
    assert_eq!(unsendable, Some(1));
}

/// Spawned connections inherit the listener's mode and policy, report their ids, and a
/// closed listener releases its port.
#[tokio::test]
async fn a_listener_spawns_interfaces_with_its_policy_and_closes() {
    let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
    let (spawned, mut ids) = tokio::sync::mpsc::channel(4);
    let iface = IfacePolicy {
        cap_percent: 10,
        ..IfacePolicy::default()
    };
    let listener = ep
        .listen_tcp_with(
            "127.0.0.1:0".parse().unwrap(),
            ListenPolicy {
                mode: InterfaceMode::AccessPoint,
                iface,
                spawned: Some(spawned),
                ..ListenPolicy::default()
            },
        )
        .await
        .unwrap();
    let _client = TcpStream::connect(listener.local_addr()).await.unwrap();
    let id = ids.recv().await.unwrap();
    assert_eq!(ep.shared.interface_mode(id), InterfaceMode::AccessPoint);
    assert_eq!(ep.shared.iface_policy(id), iface);
    assert!(ep.interface_online(id));

    listener.close();
    let mut refused = false;
    for _ in 0..100 {
        if TcpStream::connect(listener.local_addr()).await.is_err() {
            refused = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(refused, "a closed listener releases its port");
    assert_eq!(
        ep.interface_ids(),
        [id],
        "and leaves its spawned interfaces"
    );
}

/// Descriptor exhaustion makes `accept` fail; the listener backs off and accepts again once
/// descriptors are free. Runs in a child process, since exhausting descriptors here would
/// break concurrent tests.
#[cfg(unix)]
#[test]
fn a_listener_survives_descriptor_exhaustion() {
    const CHILD: &str = "RETINUE_TEST_EMFILE_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = "endpoint::tests::interfaces::a_listener_survives_descriptor_exhaustion";
        // A low soft limit keeps exhaustion cheap where the default is large (Docker: 2^20).
        let output = std::process::Command::new("sh")
            .args(["-c", "ulimit -Sn 256 2>/dev/null; exec \"$0\" \"$@\""])
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", name, "--test-threads=1", "--nocapture"])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the child ran the test: {stdout}"
        );
        return;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x68; 64]));
        let addr = ep.listen_tcp("127.0.0.1:0".parse().unwrap()).await.unwrap();
        let mut hoard = Vec::new();
        while let Ok(file) = std::fs::File::open("/dev/null") {
            hoard.push(file);
            assert!(hoard.len() < 1 << 16, "no descriptor limit to exhaust");
        }
        hoard.pop();
        let _client = TcpStream::connect(addr).await.unwrap();
        tokio::time::sleep(Duration::from_millis(350)).await;
        assert_eq!(
            ep.interface_count(),
            0,
            "accept failed while descriptors ran out"
        );
        drop(hoard);
        // XNU drops the connection whose accept failed; Linux leaves it queued.
        let _another = TcpStream::connect(addr).await.unwrap();
        assert!(eventually(Duration::from_secs(2), || ep.interface_count() >= 1).await);
    });
}
