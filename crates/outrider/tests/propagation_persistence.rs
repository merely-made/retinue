use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use outrider::{
    DeliveryAnnounce, FetchPolicy, LxmfPayload, NodePolicy, PreparedPropagation,
    PropagationAnnounce, PropagationBatch, PropagationCosts, PropagationNode, PropagationStamps,
    PropagationStore, PropagationStoreLimits, StoreRestoreReceipt, delivery_destination,
    fetch_propagation, prepare_propagation_with, register_delivery, register_propagation,
    serve_fetch,
};
use retinue::endpoint::Endpoint;
use retinue::identity::{Identity, PrivateIdentity};
use retinue::lossy::{LossModel, connect};
use retinue::token::encrypt_to_identity;

struct SnapshotFile(PathBuf);

impl SnapshotFile {
    fn unique() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        Self(std::env::temp_dir().join(format!(
            "outrider-propagation-{}-{nonce}.snapshot",
            std::process::id()
        )))
    }

    fn replace(&self, bytes: &[u8]) {
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&self.0)
            .unwrap();
        file.write_all(bytes).unwrap();
        file.sync_all().unwrap();
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for SnapshotFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn sealed(
    sender: &PrivateIdentity,
    recipient: &Identity,
    payload: &LxmfPayload,
    key: u8,
) -> PreparedPropagation {
    let stamps = PropagationStamps {
        delivery_cost: None,
        propagation_cost: 0,
        seed: [0; 32],
        max_attempts: 1,
    };
    prepare_propagation_with(
        sender,
        delivery_destination(recipient),
        payload,
        &stamps,
        |plaintext| {
            let iv = [key.wrapping_add(0x10); 16];
            Ok((
                encrypt_to_identity(recipient, &[key; 32], &iv, plaintext),
                None,
            ))
        },
    )
    .unwrap()
}

/// One message per fetch. Retaining keeps this test on the node's explicit `haves` path.
fn policy(retain_on_node: bool) -> FetchPolicy {
    FetchPolicy {
        max_messages: 1,
        retain_on_node,
        max_entry_bytes: 2 * 1024,
        max_message_bytes: 2 * 1024,
        ..FetchPolicy::default()
    }
}

fn announce() -> PropagationAnnounce {
    PropagationAnnounce {
        legacy: false,
        unix_time: 1_753_603_200,
        active: true,
        transfer_limit_kb: 256.0,
        sync_limit_kb: 10_240.0,
        costs: PropagationCosts {
            propagation: 0,
            flexibility: 0,
            peering: 0,
        },
        metadata: Vec::new(),
    }
}

