//! The reliable stream end to end: two endpoints over an in-memory interface, one
//! `open_reliable`, the other `accept_reliable`, exchanging a multi-packet request and
//! response with half-close and eof.
//!
//! The loss tolerance of the machinery — retransmit, reorder, proof-based acks — is proven
//! deterministically in `reliable`'s sans-io tests on a virtual clock. This test proves the
//! *endpoint wiring*: the router dispatching channel-data and proof packets to the driver
//! task, the driver proving receipts and releasing acked sequences, ordered bytes reaching
//! the app, and half-close teardown (the client finishes sending, then reads the reply).

use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};

#[tokio::test]
async fn reliable_request_response_end_to_end() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
    // The endpoint owns its tasks and tears them down on drop, so the server must outlive the
    // client's read. Hold it in an Arc kept alive in this scope past the exchange.
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id.clone());

    let name = DestinationName::new("retinue", ["reliable"]);
    let dest = name.destination_hash(server_id.public());
    server.register_reliable(name, b"");

    // A clean in-memory interface between the two endpoints.
    connect(&client, &server, LossModel::new(1), LossModel::new(2));
    // (`server` is an Arc; `connect` and the endpoint methods deref through it.)

    // Server: accept one reliable link (it learns the client's identity from the client's
    // IDENTIFY), read the whole request, reply with its length, and finish.
    let server_task = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut stream = server.accept_reliable().await.unwrap();
            let mut req = Vec::new();
            stream.read_to_end(&mut req).await.unwrap();
            let mut resp = b"got ".to_vec();
            resp.extend_from_slice(&(req.len() as u32).to_le_bytes());
            stream.write_all(&resp).await.unwrap();
            stream.shutdown().await.unwrap();
            req
        }
    });

    // Client: open the reliable link, send a multi-packet payload, half-close, read the reply.
    let server_pub = *server_id.public();
    let mut stream = tokio::time::timeout(
        Duration::from_secs(10),
        client.open_reliable(dest, server_pub),
    )
    .await
    .expect("link opens within timeout")
    .expect("reliable stream");

    let payload: Vec<u8> = (0..3000u32)
        .map(|i| (i.wrapping_mul(7).wrapping_add(3)) as u8)
        .collect();
    stream.write_all(&payload).await.unwrap();
    stream.shutdown().await.unwrap(); // half-close: done sending, still reading

    let mut resp = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), stream.read_to_end(&mut resp))
        .await
        .expect("response within timeout")
        .unwrap();

    let got_req = tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server finished")
        .unwrap();
    assert_eq!(
        got_req, payload,
        "server received the exact multi-packet request"
    );

    let mut expected = b"got ".to_vec();
    expected.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    assert_eq!(resp, expected, "client received the exact response");
}

/// The same bidirectional exchange over a *lossy* interface. The response direction is the
/// one that depends on the client's IDENTIFY reaching the server (so the server can validate
/// the client's proofs of the response data). If IDENTIFY were sent once and dropped, the
/// server could never release its response and this would time out; the bounded IDENTIFY
/// re-send makes it survive. This also exercises endpoint-level retransmission end to end.
#[tokio::test]
async fn reliable_exchange_survives_loss_including_identify() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x24; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id.clone());

    let name = DestinationName::new("retinue", ["reliable-lossy"]);
    let dest = name.destination_hash(server_id.public());
    server.register_reliable(name, b"");

    // Drop ~20% of packets each way, so some early packets — potentially the IDENTIFY —
    // are lost and must be recovered by re-send. Link setup packets are not lossy (they
    // predate the interface); this stresses the data + identify path.
    connect(
        &client,
        &server,
        LossModel::new(11).drop_per_mille(200),
        LossModel::new(29).drop_per_mille(200),
    );

    let server_task = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut stream = server.accept_reliable().await.unwrap();
            let mut req = Vec::new();
            stream.read_to_end(&mut req).await.unwrap();
            let mut resp = b"got ".to_vec();
            resp.extend_from_slice(&(req.len() as u32).to_le_bytes());
            stream.write_all(&resp).await.unwrap();
            stream.shutdown().await.unwrap();
            req
        }
    });

    let server_pub = *server_id.public();
    let mut stream = tokio::time::timeout(
        Duration::from_secs(20),
        client.open_reliable(dest, server_pub),
    )
    .await
    .expect("link opens within timeout")
    .expect("reliable stream");

    let payload: Vec<u8> = (0..3000u32)
        .map(|i| (i.wrapping_mul(13).wrapping_add(1)) as u8)
        .collect();
    stream.write_all(&payload).await.unwrap();
    stream.shutdown().await.unwrap();

    let mut resp = Vec::new();
    tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut resp))
        .await
        .expect("response within timeout despite loss")
        .unwrap();

    let got_req = tokio::time::timeout(Duration::from_secs(10), server_task)
        .await
        .expect("server finished")
        .unwrap();
    assert_eq!(
        got_req, payload,
        "server received the exact request over loss"
    );

    let mut expected = b"got ".to_vec();
    expected.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    assert_eq!(
        resp, expected,
        "client received the exact response over loss"
    );
}

