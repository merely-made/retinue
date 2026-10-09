//! Multi-segment Resources and requests sent as Resources, through the Endpoint API.

use super::*;
use retinue::resource::MAX_SEGMENT_SIZE;

/// A Resource past one segment goes as two, each proved before the next is advertised.
/// The first segment's proof is dropped, so the publisher recovers it from the kept proof
/// with a cache request before the second segment can begin.
#[tokio::test]
async fn a_two_segment_resource_survives_a_lost_segment_proof() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x81; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x82; 64]));
    let name = DestinationName::new("retinue", ["segments"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    let (dropped, cache_requests) = connect_dropping_resource_proofs(&client, &server, false);

    let payload = incompressible(MAX_SEGMENT_SIZE + 20_000);
    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.set_config(quick(Duration::from_secs(120)));
            let received = accepted.session.receive().await.unwrap();
            (received, accepted)
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    let mut config = quick(Duration::from_secs(120));
    config.request_window = 75;
    session.set_config(config);
    session.publish(&payload).await.unwrap();
    let (received, _accepted) = receiver.await.unwrap();
    assert_eq!(received, ReceivedPayload::Resource(payload));
    assert_eq!(dropped.load(Ordering::Acquire), 1);
    assert!(cache_requests.load(Ordering::Acquire) >= 1);
}

/// A total past the receiver's cap is refused at the first segment's advertisement.
#[tokio::test]
async fn a_resource_past_the_size_cap_is_refused() {
    let server_id = PrivateIdentity::from_secret_bytes(&[0x83; 64]);
    let server = Arc::new(Endpoint::new(server_id.clone()));
    let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x84; 64]));
    let name = DestinationName::new("retinue", ["segments-cap"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name, b"");
    connect(&client, &server, LossModel::new(83), LossModel::new(84));

    let receiver = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.set_config(quick(Duration::from_secs(20)));
            accepted.session.set_max_resource_size(10_000);
            accepted.session.receive().await.unwrap_err().kind()
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    session.set_config(quick(Duration::from_secs(20)));
    let error = session.publish(&incompressible(20_000)).await.unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionAborted);
    assert_eq!(receiver.await.unwrap(), std::io::ErrorKind::InvalidData);
}

/// Requests and responses past one packet travel as Resources, the request named by its
/// packed form's hash; one past the responder's `max_request_size` is rejected and the
/// responder goes on to the next.
#[tokio::test]
async fn large_requests_and_responses_travel_as_resources() {
    tokio::time::timeout(Duration::from_secs(60), async {
        let server_id = PrivateIdentity::from_secret_bytes(&[0x85; 64]);
        let server = Endpoint::new(server_id.clone());
        let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x86; 64]));
        let name = DestinationName::new("retinue", ["big-request"]);
        let destination = name.destination_hash(server_id.public());
        server.register_resource(name, b"");
        connect(&client, &server, LossModel::new(85), LossModel::new(86));

        let big = Request::new(b"/big", incompressible(2048), 0.0);
        let expected_id = retinue::hash::AddressHash::of(&big.pack());
        let responder = tokio::spawn(async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.set_config(quick(Duration::from_secs(30)));
            accepted.session.set_max_request_size(Some(4096));
            // The oversized request is rejected unseen; the next one is served.
            let request = accepted.session.receive_request().await.unwrap();
            assert_eq!(request.request_id, expected_id);
            let echo = [request.request.data.as_slice(); 2].concat();
            let mode = accepted
                .session
                .respond_auto(request.request_id, echo)
                .await
                .unwrap();
            assert_eq!(mode, PayloadMode::Resource);
            accepted
        });
        let mut session = client
            .open_resource(destination, *server_id.public())
            .await
            .unwrap();
        session.set_config(quick(Duration::from_secs(30)));
        let oversized = Request::new(b"/too-big", incompressible(8192), 0.0).pack();
        assert_eq!(
            session.request_raw(&oversized).await.unwrap_err().kind(),
            std::io::ErrorKind::ConnectionAborted
        );
        let response = session.request(&big).await.unwrap();
        assert_eq!(response.request_id, expected_id);
        assert_eq!(response.data, [big.data.as_slice(); 2].concat());
        drop(responder.await.unwrap());
    })
    .await
    .expect("the exchange completes");
}
