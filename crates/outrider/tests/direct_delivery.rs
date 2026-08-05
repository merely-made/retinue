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

/// A message from a sender we have never heard announce is refused, *and asked about*, so the
/// sender's next attempt succeeds.
///
/// This is the MeshChatX 2.0.1 case, reduced. A stock client opened a conversation and sent
/// before it had announced on our air; the message arrived intact three times and was dropped
/// three times, because verifying a signature needs the sender's keys and an announce is the
/// only thing that carries them. Refusing is correct. Refusing silently made it permanent.
#[tokio::test]
async fn an_unknown_sender_is_asked_about_so_its_retry_lands() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x51; 64]);
    let receiver_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let receiver = Arc::new(Endpoint::new(receiver_identity.clone()));

    // Registered before there is any interface to carry it, so the announce reaches nobody.
    // That is the whole setup: the receiver will meet this sender for the first time as an
    // unverifiable message rather than as an announce.
    register_delivery(&sender, &DeliveryAnnounce::named(b"Stranger".to_vec())).unwrap();

    connect(&sender, &receiver, LossModel::new(7), LossModel::new(9));
    register_delivery(&receiver, &DeliveryAnnounce::named(b"Receiver".to_vec())).unwrap();

    let receiver_announce =
        tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
            .await
            .expect("the receiver announces")
            .unwrap();

    // First attempt: refused, because the source cannot be resolved.
    let first = tokio::spawn({
        let receiver = Arc::clone(&receiver);
        async move {
            let accepted = receiver.accept_resource().await.unwrap();
            receive_direct_with_stamp_cost(&receiver, accepted, 64 * 1024, None).await
        }
    });
    let payload = LxmfPayload::text(1_753_603_203.5, b"TITLE", b"first try".to_vec());
    send_direct_stamped(
        &sender,
        &sender_identity,
        &receiver_announce,
        &payload,
        [0; 32],
        0,
    )
    .await
    .unwrap();
    let refused = tokio::time::timeout(Duration::from_secs(10), first)
        .await
        .expect("the receive completes")
        .unwrap();
    assert!(
        matches!(refused, Err(outrider::DirectError::UnknownSource { .. })),
        "an unverifiable message is refused, not delivered: {refused:?}",
    );

    // The refusal asked. The sender answers the path request with an announce, which is what
    // carries its identity, so the receiver now knows who it is.
    let learned = tokio::time::timeout(Duration::from_secs(5), receiver.next_announcement())
        .await
        .expect("the path request was answered")
        .unwrap();
    assert_eq!(
        learned.identity,
        *sender_identity.public(),
        "the answer is the sender's own announce",
    );

    // Second attempt, which is what a retrying client does: it lands.
    let second = tokio::spawn({
        let receiver = Arc::clone(&receiver);
        async move {
            let accepted = receiver.accept_resource().await.unwrap();
            receive_direct_with_stamp_cost(&receiver, accepted, 64 * 1024, None).await
        }
    });
    let payload = LxmfPayload::text(1_753_603_204.5, b"TITLE", b"second try".to_vec());
    send_direct_stamped(
        &sender,
        &sender_identity,
        &receiver_announce,
        &payload,
        [0; 32],
        0,
    )
    .await
    .unwrap();
    let received = tokio::time::timeout(Duration::from_secs(10), second)
        .await
        .expect("the retry completes")
        .unwrap()
        .expect("and is accepted");
    assert_eq!(received.message.payload.content, b"second try");
    assert_eq!(received.source_identity, *sender_identity.public());
}

/// A sender that has never announced is still accepted when it identifies on the link.
///
/// The stronger of the two recoveries, and the one that needs nothing from the network: an
/// IDENTIFY is the peer on the other end of *this* link signing that it is that identity,
/// which is better evidence than an announce saying a destination exists somewhere. Request
/// links already read it; resource links dropped it, so an LXMF message arriving on a link
/// whose sender had said exactly who it was still failed to authenticate.
///
/// It is only accepted when it derives to the destination the message names as its source,
/// which the second half of this test pins.
#[tokio::test]
async fn a_sender_that_identifies_on_the_link_is_accepted_without_any_announce() {
    use retinue::endpoint::ResourceTransferConfig;
    use retinue::identity::Identity;

    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x73; 64]);
    let receiver_identity = PrivateIdentity::from_secret_bytes(&[0x84; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let receiver = Arc::new(Endpoint::new(receiver_identity.clone()));

    // Registered before any interface exists, so this announce reaches nobody and the
    // receiver never learns the sender from the air.
    register_delivery(&sender, &DeliveryAnnounce::named(b"Silent".to_vec())).unwrap();
    connect(&sender, &receiver, LossModel::new(3), LossModel::new(5));
    register_delivery(&receiver, &DeliveryAnnounce::named(b"Receiver".to_vec())).unwrap();

    let receiver_announce =
        tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
            .await
            .expect("the receiver announces")
            .unwrap();

    let accept = tokio::spawn({
        let receiver = Arc::clone(&receiver);
        async move {
            let accepted = receiver.accept_resource().await.unwrap();
            receive_direct_with_stamp_cost(&receiver, accepted, 64 * 1024, None).await
        }
    });

    // Build the message by hand so the sender can identify before publishing it, which is
    // what `Endpoint::send_payload_with_config` does not expose.
    let payload = LxmfPayload::text(1_753_603_205.5, b"TITLE", b"identified".to_vec());
    let prepared = outrider::prepare(
        *receiver_announce.destination.as_bytes(),
        *outrider::delivery_destination(sender_identity.public()).as_bytes(),
        &payload,
    )
    .unwrap();
    let signature = sender_identity.sign(prepared.signing_bytes());
    let packed = prepared.finish(signature);

    let mut session = sender
        .open_resource(receiver_announce.destination, receiver_announce.identity)
        .await
        .expect("link to the receiver");
    session.set_config(ResourceTransferConfig::default());
    session.identify();
    session.publish(&packed).await.expect("publish the message");

    let received = tokio::time::timeout(Duration::from_secs(10), accept)
        .await
        .expect("the receive completes")
        .unwrap()
        .expect("an identified sender is accepted with no announce anywhere");
    assert_eq!(received.message.payload.content, b"identified");
    assert_eq!(received.source_identity, *sender_identity.public());

    // And the identity is only accepted for the source it actually derives to: a peer that
    // identifies as itself while claiming somebody else's source is refused.
    let impostor_source =
        outrider::delivery_destination(PrivateIdentity::from_secret_bytes(&[0x95; 64]).public());
    assert_ne!(
        impostor_source,
        outrider::delivery_destination(sender_identity.public()),
    );
    let resolved: Option<Identity> = outrider::resolve_source_with_link(
        &receiver,
        impostor_source,
        Some(*sender_identity.public()),
    );
    assert!(
        resolved.is_none(),
        "an IDENTIFY proves who the peer is, not who its message claims to be from",
    );
}
