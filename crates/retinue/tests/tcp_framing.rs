//! R1 framing, replayed against a real RNS 1.3.8 TCP stream.
//!
//! `tcp_stream.bin` is the literal byte stream RNS wrote to a socket. Nothing here is
//! synthetic: if our de-framing is wrong, these fail.
//!
//! Regenerate with `oracle/.venv/Scripts/python.exe -u oracle/capture_tcp.py`.

use retinue::announce::Announce;
use retinue::iface::hdlc::{Deframer, frame};
use retinue::packet::{Packet, PacketType};

fn fixture(name: &str) -> Vec<u8> {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
    std::fs::read(format!("{path}{name}"))
        .unwrap_or_else(|e| panic!("missing fixture {name}: {e}. Run oracle/capture_tcp.py."))
}

/// De-frame the real stream and find RNS's announces in it.
#[test]
fn we_can_deframe_a_real_rns_tcp_stream() {
    let stream = fixture("tcp_stream.bin");
    let mut d = Deframer::new();
    let frames = d.push(&stream);

    // RNS sent an interface-chatter packet plus our two announces.
    assert!(
        frames.len() >= 2,
        "expected several frames, got {}",
        frames.len()
    );

    let announces: Vec<Announce> = frames
        .iter()
        .filter_map(|f| Packet::decode(f).ok())
        .filter(|p| p.packet_type == PacketType::Announce)
        .map(|p| Announce::decode(&p).expect("RNS announce must validate"))
        .collect();

    assert_eq!(announces.len(), 2, "expected two announces in the stream");
    for a in &announces {
        assert_eq!(
            a.destination.to_string(),
            "a8725a7e212dace39e9f99a8ac5da28c"
        );
    }
    // One of them carries the adversarial app_data full of HDLC special bytes.
    assert!(
        announces
            .iter()
            .any(|a| a.app_data == [0x7e, 0x7d, 0x7e, 0x7d, 0x00, 0xff]),
        "the escape-exercising announce did not survive de-framing intact",
    );
}

/// TCP splits wherever it likes. Feed the real stream one byte at a time and the same
/// frames must fall out.
#[test]
fn a_real_stream_survives_being_split_arbitrarily() {
    let stream = fixture("tcp_stream.bin");

    let mut whole = Deframer::new();
    let expected = whole.push(&stream);

    let mut drip = Deframer::new();
    let mut got = Vec::new();
    for &b in &stream {
        got.extend(drip.push(&[b]));
    }
    assert_eq!(got, expected);

    let mut chunky = Deframer::new();
    let mut got = Vec::new();
    for chunk in stream.chunks(7) {
        got.extend(chunky.push(chunk));
    }
    assert_eq!(got, expected);
}

/// The destination hash of the fixture identity contains a literal `0x7E`, so RNS had to
/// escape it. Our framing must produce the identical stuffing.
#[test]
fn our_framing_reproduces_rns_byte_stuffing() {
    let packet = fixture("tcp_frame_announce.bin");
    let framed = frame(&packet);

    // a8 72 5a 7e ... must go out as a8 72 5a 7d 5e ...
    let needle = [0xa8, 0x72, 0x5a, 0x7d, 0x5e, 0x21];
    assert!(
        framed.windows(needle.len()).any(|w| w == needle),
        "the 0x7E in the destination hash was not escaped to 7d 5e",
    );

    // And it must appear in the real stream exactly the same way.
    let stream = fixture("tcp_stream.bin");
    assert!(stream.windows(needle.len()).any(|w| w == needle));

    let mut d = Deframer::new();
    assert_eq!(d.push(&framed), vec![packet]);
}

/// The escape byte itself is escaped too. Proven by asking RNS to announce app_data
/// containing both special bytes, and reading what it put on the wire.
#[test]
fn our_framing_escapes_the_escape_byte_like_rns_does() {
    let packet = fixture("tcp_frame_announce_escapes.bin");
    let framed = frame(&packet);

    // app_data 7e 7d 7e 7d 00 ff goes out as 7d5e 7d5d 7d5e 7d5d 00 ff.
    let needle = [0x7d, 0x5e, 0x7d, 0x5d, 0x7d, 0x5e, 0x7d, 0x5d, 0x00, 0xff];
    assert!(
        framed.windows(needle.len()).any(|w| w == needle),
        "our stuffing of the app_data does not match what RNS emitted",
    );

    let stream = fixture("tcp_stream.bin");
    assert!(
        stream.windows(needle.len()).any(|w| w == needle),
        "sanity: RNS's own stream should contain this stuffing",
    );

    let a = Announce::decode(&Packet::decode(&packet).unwrap()).unwrap();
    assert_eq!(a.app_data, [0x7e, 0x7d, 0x7e, 0x7d, 0x00, 0xff]);
}

