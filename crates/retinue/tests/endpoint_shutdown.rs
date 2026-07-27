//! Orderly shutdown: work already queued for an interface reaches the wire.
//!
//! `close`/`Drop` are abrupt by design — they abort every tracked task,
//! including the interface writers — and that is invisible from the caller's
//! side, because `AsyncWrite::flush` on a link stream returns once the bytes
//! reach the relay's duplex, long before they are framed, queued, and written.
//! A server that wrote a reply and then let its endpoint fall out of scope
//! could therefore lose the reply with every call having returned `Ok`.
//!
//! This pins the orderly path: drop the stream so the relay drains, then await
//! `shutdown`, and the peer gets everything.

use std::sync::Arc;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const REPLY: &[u8] = b"the-reply-that-must-not-be-lost";

/// Wait until `ep` can resolve `dest`, pumping announcements.
async fn await_resolve(ep: &Endpoint, dest: retinue::hash::AddressHash) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while ep.resolve(dest).is_none() && tokio::time::Instant::now() < deadline {
        let _ = tokio::time::timeout(Duration::from_millis(300), ep.next_announcement()).await;
    }
    assert!(ep.resolve(dest).is_some(), "peer should learn the dest");
}

#[tokio::test]
async fn a_reply_written_before_an_orderly_shutdown_reaches_the_peer() {
    let server_id = PrivateIdentity::from_secret_bytes(&[9u8; 64]);
    let server = Endpoint::new(server_id.clone());
    let addr = server
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let name = DestinationName::new("shutdown", ["svc"]);
    let dest = name.destination_hash(server_id.public());
    server.register(name.clone(), b"svc");

    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[2u8; 64]));
    client.attach_tcp_client(addr).await.unwrap();

    for _ in 0..4 {
        server.announce(&name, b"svc");
        tokio::time::sleep(Duration::from_millis(120)).await;
    }
    await_resolve(&client, dest).await;

    let mut client_stream = client.open(dest, *server_id.public()).await.unwrap();
    let mut accepted = tokio::time::timeout(Duration::from_secs(5), server.accept_on_any())
        .await
        .expect("accept should not time out")
        .unwrap();

    // The server answers and then tears down, in the order a service would:
    // write, drop the stream so the relay drains its duplex, then shut the
    // endpoint down with a grace window.
    accepted.stream.write_all(REPLY).await.unwrap();
    accepted.stream.flush().await.unwrap();
    drop(accepted);
    server.shutdown(Duration::from_secs(2)).await;

    let mut got = vec![0u8; REPLY.len()];
    tokio::time::timeout(Duration::from_secs(5), client_stream.read_exact(&mut got))
        .await
        .expect("the reply should arrive despite the shutdown")
        .expect("read the reply");
    assert_eq!(got, REPLY);
}

/// `shutdown` must not hang when there is nothing left to flush, and must
/// still stop the endpoint.
#[tokio::test]
async fn shutdown_returns_promptly_on_an_idle_endpoint() {
    let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[5u8; 64]));
    endpoint
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();

    let started = tokio::time::Instant::now();
    endpoint.shutdown(Duration::from_secs(10)).await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "an idle endpoint should not wait out its grace window"
    );

    // And it really stopped: a second call is harmless, like `close`.
    endpoint.shutdown(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn a_reliable_reply_drains_before_orderly_shutdown() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x39; 64]);
    let client_id = PrivateIdentity::from_secret_bytes(&[0x17; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(client_id);
    let name = DestinationName::new("shutdown", ["reliable"]);
    let dest = name.destination_hash(server_id.public());
    server.register_reliable(name, b"svc");
    connect(&client, &server, LossModel::new(31), LossModel::new(73));

    let server_task = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_reliable().await.unwrap();
            let mut request = Vec::new();
            accepted.read_to_end(&mut request).await.unwrap();
            assert_eq!(request, b"request");

            accepted.write_all(REPLY).await.unwrap();
            accepted.shutdown().await.unwrap();
            drop(accepted);
            server.shutdown(Duration::from_secs(5)).await;
        }
    });

    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        client.open_reliable(dest, *server_id.public()),
    )
    .await
    .expect("reliable link should open")
    .expect("reliable stream");
    stream.write_all(b"request").await.unwrap();
    stream.shutdown().await.unwrap();

    let mut got = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stream.read_to_end(&mut got))
        .await
        .expect("reply should survive reliable endpoint shutdown")
        .expect("read reliable reply");
    assert_eq!(got, REPLY);
    tokio::time::timeout(Duration::from_secs(5), server_task)
        .await
        .expect("server shutdown should complete")
        .expect("server task");
}

