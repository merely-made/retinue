//! Endpoint-level resource publish/fetch over the raw interface seam.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, PayloadMode, ReceivedPayload, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::link::{CTX_CACHE_REQUEST, CTX_RESOURCE, CTX_RESOURCE_PRF, CTX_RESOURCE_REQ};
use retinue::lossy::{LossModel, connect};
use retinue::packet::{Packet, PacketType};
use retinue::request::Request;

/// Connect `a` to `b`, dropping `b`'s resource proofs and counting `a`'s cache requests.
/// Only the first proof is dropped, or with `until_asked` every proof until `a` has sent a
/// cache request. Returns the count of proofs dropped and of cache requests.
fn connect_dropping_resource_proofs(
    a: &Endpoint,
    b: &Endpoint,
    until_asked: bool,
) -> (Arc<AtomicUsize>, Arc<AtomicUsize>) {
    let (mut a_out, a_sink) = a.attach_interface().split();
    let (mut b_out, b_sink) = b.attach_interface().split();
    let dropped = Arc::new(AtomicUsize::new(0));
    let cache_requests = Arc::new(AtomicUsize::new(0));

    let requests = Arc::clone(&cache_requests);
    tokio::spawn(async move {
        while let Some(packet) = a_out.recv().await {
            if packet.context == CTX_CACHE_REQUEST {
                requests.fetch_add(1, Ordering::AcqRel);
            }
            if !b_sink.deliver(packet) {
                break;
            }
        }
    });

    let proofs_dropped = Arc::clone(&dropped);
    let asked = Arc::clone(&cache_requests);
    tokio::spawn(async move {
        while let Some(packet) = b_out.recv().await {
            if packet.context == CTX_RESOURCE_PRF {
                let drop = if until_asked {
                    asked.load(Ordering::Acquire) == 0
                } else {
                    proofs_dropped.load(Ordering::Acquire) == 0
                };
                if drop {
                    proofs_dropped.fetch_add(1, Ordering::AcqRel);
                    continue;
                }
            }
            if !a_sink.deliver(packet) {
                break;
            }
        }
    });

    (dropped, cache_requests)
}

/// Connect `a` to `b`, delivering only the packets `a_to_b` and `b_to_a` pass.
fn connect_filtered(
    a: &Endpoint,
    b: &Endpoint,
    a_to_b: impl Fn(&Packet) -> bool + Send + 'static,
    b_to_a: impl Fn(&Packet) -> bool + Send + 'static,
) {
    let (mut a_out, a_sink) = a.attach_interface().split();
    let (mut b_out, b_sink) = b.attach_interface().split();
    tokio::spawn(async move {
        while let Some(packet) = a_out.recv().await {
            if a_to_b(&packet) && !b_sink.deliver(packet) {
                break;
            }
        }
    });
    tokio::spawn(async move {
        while let Some(packet) = b_out.recv().await {
            if b_to_a(&packet) && !a_sink.deliver(packet) {
                break;
            }
        }
    });
}

#[tokio::test]
async fn oversized_request_is_refused_before_send_and_link_remains_usable() {
    tokio::time::timeout(Duration::from_secs(10), async {
        let server_id = PrivateIdentity::from_secret_bytes(&[0x75; 64]);
        let server = Endpoint::new(server_id.clone());
        let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x76; 64]));
        let name = DestinationName::new("retinue", ["request-cap"]);
        let destination = name.destination_hash(server_id.public());
        server.register_resource(name, b"");
        connect(&client, &server, LossModel::new(97), LossModel::new(98));
        let responder = tokio::spawn(async move {
            let mut accepted = server.accept_resource().await.unwrap();
            let request = accepted.session.receive_request().await.unwrap();
            // The oversized request must never precede this one at the handler.
            assert_eq!(
                request.request.path_hash,
                retinue::hash::AddressHash::of(b"/small")
            );
            accepted.session.respond(request.request_id, b"ok".to_vec());
        });
        let mut session = client
            .open_resource(destination, *server_id.public())
            .await
            .unwrap();
        let oversized = Request::new(b"/oversized", vec![0; 4096], 0.0).pack();
        assert_eq!(
            session.request_raw(&oversized).await.unwrap_err().kind(),
            std::io::ErrorKind::InvalidInput
        );
        let response = session
            .request(&Request::new(b"/small", Vec::new(), 0.0))
            .await
            .unwrap();
        assert_eq!(response.data, b"ok");
        responder.await.unwrap();
        client.close();
    })
    .await
    .expect("oversized request is refused without waiting for a timeout");
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
/// request, and the receiver, still holding the link, answers from the proof it kept.
#[tokio::test]
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
                timeout: Duration::from_secs(2),
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

