use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, PROPAGATION_METADATA_NAME, PropagationAnnounce,
    PropagationBatch, PropagationCosts, PropagationStore, PropagationStoreLimits,
    fetch_propagation_with_resource_config, prepare_propagation, register_delivery,
    register_propagation, serve_fetch,
};
use retinue::endpoint::{Endpoint, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use rmpv::Value;

#[tokio::test]
async fn large_fetch_response_uses_a_resource_and_authenticates() {
    let node_identity = PrivateIdentity::from_secret_bytes(&[0x70; 64]);
    let recipient_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let node = Arc::new(Endpoint::new(node_identity.clone()));
    let recipient = Arc::new(Endpoint::new(recipient_identity.clone()));
    connect(&recipient, &node, LossModel::new(62), LossModel::new(70));

    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: 1_753_603_200,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
        costs: PropagationCosts {
            propagation: 0,
            flexibility: 0,
            peering: 0,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"Large Response Node".to_vec()),
        )],
    };
    register_propagation(&node, &announce).unwrap();
    let node_announce = tokio::time::timeout(Duration::from_secs(2), recipient.next_announcement())
        .await
        .unwrap()
        .unwrap();
    register_delivery(&node, &DeliveryAnnounce::named(b"Large Response Sender")).unwrap();
    let source_announce =
        tokio::time::timeout(Duration::from_secs(2), recipient.next_announcement())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(source_announce.identity, *node_identity.public());
    register_delivery(
        &recipient,
        &DeliveryAnnounce::named(b"Large Response Recipient"),
    )
    .unwrap();

    let content: Vec<u8> = (0..4_096_u32)
        .map(|value| value.wrapping_mul(73).wrapping_add(19) as u8)
        .collect();
    let prepared = prepare_propagation(
        &node_identity,
        recipient_identity.public(),
        &LxmfPayload::text(1_753_603_204.5, b"PROPAGATION TITLE", content.clone()),
        &[0x31; 32],
        &[0x41; 16],
        [0; 32],
        0,
        1,
    )
    .unwrap();
    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_entries: 4,
        max_bytes: 64 * 1024,
        max_message_bytes: 16 * 1024,
        max_age: Duration::from_secs(60),
        max_per_fetch: 1,
    });
    assert_eq!(
        store
            .ingest(
                &PropagationBatch {
                    transfer_time: 1_753_603_205.0,
                    entries: vec![prepared.entry],
                },
                1_753_603_205.0,
            )
            .inserted,
        1
    );

    let server = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let mut accepted = node.accept_resource().await.unwrap();
            accepted.session.set_config(ResourceTransferConfig {
                timeout: Duration::from_secs(5),
                retry_interval: Duration::from_millis(50),
                request_window: 1,
            });
            serve_fetch(&node, &mut accepted, &mut store, 1_753_603_206.0)
                .await
                .unwrap()
        }
    });
    let receipt = tokio::time::timeout(
        Duration::from_secs(15),
        fetch_propagation_with_resource_config(
            &recipient,
            &recipient_identity,
            &node_announce,
            &[],
            1,
            1_753_603_206.0,
            16 * 1024,
            16 * 1024,
            ResourceTransferConfig {
                timeout: Duration::from_secs(5),
                retry_interval: Duration::from_millis(50),
                request_window: 1,
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let served = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(receipt.offered.len(), 1);
    assert_eq!(receipt.messages.len(), 1);
    assert_eq!(receipt.messages[0].message.payload.content, content);
    assert_eq!(served.served, receipt.offered);
}