#[tokio::test]
async fn shutdown_waits_for_a_packet_held_by_the_interface_pump() {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x51; 64],
    )));
    let mut interface = endpoint.attach_interface();
    let name = DestinationName::new("shutdown", ["in-flight"]);
    endpoint.register(name, b"svc");

    let packet = tokio::time::timeout(Duration::from_secs(1), interface.next_outbound())
        .await
        .expect("announce should be queued")
        .expect("interface should be open");
    assert_eq!(packet.packet_type, retinue::packet::PacketType::Announce);

    let shutdown = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move {
            endpoint.shutdown(Duration::from_secs(2)).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        !shutdown.is_finished(),
        "dequeue alone must not count as wire delivery"
    );

    // Dropping the pump reports that its held packet can make no further progress.
    // The bounded shutdown may now finish instead of mistaking dequeue for delivery.
    drop(interface);
    tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("shutdown should finish once the pump releases its packet")
        .expect("shutdown task");
}

#[tokio::test]
async fn shutdown_waits_for_an_active_resource_session() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x72; 64]));
    let name = DestinationName::new("shutdown", ["resource"]);
    let dest = name.destination_hash(server_id.public());
    server.register_resource(name, b"svc");
    connect(&client, &server, LossModel::new(81), LossModel::new(82));

    let client_session = tokio::time::timeout(
        Duration::from_secs(5),
        client.open_resource(dest, *server_id.public()),
    )
    .await
    .expect("resource link should open")
    .expect("client resource session");
    let accepted = tokio::time::timeout(Duration::from_secs(5), server.accept_resource())
        .await
        .expect("resource accept should finish")
        .expect("server resource session");

    let shutdown = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            server.shutdown(Duration::from_secs(2)).await;
        }
    });
    tokio::time::sleep(Duration::from_millis(40)).await;
    assert!(
        !shutdown.is_finished(),
        "an active resource session must hold orderly shutdown open"
    );

    drop(accepted);
    tokio::time::timeout(Duration::from_secs(1), shutdown)
        .await
        .expect("shutdown should finish after the resource session drops")
        .expect("shutdown task");
    drop(client_session);
}

#[tokio::test]
async fn an_accept_waiter_is_released_when_the_endpoint_closes() {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x61; 64],
    )));
    let waiter = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move { endpoint.accept_on_any().await }
    });
    tokio::task::yield_now().await;

    endpoint.shutdown(Duration::from_millis(100)).await;
    let result = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("accept waiter should wake")
        .expect("accept task");
    let error = match result {
        Ok(_) => panic!("closed endpoint should not accept"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn a_pending_open_is_released_when_the_endpoint_closes() {
    let endpoint = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x62; 64],
    )));
    let peer = PrivateIdentity::from_secret_bytes(&[0x63; 64]);
    let dest = DestinationName::new("shutdown", ["pending"]).destination_hash(peer.public());
    let waiter = tokio::spawn({
        let endpoint = Arc::clone(&endpoint);
        async move { endpoint.open(dest, *peer.public()).await }
    });
    tokio::task::yield_now().await;

    endpoint.close();
    let result = tokio::time::timeout(Duration::from_secs(1), waiter)
        .await
        .expect("pending open should wake")
        .expect("open task");
    let error = match result {
        Ok(_) => panic!("closed endpoint should not establish a link"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
}

#[tokio::test]
async fn abrupt_close_releases_an_active_resource_operation() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x73; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x74; 64]));
    let name = DestinationName::new("shutdown", ["resource-close"]);
    let dest = name.destination_hash(server_id.public());
    server.register_resource(name, b"svc");
    connect(&client, &server, LossModel::new(83), LossModel::new(84));

    let client_session = tokio::time::timeout(
        Duration::from_secs(5),
        client.open_resource(dest, *server_id.public()),
    )
    .await
    .expect("resource link should open")
    .expect("client resource session");
    let accepted = tokio::time::timeout(Duration::from_secs(5), server.accept_resource())
        .await
        .expect("resource accept should finish")
        .expect("server resource session");
    let operation = tokio::spawn(async move {
        let mut session = accepted.session;
        session.fetch().await
    });
    tokio::task::yield_now().await;

    server.close();
    let result = tokio::time::timeout(Duration::from_secs(1), operation)
        .await
        .expect("resource operation should wake")
        .expect("resource task");
    let error = result.expect_err("closed endpoint should break the resource operation");
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    drop(client_session);
}
