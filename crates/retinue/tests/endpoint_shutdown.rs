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

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
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
