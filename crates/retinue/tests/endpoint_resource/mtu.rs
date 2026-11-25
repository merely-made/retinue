//! Links over TCP negotiate the autoconfigured 16384-byte MTU, and resources use it.

use super::*;

#[tokio::test]
async fn a_tcp_link_takes_the_autoconfigured_mtu_and_carries_a_resource() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
    let server = Arc::new(Endpoint::with_network_stamp_policy(
        server_id.clone(),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let client = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0x72; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let name = DestinationName::new("retinue", ["tcp-mtu"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name.clone(), b"");
    let addr = server
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    client.attach_tcp_client(addr).await.unwrap();
    // The link request leaves by the learned route, so it signals that interface's MTU.
    let learned = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            server.announce(&name, b"");
            if let Ok(Ok(fact)) =
                tokio::time::timeout(Duration::from_millis(200), client.next_announcement()).await
                && fact.destination == destination
            {
                return;
            }
        }
    });
    learned.await.expect("the client learns the route");

    let payload: Vec<u8> = (0..1_048_576_u32)
        .map(|n| (n.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect();
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let mtu = accepted.session.mtu();
            (mtu, accepted.session.receive().await.unwrap())
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    assert_eq!(session.mtu(), 16_384);
    tokio::time::timeout(Duration::from_secs(30), session.publish(&payload))
        .await
        .expect("publish completes")
        .unwrap();
    let (mtu, received) = tokio::time::timeout(Duration::from_secs(10), receiver)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mtu, 16_384);
    assert_eq!(received, ReceivedPayload::Resource(payload));
}

#[tokio::test]
async fn one_endpoint_negotiates_by_interface_in_both_directions() {
    async fn check(from: &Endpoint, to: &Endpoint, name: &DestinationName, expected: u32) {
        let dest = name.destination_hash(to.identity());
        let session = from.open_resource(dest, *to.identity()).await.unwrap();
        let accepted = to.accept_resource().await.unwrap();
        assert_eq!(
            (session.mtu(), accepted.session.mtu()),
            (expected, expected)
        );
    }

    async fn learn(from: &Endpoint, to: &Endpoint, name: &DestinationName) {
        let dest = name.destination_hash(to.identity());
        loop {
            to.announce(name, b"");
            if let Ok(Ok(fact)) =
                tokio::time::timeout(Duration::from_millis(200), from.next_announcement()).await
                && fact.destination == dest
            {
                return;
            }
        }
    }

    tokio::time::timeout(Duration::from_secs(20), async {
        let hub = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[1; 64]),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let radio = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[2; 64]),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let tcp = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[3; 64]),
            retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let name = DestinationName::new("retinue", ["mixed-mtu"]);
        for endpoint in [&hub, &radio, &tcp] {
            endpoint.register_resource(name.clone(), b"");
        }
        let local = "127.0.0.1:0".parse().unwrap();
        let id = hub
            .attach_tcp_client(radio.listen_tcp(local).await.unwrap())
            .await
            .unwrap();
        hub.attach_tcp_client(tcp.listen_tcp(local).await.unwrap())
            .await
            .unwrap();
        assert!(hub.set_interface_link_mtu(id, Some(247)));
        // No route yet. Every copy of the broadcast request must fit the radio.
        check(&hub, &tcp, &name, 247).await;
        for peer in [&radio, &tcp] {
            learn(&hub, peer, &name).await;
            learn(peer, &hub, &name).await;
        }
        for (peer, expected) in [(&radio, 247), (&tcp, 16_384)] {
            check(&hub, peer, &name, expected).await;
            check(peer, &hub, &name, expected).await;
        }
        hub.set_link_mtu(500);
        assert!(hub.set_interface_link_mtu(id, None));
        check(&hub, &radio, &name, 500).await;
        check(&radio, &hub, &name, 500).await;
    })
    .await
    .expect("mixed-interface link negotiation completes");
}
