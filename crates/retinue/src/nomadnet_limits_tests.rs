use super::*;
use crate::identity::PrivateIdentity;
use alloc::vec;

async fn exchange(
    body: Vec<u8>,
    page_cap: usize,
    limits: ResourceReceiveLimits,
) -> (io::Result<Vec<u8>>, io::Result<PayloadMode>) {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x66; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let node = StaticNode::new();
        let destination = node.destination(server.identity());
        let peer = *server.identity();
        node.register(&server);
        let address = server
            .listen_tcp("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let request = accepted.session.receive_request().await.unwrap();
            let result = accepted
                .session
                .respond_auto(request.request_id, body)
                .await;
            (result, accepted, server)
        });
        let client = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x67; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        client.attach_tcp_client(address).await.unwrap();
        let result = fetch_page_with_limits(
            &client,
            destination,
            peer,
            INDEX_PATH,
            FetchPageConfig {
                max_page_bytes: page_cap,
                transfer: ResourceTransferConfig {
                    timeout: Duration::from_secs(3),
                    ..Default::default()
                },
            },
            limits,
        )
        .await;
        client.shutdown(Duration::from_secs(1)).await;
        let (sent, accepted, server) = task.await.unwrap();
        drop(accepted);
        server.shutdown(Duration::from_secs(1)).await;
        (result, sent)
    })
    .await
    .expect("bounded native limit exchange")
}

#[tokio::test]
async fn native_packet_and_resource_exact_body_fit_then_plus_one_refuses() {
    for size in [0, 1, 255, 256, 4000] {
        let data = (0..size)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let limits = ResourceReceiveLimits {
            max_encoded_bytes: 16_000,
            max_decoded_bytes: size + 29,
        };
        let (result, sent) = exchange(data.clone(), size, limits).await;
        assert_eq!(result.unwrap(), data);
        assert_eq!(
            sent.unwrap(),
            if size == 4000 {
                PayloadMode::Resource
            } else {
                PayloadMode::Data
            }
        );
        if size > 0 {
            let (result, sent) = exchange(data, size - 1, limits).await;
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
            assert!(
                sent.is_ok(),
                "the exact body check follows a valid wire exchange"
            );
        }
    }
}

#[tokio::test]
async fn native_encoded_and_decoded_envelope_limits_reject_independently() {
    for size in [32, 4000] {
        for limits in [
            ResourceReceiveLimits {
                max_encoded_bytes: 1,
                max_decoded_bytes: 8000,
            },
            ResourceReceiveLimits {
                max_encoded_bytes: 8000,
                max_decoded_bytes: 1,
            },
        ] {
            let (result, sent) = exchange(vec![b'x'; size], size, limits).await;
            assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
            if size == 4000 {
                assert_eq!(sent.unwrap_err().kind(), io::ErrorKind::ConnectionAborted);
            }
        }
    }
}

#[tokio::test]
async fn refused_resource_releases_request_for_a_fresh_read_on_the_same_endpoint() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x66; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        let node = StaticNode::new();
        let destination = node.destination(server.identity());
        let peer = *server.identity();
        node.register(&server);
        let address = server
            .listen_tcp("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let task = tokio::spawn(async move {
            let mut outcomes = Vec::new();
            let mut held = Vec::new();
            for body in [vec![b'x'; 4000], b"ok".to_vec()] {
                let mut accepted = server.accept_resource().await.unwrap();
                let request = accepted.session.receive_request().await.unwrap();
                outcomes.push(
                    accepted
                        .session
                        .respond_auto(request.request_id, body)
                        .await,
                );
                held.push(accepted);
            }
            (outcomes, held, server)
        });
        let client = Endpoint::with_network_stamp_policy(
            PrivateIdentity::from_secret_bytes(&[0x67; 64]),
            crate::network_stamp::NetworkStampPolicy::rns_1_6_0(),
        );
        client.attach_tcp_client(address).await.unwrap();
        let config = FetchPageConfig {
            max_page_bytes: 4000,
            transfer: ResourceTransferConfig::default(),
        };
        let error = fetch_page_with_limits(
            &client,
            destination,
            peer,
            INDEX_PATH,
            config,
            ResourceReceiveLimits {
                max_encoded_bytes: 8000,
                max_decoded_bytes: 1,
            },
        )
        .await
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert_eq!(
            fetch_page_with_limits(
                &client,
                destination,
                peer,
                INDEX_PATH,
                config,
                ResourceReceiveLimits::default()
            )
            .await
            .unwrap(),
            b"ok"
        );
        client.shutdown(Duration::from_secs(1)).await;
        let (outcomes, held, server) = task.await.unwrap();
        assert_eq!(
            outcomes[0].as_ref().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert_eq!(*outcomes[1].as_ref().unwrap(), PayloadMode::Data);
        drop(held);
        server.shutdown(Duration::from_secs(1)).await;
    })
    .await
    .expect("refusal must not poison a later native read");
}
