//! Endpoint-level resource publish/fetch over the raw interface seam.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, PayloadMode, ReceivedPayload, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::link::CTX_RESOURCE_PRF;
use retinue::lossy::{LossModel, connect};
use retinue::request::Request;

fn connect_dropping_first_resource_proof(a: &Endpoint, b: &Endpoint) -> Arc<AtomicBool> {
    let (mut a_out, a_sink) = a.attach_interface().split();
    let (mut b_out, b_sink) = b.attach_interface().split();
    let dropped = Arc::new(AtomicBool::new(false));

    tokio::spawn(async move {
        while let Some(packet) = a_out.recv().await {
            if !b_sink.deliver(packet) {
                break;
            }
        }
    });

    let proof_dropped = Arc::clone(&dropped);
    tokio::spawn(async move {
        while let Some(packet) = b_out.recv().await {
            if packet.context == CTX_RESOURCE_PRF && !proof_dropped.swap(true, Ordering::AcqRel) {
                continue;
            }
            if !a_sink.deliver(packet) {
                break;
            }
        }
    });

    dropped
}

#[tokio::test]
async fn endpoint_publishes_and_fetches_a_resource() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["resource"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(1), LossModel::new(2));

    let payload: Vec<u8> = (0..12_000_u32)
        .map(|n| n.wrapping_mul(31).wrapping_add(7) as u8)
        .collect();
    let expected = payload.clone();
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            assert_eq!(accepted.destination, destination);
            accepted.session.receive().await.unwrap()
        }
    });

    let sent = tokio::time::timeout(
        Duration::from_secs(10),
        client.send_payload_with_config(
            destination,
            *server_id.public(),
            &payload,
            ResourceTransferConfig {
                timeout: Duration::from_secs(5),
                retry_interval: Duration::from_millis(100),
                request_window: 1,
            },
        ),
    )
    .await
    .expect("publish completes")
    .expect("receiver proves the resource");
    assert_eq!(sent, PayloadMode::Resource);

    let fetched = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("receiver completes")
        .unwrap();
    assert_eq!(fetched, ReceivedPayload::Resource(expected));
}

#[tokio::test]
async fn endpoint_publish_survives_a_lost_completion_proof() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x24; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x13; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["resource-proof-replay"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    let proof_dropped = connect_dropping_first_resource_proof(&client, &server);

    let payload: Vec<u8> = (0..2_000_u32)
        .map(|n| n.wrapping_mul(29).wrapping_add(5) as u8)
        .collect();
    let expected = payload.clone();
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.receive().await.unwrap()
        }
    });

    let sent = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &payload,
            ResourceTransferConfig {
                timeout: Duration::from_secs(2),
                retry_interval: Duration::from_millis(20),
                request_window: 1,
            },
        )
        .await
        .expect("a replayed completion proof reaches the publisher");

    assert_eq!(sent, PayloadMode::Resource);
    assert!(
        proof_dropped.load(Ordering::Acquire),
        "the test must remove the receiver's first completion proof"
    );
    assert_eq!(receiver.await.unwrap(), ReceivedPayload::Resource(expected));
}

#[tokio::test]
async fn resource_registration_also_receives_best_effort_data() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x66; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["mixed"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(5), LossModel::new(6));

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            assert_eq!(accepted.destination, destination);
            accepted.session.receive().await.unwrap()
        }
    });

    let mode = client
        .send_payload(destination, *server_id.public(), b"small message")
        .await
        .unwrap();
    assert_eq!(mode, PayloadMode::Data);

    let received = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("receiver completes")
        .unwrap();
    assert_eq!(received, ReceivedPayload::Data(b"small message".to_vec()));
}

#[tokio::test]
async fn resource_session_carries_a_matching_request_and_response() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x76; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x75; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["request"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(75), LossModel::new(76));

    let responder = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let received = accepted.session.receive_request().await.unwrap();
            assert_eq!(received.request.data, b"ping");
            accepted
                .session
                .respond(received.request_id, b"pong".to_vec());
        }
    });
    let request = Request::new(b"/echo", b"ping".to_vec(), 1_753_603_206.5);
    let response = tokio::time::timeout(
        Duration::from_secs(5),
        client.request(destination, *server_id.public(), &request),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.data, b"pong");
    tokio::time::timeout(Duration::from_secs(5), responder)
        .await
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_large_response_degrades_to_a_matching_resource() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x78; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x77; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["large-response"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(77), LossModel::new(78));

    let payload: Vec<u8> = (0..4_096_u32)
        .map(|value| value.wrapping_mul(73).wrapping_add(19) as u8)
        .collect();
    let expected = payload.clone();
    let responder = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let received = accepted.session.receive_request().await.unwrap();
            accepted
                .session
                .respond_auto(received.request_id, payload)
                .await
                .unwrap()
        }
    });
    let request = Request::new(b"/large", Vec::new(), 1_753_603_207.5);
    let response = tokio::time::timeout(
        Duration::from_secs(10),
        client.request(destination, *server_id.public(), &request),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response.data, expected);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), responder)
            .await
            .unwrap()
            .unwrap(),
        PayloadMode::Resource
    );
}

#[tokio::test]
async fn endpoint_fetches_a_resource_published_by_peer() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x44; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["resource-fetch"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(3), LossModel::new(4));

    let payload: Vec<u8> = (0..12_000_u32)
        .map(|n| n.wrapping_mul(17).wrapping_add(11) as u8)
        .collect();
    let expected = payload.clone();
    let publisher = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            assert_eq!(accepted.destination, destination);
            accepted.session.publish(&payload).await.unwrap();
        }
    });

    let fetched = tokio::time::timeout(
        Duration::from_secs(10),
        client.fetch_resource(destination, *server_id.public()),
    )
    .await
    .expect("fetch completes")
    .expect("published resource verifies");

    tokio::time::timeout(Duration::from_secs(5), publisher)
        .await
        .expect("publisher sees the receipt")
        .unwrap();
    assert_eq!(fetched, expected);
}
