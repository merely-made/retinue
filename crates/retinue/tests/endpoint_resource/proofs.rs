//! Completion proofs: lost, recovered by cache request, and never arriving.

use super::*;

/// A receiver that drops its session as soon as it has the data takes the link, and the
/// proof it kept, with it. The copies of its proof queued at completion still reach a
/// publisher that lost the first, as LXMF direct delivery needs.
#[tokio::test]
async fn endpoint_publish_survives_a_lost_completion_proof() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x24; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x13; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["resource-proof-replay"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    let (proofs_dropped, _) = connect_dropping_resource_proofs(&client, &server, false);

    let payload: Vec<u8> = (0..2_000_u32)
        .map(|n| n.wrapping_mul(29).wrapping_add(5) as u8)
        .collect();
    let expected = payload.clone();
    // The session, and with it the link, is dropped the moment the data is in hand.
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
        .expect("a queued copy of the completion proof reaches the publisher");

    assert_eq!(sent, PayloadMode::Resource);
    assert_eq!(
        proofs_dropped.load(Ordering::Acquire),
        1,
        "the test must remove the receiver's first completion proof"
    );
    assert_eq!(receiver.await.unwrap(), ReceivedPayload::Resource(expected));
}

/// A publisher that hears none of the proofs sent at completion asks for one with a cache
/// request, after RNS's proof wait, and the receiver, still holding the link, answers from
/// the proof it kept.
#[tokio::test(start_paused = true)]
async fn endpoint_publish_recovers_a_lost_proof_with_a_cache_request() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x25; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x14; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["resource-proof-cache"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    let (proofs_dropped, cache_requests) = connect_dropping_resource_proofs(&client, &server, true);

    let payload: Vec<u8> = (0..2_000_u32)
        .map(|n| n.wrapping_mul(31).wrapping_add(7) as u8)
        .collect();
    let expected = payload.clone();
    // The receiver keeps its session (and so the link and its kept proof) until the
    // publisher is done: a proof can only be asked for again while the link lives.
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let payload = accepted.session.receive().await.unwrap();
            (payload, accepted)
        }
    });

    let sent = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &payload,
            ResourceTransferConfig {
                timeout: Duration::from_secs(30),
                retry_interval: Duration::from_millis(20),
                request_window: 1,
            },
        )
        .await
        .expect("the publisher's cache request recovers the lost completion proof");

    assert_eq!(sent, PayloadMode::Resource);
    assert!(
        proofs_dropped.load(Ordering::Acquire) >= 3,
        "every proof sent at completion was removed"
    );
    assert!(
        cache_requests.load(Ordering::Acquire) > 0,
        "the publisher asked for its proof with a cache request"
    );
    let (received, _session) = receiver.await.unwrap();
    assert_eq!(received, ReceivedPayload::Resource(expected));
}

/// The receiver's completion proof crosses the wire as a PROOF-type packet, the only form
/// RNS accepts, and the publishing Endpoint completes on it.
#[tokio::test]
async fn endpoint_resource_proof_is_proof_typed_and_completes_the_publisher() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x26; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x15; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["resource-proof-type"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");

    let (mut a_out, a_sink) = client.attach_interface().split();
    let (mut b_out, b_sink) = server.attach_interface().split();
    tokio::spawn(async move {
        while let Some(packet) = a_out.recv().await {
            if !b_sink.deliver(packet) {
                break;
            }
        }
    });
    let proof_types = Arc::new(std::sync::Mutex::new(Vec::new()));
    tokio::spawn({
        let proof_types = Arc::clone(&proof_types);
        async move {
            while let Some(packet) = b_out.recv().await {
                if packet.context == CTX_RESOURCE_PRF {
                    proof_types.lock().unwrap().push(packet.packet_type);
                }
                if !a_sink.deliver(packet) {
                    break;
                }
            }
        }
    });

    let payload: Vec<u8> = (0..3_000_u32).map(|n| n.wrapping_mul(17) as u8).collect();
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
                timeout: Duration::from_secs(5),
                retry_interval: Duration::from_millis(100),
                request_window: 4,
            },
        )
        .await
        .expect("the publisher completes on the PROOF-type receipt");
    assert_eq!(sent, PayloadMode::Resource);
    assert_eq!(receiver.await.unwrap(), ReceivedPayload::Resource(payload));

    let proof_types = proof_types.lock().unwrap();
    assert!(!proof_types.is_empty());
    assert!(proof_types.iter().all(|t| *t == PacketType::Proof));
}

/// A publisher that never hears a proof asks for it three times, then cancels and fails,
/// as RNS's sender does, rather than waiting out its whole timeout. The receiver answers
/// each request, and no more than its cap however often it is asked.
#[tokio::test(start_paused = true)]
async fn a_publish_whose_proof_never_arrives_gives_up_after_its_cache_requests() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x37; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x38; 64]));
    let name = DestinationName::new("retinue", ["resource-proof-never"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    let proofs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&proofs);
    // No proof ever reaches the publisher.
    connect_filtered(
        &client,
        &server,
        |_| true,
        move |packet| {
            if packet.context == CTX_RESOURCE_PRF {
                counted.fetch_add(1, Ordering::AcqRel);
                return false;
            }
            true
        },
    );

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let payload = accepted.session.receive().await.unwrap();
            (payload, accepted)
        }
    });
    let started = tokio::time::Instant::now();
    let error = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &incompressible(2_000),
            quick(Duration::from_secs(120)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    // Four proof waits of RTT × 3 + 10 s each.
    assert!(
        started.elapsed() < Duration::from_secs(60),
        "{:?}",
        started.elapsed()
    );
    let (_, _session) = receiver.await.unwrap();
    // Three sent at completion, then one answer to each of three cache requests.
    assert_eq!(proofs.load(Ordering::Acquire), 3 + 3);
}

/// LXMF proves direct link data before parsing it (`LXMRouter.py` 1995-1996): the session
/// proves the data packet its last receive returned, signed over that packet's hash.
#[tokio::test]
async fn a_session_proves_the_data_packet_it_received() {
    use std::sync::Mutex;

    let server_id = PrivateIdentity::from_secret_bytes(&[0x2a; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x1a; 64]));
    let name = DestinationName::new("retinue", ["data-proof"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");

    let sent = Arc::new(Mutex::new(Vec::new()));
    let proved = Arc::new(Mutex::new(Vec::new()));
    let (sent_log, proof_log) = (Arc::clone(&sent), Arc::clone(&proved));
    connect_filtered(
        &client,
        &server,
        move |packet| {
            if packet.packet_type == PacketType::Data && packet.context == 0 {
                sent_log.lock().unwrap().push(packet.full_hash());
            }
            true
        },
        move |packet| {
            if packet.packet_type == PacketType::Proof && packet.context == 0 {
                proof_log
                    .lock()
                    .unwrap()
                    .push(packet.payload[..32].to_vec());
            }
            true
        },
    );

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            assert!(accepted.session.prove_data().is_err());
            let received = accepted.session.receive().await.unwrap();
            accepted.session.prove_data().unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            received
        }
    });
    client
        .send_payload(destination, *server_id.public(), b"prove me")
        .await
        .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), receiver)
        .await
        .expect("receiver completes")
        .unwrap();
    assert_eq!(received, ReceivedPayload::Data(b"prove me".to_vec()));
    let proved = proved.lock().unwrap();
    assert_eq!(proved.len(), 1);
    assert!(sent.lock().unwrap().iter().any(|hash| proved[0] == hash));
}