/// The stream carriers' framing options, end to end through the endpoint.
#[cfg(feature = "tokio")]
mod carriers {
    use std::net::{Ipv6Addr, SocketAddr};
    use std::time::Duration;

    use retinue::destination::DestinationName;
    use retinue::endpoint::{Endpoint, FlapPolicy, ListenAddr, ListenPolicy, TcpClient};
    use retinue::identity::PrivateIdentity;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpStream;
    use tokio::sync::mpsc;
    use tokio::time::timeout;

    const WAIT: Duration = Duration::from_secs(5);

    /// A KISS listener and a KISS client carry an announce (`TCPInterface.py` 322-388).
    #[tokio::test]
    async fn kiss_framed_endpoints_hear_each_other() {
        let server_id = PrivateIdentity::from_secret_bytes(&[0x4b; 64]);
        let server = Endpoint::with_network_stamp_policy(
            server_id.clone(),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let policy = ListenPolicy {
            kiss_framing: true,
            ..ListenPolicy::default()
        };
        let listener = server
            .listen_tcp_with("127.0.0.1:0".parse().unwrap(), policy)
            .await
            .unwrap();
        let client = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x4c; 64]),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let dial = TcpClient {
            kiss_framing: true,
            ..TcpClient::new("127.0.0.1", listener.local_addr().port())
        };
        client.attach_tcp(dial).await.unwrap();
        let name = DestinationName::new("kiss", ["svc"]);
        let dest = name.destination_hash(server_id.public());
        server.register(name.clone(), b"svc");
        let heard = timeout(WAIT, async {
            loop {
                server.announce(&name, b"svc");
                let next = timeout(Duration::from_millis(200), client.next_announcement());
                if let Ok(Ok(announce)) = next.await
                    && announce.destination == dest
                {
                    return;
                }
            }
        });
        heard.await.expect("the KISS client heard the announce");
    }

    /// Six fast flaps from one address block its seventh connection, while another address
    /// is still served (`BackboneInterface.py` 807-835, 1084-1109).
    #[tokio::test]
    async fn fast_flapping_addresses_are_refused() {
        let ep = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x66; 64]),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let (spawned, mut ids) = mpsc::channel(16);
        let policy = ListenPolicy {
            flap: Some(FlapPolicy::default()),
            spawned: Some(spawned),
            ..ListenPolicy::default()
        };
        let any = ListenAddr::Socket("[::]:0".parse().unwrap());
        let port = ep.listen_on(any, policy).await.unwrap().local_addr().port();
        let v4 = SocketAddr::from(([127, 0, 0, 1], port));
        // `[::]` takes IPv4 only on a dual-stack host (bindv6only off).
        let Ok(first) = TcpStream::connect(v4).await else {
            eprintln!("skipped: [::] does not accept IPv4 here");
            return;
        };
        let mut first = Some(first);
        for _ in 0..6 {
            let stream = match first.take() {
                Some(stream) => stream,
                None => TcpStream::connect(v4).await.unwrap(),
            };
            let id = timeout(WAIT, ids.recv()).await.unwrap().unwrap();
            drop(stream);
            let gone = async {
                while ep.interface_ids().contains(&id) {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            };
            timeout(WAIT, gone)
                .await
                .expect("the flap ended its interface");
        }
        let mut refused = TcpStream::connect(v4).await.unwrap();
        let read = timeout(WAIT, refused.read(&mut [0; 1])).await.unwrap();
        assert_eq!(read.unwrap_or(0), 0, "the seventh connection is closed");
        assert!(ids.try_recv().is_err(), "and gets no interface");

        let _v6 = TcpStream::connect((Ipv6Addr::LOCALHOST, port))
            .await
            .unwrap();
        assert!(timeout(WAIT, ids.recv()).await.unwrap().is_some());
    }
}