/// A peer that vanishes mid-stream fails the stream and closes the link (review #15).
///
/// RNS gives each channel packet five tries, then tears the link down. Before, the driver
/// retransmitted forever and the reader waited forever. Here both directions are cut after a
/// first exchange, the client writes again, and its reader must end with `TimedOut` while
/// the driver sends a link close and forgets the link.
#[tokio::test]
async fn a_vanished_peer_fails_the_stream_and_closes_the_link() {
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use retinue::link::CTX_LINKCLOSE;

    let server_id = PrivateIdentity::from_secret_bytes(&[0x52; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x25; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);

    let name = DestinationName::new("retinue", ["reliable-vanish"]);
    let dest = name.destination_hash(server_id.public());
    server.register_reliable(name, b"");

    // Hand-rolled pumps, so the wire can be cut and the client's last packets inspected.
    let alive = Arc::new(AtomicBool::new(true));
    let closes_after_cut = Arc::new(AtomicU32::new(0));
    let (mut client_out, client_sink) = client.attach_interface().split();
    let (mut server_out, server_sink) = server.attach_interface().split();
    tokio::spawn({
        let alive = Arc::clone(&alive);
        let closes = Arc::clone(&closes_after_cut);
        async move {
            while let Some(pkt) = client_out.recv().await {
                if alive.load(Ordering::SeqCst) {
                    server_sink.deliver(pkt);
                } else if pkt.context == CTX_LINKCLOSE {
                    closes.fetch_add(1, Ordering::SeqCst);
                }
            }
        }
    });
    tokio::spawn({
        let alive = Arc::clone(&alive);
        async move {
            while let Some(pkt) = server_out.recv().await {
                if alive.load(Ordering::SeqCst) {
                    client_sink.deliver(pkt);
                }
            }
        }
    });

    let server_task = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut stream = server.accept_reliable().await.unwrap();
            let mut first = [0u8; 5];
            stream.read_exact(&mut first).await.unwrap();
            // Keep the stream open; the wire is about to be cut under it.
            (stream, first)
        }
    });

    let mut stream = tokio::time::timeout(
        Duration::from_secs(10),
        client.open_reliable(dest, *server_id.public()),
    )
    .await
    .expect("link opens within timeout")
    .expect("reliable stream");
    let link_id = stream.link_id();
    stream.write_all(b"hello").await.unwrap();
    let (_server_stream, first) = tokio::time::timeout(Duration::from_secs(10), server_task)
        .await
        .expect("server read the first bytes")
        .unwrap();
    assert_eq!(&first, b"hello");
    // Let the proofs of the first bytes land before the cut.
    tokio::time::sleep(Duration::from_millis(300)).await;

    alive.store(false, Ordering::SeqCst);
    stream.write_all(b"into the void").await.unwrap();

    let mut sink = Vec::new();
    let error = tokio::time::timeout(Duration::from_secs(30), stream.read_to_end(&mut sink))
        .await
        .expect("the stream must fail rather than wait forever")
        .expect_err("a vanished peer is an error, not a clean end of stream");
    assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
    assert!(
        closes_after_cut.load(Ordering::SeqCst) >= 1,
        "the driver sent a link close"
    );
    assert!(
        client.link_facts().iter().all(|fact| fact.id != link_id),
        "and forgot the link"
    );
}
