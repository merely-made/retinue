use std::sync::Arc;
use std::time::Duration;

use outrider::{
    LxmfPayload, PROPAGATION_METADATA_NAME, PropagationAnnounce, PropagationBatch,
    PropagationCosts, prepare_propagation, receive_submission, register_propagation,
    submit_propagation,
};
use retinue::endpoint::{Endpoint, PayloadMode};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use rmpv::Value;

#[tokio::test]
async fn stamped_submission_crosses_the_propagation_boundary() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let node_identity = PrivateIdentity::from_secret_bytes(&[0x70; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let node = Arc::new(Endpoint::new(node_identity));
    connect(&sender, &node, LossModel::new(61), LossModel::new(70));

    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: 1_753_603_200,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
        costs: PropagationCosts {
            propagation: 8,
            flexibility: 3,
            peering: 8,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"Test Node".to_vec()),
        )],
    };
    register_propagation(&node, &announce).unwrap();
    let node_announce = tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .unwrap()
        .unwrap();

    let prepared = prepare_propagation(
        &sender_identity,
        recipient_identity.public(),
        &LxmfPayload::text(1_753_603_204.5, b"PROPAGATION TITLE", b"PROPAGATION BODY"),
        &[0x31; 32],
        &[0x41; 16],
        [0; 32],
        8,
        100_000,
    )
    .unwrap();
    let batch = PropagationBatch {
        transfer_time: 1_753_603_205.0,
        entries: vec![prepared.entry.clone()],
    };

    let receive_task = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let accepted = node.accept_resource().await.unwrap();
            receive_submission(&node, accepted, 8, 4_096, 1)
                .await
                .unwrap()
        }
    });
    let receipt = tokio::time::timeout(
        Duration::from_secs(5),
        submit_propagation(&sender, &node_announce, &batch),
    )
    .await
    .unwrap()
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(5), receive_task)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(receipt.mode, PayloadMode::Data);
    assert_eq!(received.mode, PayloadMode::Data);
    assert_eq!(receipt.transient_ids, vec![prepared.transient_id]);
    assert_eq!(
        received.batch.entries[0].transient_id(),
        prepared.transient_id
    );
    let decoded = received.batch.entries[0]
        .decrypt_and_verify(
            &recipient_identity,
            sender_identity.public(),
            receipt.packed_batch.len(),
        )
        .unwrap();
    assert_eq!(decoded.message_id, prepared.message_id);
    assert_eq!(decoded.payload.title, b"PROPAGATION TITLE");
    assert_eq!(decoded.payload.content, b"PROPAGATION BODY");
}