#[tokio::test]
async fn a_host_snapshot_survives_restart_and_preserves_owner_scoping_and_acknowledgement() {
    let node_identity = PrivateIdentity::from_secret_bytes(&[0x70; 64]);
    let first_recipient_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let second_recipient_identity = PrivateIdentity::from_secret_bytes(&[0x63; 64]);
    let node = Arc::new(Endpoint::new(node_identity.clone()));
    let first_recipient = Arc::new(Endpoint::new(first_recipient_identity.clone()));
    let second_recipient = Arc::new(Endpoint::new(second_recipient_identity.clone()));
    connect(
        &first_recipient,
        &node,
        LossModel::new(62),
        LossModel::new(70),
    );
    connect(
        &second_recipient,
        &node,
        LossModel::new(63),
        LossModel::new(71),
    );

    register_propagation(&node, &announce()).unwrap();
    let first_node_announce =
        tokio::time::timeout(Duration::from_secs(2), first_recipient.next_announcement())
            .await
            .unwrap()
            .unwrap();
    let second_node_announce =
        tokio::time::timeout(Duration::from_secs(2), second_recipient.next_announcement())
            .await
            .unwrap()
            .unwrap();
    register_delivery(&node, &DeliveryAnnounce::named(b"Persistent Source")).unwrap();
    for recipient in [&first_recipient, &second_recipient] {
        let source_announce =
            tokio::time::timeout(Duration::from_secs(2), recipient.next_announcement())
                .await
                .unwrap()
                .unwrap();
        assert_eq!(source_announce.identity, *node_identity.public());
    }
    register_delivery(
        &first_recipient,
        &DeliveryAnnounce::named(b"First Recipient"),
    )
    .unwrap();
    register_delivery(
        &second_recipient,
        &DeliveryAnnounce::named(b"Second Recipient"),
    )
    .unwrap();

    let first = sealed(
        &node_identity,
        first_recipient_identity.public(),
        &LxmfPayload::text(1_753_603_204.0, b"FIRST", b"for the first owner"),
        0x31,
    );
    let second = sealed(
        &node_identity,
        second_recipient_identity.public(),
        &LxmfPayload::text(1_753_603_205.0, b"SECOND", b"for the second owner"),
        0x32,
    );
    let limits = PropagationStoreLimits {
        max_entries: 4,
        max_bytes: 8 * 1024,
        max_message_bytes: 2 * 1024,
        max_age: Duration::from_secs(60),
        max_per_fetch: 1,
    };
    let mut store = PropagationStore::new(limits.clone());
    assert_eq!(
        store
            .ingest(
                &PropagationBatch {
                    transfer_time: 1_753_603_206.0,
                    entries: vec![first.entry],
                },
                1_753_603_206.0,
            )
            .inserted,
        1
    );
    assert_eq!(
        store
            .ingest(
                &PropagationBatch {
                    transfer_time: 1_753_603_207.0,
                    entries: vec![second.entry],
                },
                1_753_603_207.0,
            )
            .inserted,
        1
    );

    let snapshot_file = SnapshotFile::unique();
    snapshot_file.replace(&store.encode_snapshot().unwrap());
    drop(store);
    let snapshot = std::fs::read(snapshot_file.path()).unwrap();
    let (store, restored) =
        PropagationStore::restore(limits.clone(), &snapshot, 1_753_603_208.0).unwrap();
    assert_eq!(
        restored,
        StoreRestoreReceipt {
            loaded: 2,
            ..StoreRestoreReceipt::default()
        }
    );

    let first_server = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let accepted = node.accept_resource().await.unwrap();
            let state = PropagationNode::new(store, NodePolicy::from_announce(&announce()));
            let served = serve_fetch(&node, accepted, &state, || 1_753_603_209.0)
                .await
                .unwrap();
            (state.into_store(), served)
        }
    });
    let first_fetch = fetch_propagation(
        &first_recipient,
        &first_node_announce,
        1_753_603_209.0,
        |_| false,
        &policy(true),
    )
    .await
    .unwrap();
    let (store, served) = first_server.await.unwrap();
    assert_eq!(served.served, vec![first.transient_id]);
    assert_eq!(first_fetch.messages.len(), 1);
    assert_eq!(
        first_fetch.messages[0].message.payload.content,
        b"for the first owner"
    );

    let acknowledgement_server = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let accepted = node.accept_resource().await.unwrap();
            let state = PropagationNode::new(store, NodePolicy::from_announce(&announce()));
            let served = serve_fetch(&node, accepted, &state, || 1_753_603_210.0)
                .await
                .unwrap();
            (state.into_store(), served)
        }
    });
    let acknowledged = fetch_propagation(
        &first_recipient,
        &first_node_announce,
        1_753_603_210.0,
        |id| *id == first.transient_id,
        &policy(false),
    )
    .await
    .unwrap();
    let (store, served) = acknowledgement_server.await.unwrap();
    assert_eq!(served.acknowledged, 1);
    assert!(acknowledged.messages.is_empty());

    snapshot_file.replace(&store.encode_snapshot().unwrap());
    let (store, restored) = PropagationStore::restore(
        limits,
        &std::fs::read(snapshot_file.path()).unwrap(),
        1_753_603_211.0,
    )
    .unwrap();
    assert_eq!(restored.loaded, 1);
    assert_eq!(store.len(), 1);

    let second_server = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let accepted = node.accept_resource().await.unwrap();
            let state = PropagationNode::new(store, NodePolicy::from_announce(&announce()));
            serve_fetch(&node, accepted, &state, || 1_753_603_212.0)
                .await
                .unwrap()
        }
    });
    let second_fetch = fetch_propagation(
        &second_recipient,
        &second_node_announce,
        1_753_603_212.0,
        |_| false,
        &policy(true),
    )
    .await
    .unwrap();
    let served = second_server.await.unwrap();
    assert_eq!(served.served, vec![second.transient_id]);
    assert_eq!(second_fetch.messages.len(), 1);
    assert_eq!(
        second_fetch.messages[0].message.payload.content,
        b"for the second owner"
    );
}
