use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, receive_direct_with_stamp_cost,
    receive_direct_with_stamp_cost_and_resource_config, register_delivery, send_direct_stamped,
    send_direct_stamped_with_resource_config,
};
use retinue::endpoint::{Endpoint, PayloadMode, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};

async fn round_trip(
    content: Vec<u8>,
    expected_mode: PayloadMode,
    resource_config: Option<ResourceTransferConfig>,
) {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let receiver_identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let receiver = Arc::new(Endpoint::new(receiver_identity.clone()));
    connect(&sender, &receiver, LossModel::new(31), LossModel::new(42));

    register_delivery(
        &sender,
        &DeliveryAnnounce {
            display_name: Some(b"Sender".to_vec()),
            stamp_cost: None,
        },
    )
    .unwrap();
    register_delivery(
        &receiver,
        &DeliveryAnnounce {
            display_name: Some(b"Receiver".to_vec()),
            stamp_cost: Some(8),
        },
    )
    .unwrap();

    let receiver_announce =
        tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
            .await
            .unwrap()
            .unwrap();
    let sender_announce =
        tokio::time::timeout(Duration::from_secs(2), receiver.next_announcement())
            .await
            .unwrap()
            .unwrap();
    assert_eq!(sender_announce.identity, *sender_identity.public());
    assert_eq!(receiver_announce.identity, *receiver_identity.public());

    let receive_task = tokio::spawn({
        let receiver = Arc::clone(&receiver);
        async move {
            let accepted = receiver.accept_resource().await.unwrap();
            match resource_config {
                Some(config) => receive_direct_with_stamp_cost_and_resource_config(
                    &receiver,
                    accepted,
                    64 * 1024,
                    Some(8),
                    config,
                )
                .await
                .unwrap(),
                None => receive_direct_with_stamp_cost(&receiver, accepted, 64 * 1024, Some(8))
                    .await
                    .unwrap(),
            }
        }
    });

    let payload = LxmfPayload::text(1_753_603_203.5, b"TITLE", content.clone());
    let receipt = match resource_config {
        Some(config) => send_direct_stamped_with_resource_config(
            &sender,
            &sender_identity,
            &receiver_announce,
            &payload,
            [0; 32],
            100_000,
            config,
        )
        .await
        .unwrap(),
        None => send_direct_stamped(
            &sender,
            &sender_identity,
            &receiver_announce,
            &payload,
            [0; 32],
            100_000,
        )
        .await
        .unwrap(),
    };
    let received = tokio::time::timeout(Duration::from_secs(10), receive_task)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(receipt.mode, expected_mode);
    assert_eq!(received.mode, expected_mode);
    assert_eq!(received.message.message_id, receipt.message_id);
    assert_eq!(received.message.payload.title, b"TITLE");
    assert_eq!(received.message.payload.content, content);
    assert_eq!(received.source_identity, *sender_identity.public());
}

#[tokio::test]
async fn direct_delivery_uses_one_data_packet_when_the_message_fits() {
    round_trip(b"small body".to_vec(), PayloadMode::Data, None).await;
}

#[tokio::test]
async fn direct_delivery_uses_a_resource_when_the_message_does_not_fit() {
    let content: Vec<u8> = (0..4096_u32)
        .map(|value| value.wrapping_mul(73).wrapping_add(19) as u8)
        .collect();
    round_trip(
        content,
        PayloadMode::Resource,
        Some(ResourceTransferConfig {
            timeout: Duration::from_secs(30),
            retry_interval: Duration::from_millis(100),
            request_window: 1,
        }),
    )
    .await;
}
