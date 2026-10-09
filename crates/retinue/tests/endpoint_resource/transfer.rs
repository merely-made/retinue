//! Publish, fetch, and request/response over a resource session.

use super::*;

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

/// Metadata published beside a Resource reaches the receiver separately from the data.
#[tokio::test]
async fn resource_metadata_reaches_the_receiver() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x39; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x3a; 64]));
    let name = DestinationName::new("retinue", ["resource-metadata"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(39), LossModel::new(40));

    // msgpack {"name": "notes.txt"}
    let metadata = b"\x81\xa4name\xa9notes.txt".to_vec();
    let payload = incompressible(6_000);
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let data = accepted.session.fetch().await.unwrap();
            (data, accepted.session.take_metadata(), accepted)
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    session.set_config(quick(Duration::from_secs(10)));
    session
        .publish_with_metadata(&payload, &metadata)
        .await
        .unwrap();
    let (data, received, _session) = receiver.await.unwrap();
    assert_eq!(data, payload);
    assert_eq!(received, Some(metadata));
}

/// A packet that arrives while `next_inbound` is receiving a Resource returns first, and
/// the Resource completes on the next call, as on one stock propagation link.
#[tokio::test]
async fn a_packet_mid_resource_leaves_the_transfer_running() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x78; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x77; 64]));
    let name = DestinationName::new("retinue", ["interleaved"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    // Hold the client's first data packet until the first Resource part has passed.
    let (mut client_out, client_sink) = client.attach_interface().split();
    let (mut server_out, server_sink) = server.attach_interface().split();
    tokio::spawn(async move {
        let mut held = None;
        while let Some(packet) = client_out.recv().await {
            let is_data = packet.destination_type == DestinationType::Link
                && packet.packet_type == PacketType::Data
                && packet.context == 0;
            if is_data && held.is_none() {
                held = Some(packet);
                continue;
            }
            let part = packet.context == CTX_RESOURCE;
            if !server_sink.deliver(packet) {
                break;
            }
            if part && let Some(data) = held.take() {
                server_sink.deliver(data);
            }
        }
    });
    tokio::spawn(async move {
        while let Some(packet) = server_out.recv().await {
            if !client_sink.deliver(packet) {
                break;
            }
        }
    });

    let payload = incompressible(20_000);
    let expected = payload.clone();
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut session = server.accept_resource().await.unwrap().session;
            let idle = Duration::from_secs(5);
            let first = session.next_inbound(idle).await.unwrap();
            let second = session.next_inbound(idle).await.unwrap();
            (first, second)
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    session.set_config(quick(Duration::from_secs(3)));
    session.send_data(b"ping");
    tokio::time::timeout(Duration::from_secs(10), session.publish(&payload))
        .await
        .expect("publish completes")
        .expect("receiver proves the resource");

    let (first, second) = receiver.await.unwrap();
    assert!(matches!(first, SessionInbound::Data(data) if data.data == b"ping"));
    assert_eq!(second, SessionInbound::Resource(expected));
}
