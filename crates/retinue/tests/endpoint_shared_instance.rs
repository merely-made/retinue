//! Attaching to a shared instance (`LocalInterface.py` 53-202): a stand-in instance on
//! loopback TCP, and on Linux the abstract unix socket `\0rns/<name>`.
#![cfg(feature = "tokio")]

use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, SharedInstance, SharedInstanceKind};
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::iface::hdlc::{Deframer, frame};
use retinue::packet::{HeaderType, Packet, PacketType};

/// An announce for `name`, re-emitted by an instance as rnsd does: header-type 2 through
/// `instance` at `hops` (`Transport.py` 2398-2425).
async fn relayed_announce(name: &DestinationName, instance: AddressHash, hops: u8) -> Packet {
    let peer = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x77; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let mut raw = peer.attach_interface();
    peer.register(name.clone(), b"");
    peer.announce(name, b"");
    let mut pkt = raw.next_outbound().await.expect("an announce");
    assert_eq!(pkt.packet_type, PacketType::Announce);
    pkt.header_type = HeaderType::Type2;
    pkt.transport = Some(instance);
    pkt.hops = hops;
    pkt
}

/// The next data packet for `dest` the client writes.
async fn next_data(sock: &mut TcpStream, deframer: &mut Deframer, dest: AddressHash) -> Packet {
    let mut buf = vec![0u8; 4096];
    let read = async {
        loop {
            let n = sock.read(&mut buf).await.unwrap();
            assert!(n > 0, "the client hung up");
            for raw in deframer.push(&buf[..n]) {
                let pkt = Packet::decode(&raw).unwrap();
                if pkt.destination == dest && pkt.packet_type == PacketType::Data {
                    return pkt;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(5), read)
        .await
        .expect("a data packet")
}

async fn wait_for(mut done: impl FnMut() -> bool) -> bool {
    for _ in 0..100 {
        if done() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    done()
}

#[tokio::test]
async fn tcp_instance_routes_through_rnsd_without_counting_its_hop() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x11; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let instance = SharedInstance {
        port,
        kind: SharedInstanceKind::Tcp,
        ..SharedInstance::default()
    };
    let id = ep.attach_shared_instance(instance).await.unwrap();
    let (mut sock, _) = listener.accept().await.unwrap();
    assert!(ep.interface_online(id));

    let rnsd = AddressHash::from_bytes([0x5d; 16]);
    let name = DestinationName::new("shared", ["peer"]);
    let pkt = relayed_announce(&name, rnsd, 1).await;
    let dest = pkt.destination;
    // rnsd's keepalive is an empty frame, which the deframer skips.
    sock.write_all(&[0x7e, 0x7e]).await.unwrap();
    sock.write_all(&frame(&pkt.encode())).await.unwrap();
    assert!(wait_for(|| ep.route_to(dest).is_some()).await);
    assert_eq!(
        ep.route_to(dest),
        Some((id, 0)),
        "one hop past rnsd reads 0"
    );

    // Another local client of rnsd, re-emitted at hops 0, is reached with header type 1.
    let sibling_name = DestinationName::new("shared", ["sibling"]);
    let sibling = relayed_announce(&sibling_name, rnsd, 0).await;
    sock.write_all(&frame(&sibling.encode())).await.unwrap();
    assert!(wait_for(|| ep.route_to(sibling.destination).is_some()).await);

    // Traffic one hop past rnsd leaves addressed through it.
    ep.send_single(dest, b"hello").ok();
    let mut deframer = Deframer::new();
    let sent = next_data(&mut sock, &mut deframer, dest).await;
    assert_eq!(sent.header_type, HeaderType::Type2);
    assert_eq!(sent.transport, Some(rnsd));
    ep.send_single(sibling.destination, b"hello").ok();
    let sent = next_data(&mut sock, &mut deframer, sibling.destination).await;
    assert_eq!(
        (sent.header_type, sent.transport),
        (HeaderType::Type1, None)
    );

    // The instance goes away: the interface is offline and what it taught is dropped.
    drop(sock);
    assert!(wait_for(|| !ep.interface_online(id)).await);
    assert!(wait_for(|| ep.route_to(dest).is_none()).await);
    assert_eq!(
        ep.interface_ids(),
        vec![id],
        "the id is kept for the reconnect"
    );
}

#[tokio::test]
async fn first_connect_failure_is_reported() {
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x12; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let instance = SharedInstance {
        port,
        kind: SharedInstanceKind::Tcp,
        ..SharedInstance::default()
    };
    assert!(ep.attach_shared_instance(instance).await.is_err());
    assert_eq!(ep.interface_count(), 0);
}

/// Needs a Linux runner: macOS has no abstract unix sockets.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn linux_abstract_socket_is_reached_by_instance_name() {
    use std::os::linux::net::SocketAddrExt;
    use std::os::unix::net::{SocketAddr, UnixListener};

    let name = format!("retinue-test-{}", std::process::id());
    let addr = SocketAddr::from_abstract_name(format!("rns/{name}")).unwrap();
    let listener = UnixListener::bind_addr(&addr).unwrap();
    let ep = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x13; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    for kind in [SharedInstanceKind::Unix, SharedInstanceKind::Auto] {
        let instance = SharedInstance {
            instance_name: name.clone(),
            // Nothing listens here, so Auto must have taken the unix socket.
            port: 1,
            kind,
        };
        let id = ep.attach_shared_instance(instance).await.unwrap();
        let (_sock, _) = listener.accept().unwrap();
        assert!(ep.interface_online(id));
    }
}
