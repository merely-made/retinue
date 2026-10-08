use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use retinue::announce::{self, AnnounceBlob};
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::iface::tcp::{RecvError, TcpInterface, TcpInterfaceListener};
use retinue::{Error, Ifac, Packet};

fn access(name: &str) -> Ifac {
    Ifac::new(Some(name), Some("interface-test"), 8).unwrap()
}

#[tokio::test]
async fn routing_verifies_ingress_and_reapplies_the_egress_ifac() {
    let router = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x31; 64]));
    router.enable_routing();

    let ingress_access = access("ingress");
    let egress_access = access("egress");
    let ingress = router
        .attach_interface_with_ifac(508, ingress_access.clone())
        .unwrap();
    let egress = router
        .attach_interface_with_ifac(508, egress_access.clone())
        .unwrap();
    let (_ingress_outbound, ingress_sink) = ingress.split();
    let (mut egress_outbound, _egress_sink) = egress.split();

    let peer = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let name = DestinationName::new("retinue", ["ifac-routing"]);
    let announce = announce::build(
        &peer,
        name.name_hash(),
        &AnnounceBlob::from_wire([0x55; announce::RAND_HASH_LEN]),
        None,
        b"private ingress",
    );

    let wrong_wire = access("wrong").seal(&announce.encode()).unwrap();
    assert_eq!(ingress_sink.deliver_frame(&wrong_wire), Err(Error::BadIfac));

    let ingress_wire = ingress_access.seal(&announce.encode()).unwrap();
    assert!(ingress_sink.deliver_frame(&ingress_wire).unwrap());

    let forwarded = tokio::time::timeout(Duration::from_secs(3), egress_outbound.recv())
        .await
        .expect("announce was not forwarded")
        .expect("egress closed");
    let egress_wire = egress_outbound.encode(&forwarded).unwrap();

    assert_eq!(ingress_access.open(&egress_wire), Err(Error::BadIfac));
    let logical = egress_access.open(&egress_wire).unwrap();
    let decoded = Packet::decode(&logical).unwrap();
    assert_eq!(decoded.destination, announce.destination);
    assert_eq!(decoded.hops, announce.hops + 1);
}

#[tokio::test]
async fn tcp_interface_authenticates_both_directions() {
    let credentials = access("tcp");
    let listener = TcpInterfaceListener::bind_with_ifac(
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0),
        credentials.clone(),
    )
    .await
    .unwrap();
    let address = listener.local_addr().unwrap();

    let responder = tokio::spawn(async move {
        let mut interface = listener.accept().await.unwrap();
        let packet = interface.recv().await.unwrap();
        interface.send(&packet).await.unwrap();
    });

    let mut initiator = TcpInterface::connect_with_ifac(address, credentials)
        .await
        .unwrap();
    let identity = PrivateIdentity::from_secret_bytes(&[0x73; 64]);
    let name = DestinationName::new("retinue", ["ifac-tcp"]);
    let packet = announce::build(
        &identity,
        name.name_hash(),
        &AnnounceBlob::from_wire([0x19; announce::RAND_HASH_LEN]),
        None,
        b"authenticated",
    );
    initiator.send(&packet).await.unwrap();
    assert_eq!(initiator.recv().await.unwrap(), packet);
    responder.await.unwrap();
}

/// RNS drops a frame carrying the IFAC flag on an interface without IFAC; so does the
/// endpoint, counting it, and the unflagged frame still routes and leaves unflagged.
#[tokio::test]
async fn plain_interface_refuses_the_ifac_flag_and_forwards_unflagged() {
    let router = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x35; 64]));
    router.enable_routing();
    let (_ingress_outbound, ingress_sink) = router.attach_interface().split();
    let (mut egress_outbound, _egress_sink) = router.attach_interface().split();

    let peer = PrivateIdentity::from_secret_bytes(&[0x46; 64]);
    let name = DestinationName::new("retinue", ["ifac-flag"]);
    let announce = announce::build(
        &peer,
        name.name_hash(),
        &AnnounceBlob::from_wire([0x57; announce::RAND_HASH_LEN]),
        None,
        b"plain ingress",
    );

    let mut flagged = announce.encode();
    flagged[0] |= 0x80;
    assert!(ingress_sink.deliver_frame(&flagged).unwrap());
    tokio::time::timeout(Duration::from_secs(3), async {
        while router.routing_counters().ifac_flag_rejected == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the flagged frame was not refused");
    assert!(
        tokio::time::timeout(Duration::from_millis(200), egress_outbound.recv())
            .await
            .is_err(),
        "a flagged frame was forwarded"
    );

    assert!(ingress_sink.deliver_frame(&announce.encode()).unwrap());
    let forwarded = tokio::time::timeout(Duration::from_secs(3), egress_outbound.recv())
        .await
        .expect("announce was not forwarded")
        .expect("egress closed");
    let wire = egress_outbound.encode(&forwarded).unwrap();
    assert_eq!(wire[0] & 0x80, 0);
    assert_eq!(
        Packet::decode(&wire).unwrap().destination,
        announce.destination
    );
    assert_eq!(router.routing_counters().ifac_flag_rejected, 1);
}

/// A plain endpoint TCP interface refuses a flagged frame in its router.
#[tokio::test]
async fn plain_tcp_endpoint_refuses_the_ifac_flag() {
    let router = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x36; 64]));
    let address = router
        .listen_tcp(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let mut peer = TcpInterface::connect(address).await.unwrap();

    let identity = PrivateIdentity::from_secret_bytes(&[0x47; 64]);
    let name = DestinationName::new("retinue", ["ifac-flag-tcp"]);
    let mut flagged = announce::build(
        &identity,
        name.name_hash(),
        &AnnounceBlob::from_wire([0x58; announce::RAND_HASH_LEN]),
        None,
        b"plain tcp",
    )
    .encode();
    flagged[0] |= 0x80;
    peer.send_raw(&flagged).await.unwrap();

    tokio::time::timeout(Duration::from_secs(3), async {
        while router.routing_counters().ifac_flag_rejected == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the flagged frame was not refused");
}

/// A standalone plain TCP interface refuses a flagged frame, and keeps the connection.
#[tokio::test]
async fn plain_tcp_interface_refuses_the_ifac_flag() {
    let listener = TcpInterfaceListener::bind(SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    let mut initiator = TcpInterface::connect(address).await.unwrap();
    let mut responder = listener.accept().await.unwrap();

    let identity = PrivateIdentity::from_secret_bytes(&[0x48; 64]);
    let name = DestinationName::new("retinue", ["ifac-flag-iface"]);
    let packet = announce::build(
        &identity,
        name.name_hash(),
        &AnnounceBlob::from_wire([0x59; announce::RAND_HASH_LEN]),
        None,
        b"plain interface",
    );
    let mut flagged = packet.encode();
    flagged[0] |= 0x80;
    initiator.send_raw(&flagged).await.unwrap();
    initiator.send(&packet).await.unwrap();

    assert!(matches!(
        responder.recv().await,
        Err(RecvError::Wire(Error::BadIfac))
    ));
    assert_eq!(responder.recv().await.unwrap(), packet);
}
