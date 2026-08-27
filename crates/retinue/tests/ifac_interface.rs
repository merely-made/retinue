use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;

use retinue::announce::{self, AnnounceBlob};
use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::iface::tcp::{TcpInterface, TcpInterfaceListener};
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
