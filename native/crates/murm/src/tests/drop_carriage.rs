// SPDX-License-Identifier: MPL-2.0
//! Native loopback carriage fixture. Reticulum TCP is not radio qualification.

use super::*;
use retinue::{
    destination::DestinationName,
    endpoint::{Endpoint, ReceivedPayload, ResourceTransferConfig},
    identity::PrivateIdentity,
};
use std::time::Duration;
use transport::{BlobReadAuthorizer, BlobScope, BlobStore, P2pandaTransport};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_drop_has_one_identity_and_import_across_carriers() {
    carry_conversation_drop(1, 6_500).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn segmented_protected_drop_across_carriers() {
    carry_conversation_drop(2, 550_000).await;
}

/// A large synthetic conversation corpus, not a release snapshot.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "large loopback Resource gate"]
async fn synthetic_24_mib_protected_drop_across_carriers() {
    carry_conversation_drop(24, 1024 * 1024).await;
}

async fn carry_conversation_drop(count: usize, body_len: usize) {
    let key = CabalKey::new([0xa1; 32]);
    let source = make_murm();
    let cabal = source.open_cabal(&key).await.unwrap();
    let body = "x".repeat(body_len);
    for index in 0..count {
        cabal
            .send_text_at("session", &body, index as u64 + 1)
            .await
            .unwrap();
    }
    let store = source
        .conversation_engine()
        .sync_store(cabal.id().as_bytes())
        .unwrap();
    let records = export_topic_operations::<_, _, u64>(
        &store,
        &Topic::from(*cabal.id().as_bytes()),
        DropExportProfile::default(),
    )
    .await
    .unwrap();
    let mut bytes = Vec::new();
    let receipt = write_protected_drop(
        &mut bytes,
        &records,
        DropLimits::default(),
        &cabal.keyring().unwrap(),
    )
    .unwrap();
    if count == 24 {
        assert!(bytes.len() > 24 * 1024 * 1024);
    }
    if count == 2 {
        assert!(bytes.len() > retinue::resource::MAX_SEGMENT_SIZE);
    }

    let dir = tempdir().unwrap();
    let file = dir.path().join("drop");
    std::fs::write(&file, &bytes).unwrap();
    let from_file = std::fs::read(file).unwrap();

    let alice = InMemoryProvider::from_seed([0xa2; 32]);
    let bob = InMemoryProvider::from_seed([0xa3; 32]);
    let alice_id = PeerID::from_public_key(alice.master_public_key());
    let bob_id = PeerID::from_public_key(bob.master_public_key());
    let owner = BlobStore::new();
    let received = BlobStore::new();
    let hash = owner.put_bytes(bytes.clone()).await.unwrap();
    let scope = BlobScope::new(*cabal.id().as_bytes());
    let authority = BlobReadAuthorizer::new();
    authority.allow_reader(scope, bob_id.to_bytes());
    authority.retain(scope, hash);
    let provider = P2pandaTransport::builder(alice.master_keypair())
        .scoped_blobs(&owner, scope, authority)
        .bind()
        .await
        .unwrap();
    let client = P2pandaTransport::builder(bob.master_keypair())
        .bind()
        .await
        .unwrap();
    client
        .add_peer(provider.endpoint_addr().await.unwrap())
        .await
        .unwrap();
    received.fetch_from(&client, alice_id, hash).await.unwrap();
    let from_iroh = received.get_bytes(hash).await.unwrap().to_vec();
    client.close().await.unwrap();
    provider.close().await.unwrap();

    let server_id = PrivateIdentity::from_secret_bytes(&[0xa4; 64]);
    let server = Arc::new(Endpoint::with_network_stamp_policy(
        server_id.clone(),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    ));
    let client = Endpoint::with_network_stamp_policy(
        PrivateIdentity::from_secret_bytes(&[0xa5; 64]),
        retinue::network_stamp::NetworkStampPolicy::rns_1_6_0(),
    );
    let name = DestinationName::new("murm", ["drop-fixture"]);
    let destination = name.destination_hash(server_id.public());
    server.register_resource(name.clone(), b"");
    let addr = server
        .listen_tcp("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    client.attach_tcp_client(addr).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            server.announce(&name, b"");
            if let Ok(Ok(fact)) =
                tokio::time::timeout(Duration::from_millis(200), client.next_announcement()).await
                && fact.destination == destination
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    let receiver = tokio::spawn({
        let server = server.clone();
        async move {
            let mut accepted = server.accept_resource().await.unwrap();
            accepted.session.receive().await.unwrap()
        }
    });
    let mut session = client
        .open_resource(destination, *server_id.public())
        .await
        .unwrap();
    session.set_config(ResourceTransferConfig {
        timeout: Duration::from_secs(120),
        retry_interval: Duration::from_millis(50),
        request_window: 75,
    });
    tokio::time::timeout(Duration::from_secs(120), session.publish(&bytes))
        .await
        .unwrap()
        .unwrap();
    let ReceivedPayload::Resource(from_reticulum) = receiver.await.unwrap() else {
        panic!("expected Resource")
    };
    client.shutdown(Duration::from_secs(2)).await;
    server.shutdown(Duration::from_secs(2)).await;

    let mut reports = Vec::new();
    for carried in [from_file, from_iroh, from_reticulum] {
        let target = make_murm();
        let target = target.open_cabal(&key).await.unwrap();
        let (report, refresh) = target
            .import_protected_drop(Cursor::new(&carried), DropLimits::default())
            .await
            .unwrap();
        assert_eq!(report.drop_id, receipt.id);
        assert_eq!(refresh.added, count as u64);
        let (replay, refresh) = target
            .import_protected_drop(Cursor::new(&carried), DropLimits::default())
            .await
            .unwrap();
        assert!(replay.receipt_hit);
        assert_eq!(refresh.added, 0);
        reports.push(report);
    }
    assert_eq!(reports[0], reports[1]);
    assert_eq!(reports[0], reports[2]);
    let target = make_murm();
    let target = target.open_cabal(&key).await.unwrap();
    let mut corrupted = bytes;
    let last = corrupted.len() - 1;
    corrupted[last] ^= 1;
    assert!(
        target
            .import_protected_drop(Cursor::new(corrupted), DropLimits::default())
            .await
            .is_err()
    );
    assert!(target.history("session").is_empty());
}
