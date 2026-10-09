//! Rejection, timeouts, and size caps cancel the other side promptly.

use super::*;

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