fn quick(timeout: Duration) -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout,
        retry_interval: Duration::from_millis(50),
        request_window: 4,
    }
}

/// Distinct bytes bz2 cannot shrink below a few parts.
fn incompressible(len: usize) -> Vec<u8> {
    (0..len as u32)
        .flat_map(|n| retinue::hash::full_hash(&n.to_be_bytes()))
        .take(len)
        .collect()
}

/// A receiver whose accept hook refuses the offer rejects it on the wire, and the
/// publisher stops at once instead of running to its timeout.
#[tokio::test]
async fn a_rejected_offer_stops_the_publisher_promptly() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x32; 64]));
    let name = DestinationName::new("retinue", ["resource-reject"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(31), LossModel::new(32));

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted
                .session
                .set_accept(|advertisement| advertisement.data_size < 1_000);
            let error = accepted.session.receive().await.unwrap_err();
            (error.kind(), accepted)
        }
    });
    let started = std::time::Instant::now();
    let error = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &incompressible(5_000),
            quick(Duration::from_secs(20)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let (kind, _session) = receiver.await.unwrap();
    assert_eq!(kind, std::io::ErrorKind::PermissionDenied);
}

/// A receive that times out mid-transfer cancels it, and the publisher stops at once.
#[tokio::test]
async fn a_receive_timeout_cancels_the_publisher() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x34; 64]));
    let name = DestinationName::new("retinue", ["resource-receive-timeout"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    // Parts never reach the receiver.
    connect_filtered(
        &client,
        &server,
        |packet| packet.context != CTX_RESOURCE,
        |_| true,
    );

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted
                .session
                .set_config(quick(Duration::from_millis(500)));
            let error = accepted.session.receive().await.unwrap_err();
            (error.kind(), accepted)
        }
    });
    let started = std::time::Instant::now();
    let error = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &incompressible(5_000),
            quick(Duration::from_secs(20)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let (kind, _session) = receiver.await.unwrap();
    assert_eq!(kind, std::io::ErrorKind::TimedOut);
}

/// A publish that times out cancels the transfer, and the receiver stops at once.
#[tokio::test]
async fn a_publish_timeout_cancels_the_receiver() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x35; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x36; 64]));
    let name = DestinationName::new("retinue", ["resource-publish-timeout"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    // The receiver's part requests never reach the publisher.
    connect_filtered(
        &client,
        &server,
        |_| true,
        |packet| packet.context != CTX_RESOURCE_REQ,
    );

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.set_config(quick(Duration::from_secs(20)));
            let started = std::time::Instant::now();
            let error = accepted.session.receive().await.unwrap_err();
            (error.kind(), started.elapsed(), accepted)
        }
    });
    let error = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &incompressible(5_000),
            quick(Duration::from_millis(500)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    let (kind, elapsed, _session) = receiver.await.unwrap();
    assert_eq!(kind, std::io::ErrorKind::ConnectionAborted);
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
}

/// A request bounded by `max_response_size` rejects a larger response Resource before
/// any part moves; the responder's publish stops on the rejection.
#[tokio::test]
async fn a_response_past_max_response_size_is_rejected() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x37; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x38; 64]));
    let name = DestinationName::new("retinue", ["resource-response-cap"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(37), LossModel::new(38));

    let responder = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.set_config(quick(Duration::from_secs(20)));
            let request = accepted.session.receive_request().await.unwrap();
            let outcome = accepted
                .session
                .respond_auto(request.request_id, incompressible(4_000))
                .await;
            (outcome.map_err(|error| error.kind()), accepted)
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    session.set_config(quick(Duration::from_secs(20)));
    let started = std::time::Instant::now();
    let error = session
        .request_raw_with_limit(&Request::new(b"/big", Vec::new(), 0.0).pack(), Some(1_000))
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let (outcome, _session) = responder.await.unwrap();
    assert_eq!(outcome, Err(std::io::ErrorKind::ConnectionAborted));
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

/// A publisher that never hears a proof asks for it three times, then cancels and fails,
/// as RNS's sender does, rather than waiting out its whole timeout. The receiver answers
/// each request, and no more than its cap however often it is asked.
#[tokio::test]
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
    let started = std::time::Instant::now();
    let error = client
        .send_payload_with_config(
            destination,
            *server_id.public(),
            &incompressible(2_000),
            quick(Duration::from_secs(20)),
        )
        .await
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    let (_, _session) = receiver.await.unwrap();
    // Three sent at completion, then one answer to each of three cache requests.
    assert_eq!(proofs.load(Ordering::Acquire), 3 + 3);
}
