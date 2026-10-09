//! Outbound receipts: what a send learns about its delivery, and stamping that costs no
//! more than it must.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveredCache, DeliveryAnnounce, LxmfPayload, OpportunisticError, delivery_name,
    receive_direct, register_delivery, register_opportunistic, send_direct_stamped,
    send_opportunistic, send_opportunistic_stamped,
};
use retinue::endpoint::{Endpoint, PayloadMode, PeerAnnounce, ProofStrategy, SingleDelivery};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

struct Pair {
    sender_identity: PrivateIdentity,
    sender: Arc<Endpoint>,
    receiver: Arc<Endpoint>,
    receiver_announce: PeerAnnounce,
}

/// Two connected endpoints, the receiver announcing `announce` for opportunistic delivery
/// (and, being registered for Resources, for direct delivery too).
async fn pair(announce: DeliveryAnnounce) -> Pair {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x51; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let receiver = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x52; 64],
    )));
    connect(&sender, &receiver, LossModel::new(5), LossModel::new(6));
    let ratchets = RatchetStore::new(RatchetPolicy::default()).unwrap();
    let destination = register_opportunistic(&receiver, &announce, ratchets).unwrap();
    let receiver_announce = tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let peer = sender.next_announcement().await.unwrap();
            if peer.destination == destination {
                return peer;
            }
        }
    })
    .await
    .expect("delivery announce arrives");
    Pair {
        sender_identity,
        sender,
        receiver,
        receiver_announce,
    }
}

#[tokio::test]
async fn an_opportunistic_receipt_is_delivered_once_the_recipient_proves() {
    let pair = pair(DeliveryAnnounce::named(b"Prover")).await;
    pair.receiver
        .set_proof_strategy(&delivery_name(), ProofStrategy::All)
        .unwrap();
    let receipt = send_opportunistic(
        &pair.sender,
        &pair.sender_identity,
        &pair.receiver_announce,
        &LxmfPayload::text(1_753_603_220.5, b"TITLE", b"prove me"),
    )
    .unwrap();
    assert!(matches!(
        receipt.packet.delivery().await,
        SingleDelivery::Delivered { .. }
    ));
}

#[tokio::test]
async fn an_oversized_stamped_message_is_refused_before_minting() {
    let pair = pair(DeliveryAnnounce {
        display_name: Some(b"Costly".to_vec()),
        stamp_cost: Some(250),
    })
    .await;
    // Content that fits one packet bare, but not with the 34 bytes a stamp adds.
    let source = *outrider::delivery_destination(pair.sender_identity.public()).as_bytes();
    let destination = *pair.receiver_announce.destination.as_bytes();
    let single_len = |content_len: usize| {
        let payload = LxmfPayload::text(1.5, b"", vec![0; content_len]);
        let prepared = outrider::prepare(destination, source, &payload).unwrap();
        prepared.stamped_len() - 34 - outrider::DESTINATION_LEN
    };
    let mdu = retinue::packet::ENCRYPTED_MDU;
    let content_len = (0..mdu).rev().find(|&len| single_len(len) <= mdu).unwrap();
    let payload = LxmfPayload::text(1.5, b"", vec![0; content_len]);

    // Had minting started, this budget at cost 250 would end in StampBudgetExhausted.
    let refused = send_opportunistic_stamped(
        &pair.sender,
        &pair.sender_identity,
        &pair.receiver_announce,
        &payload,
        [0; 32],
        1 << 16,
    );
    assert!(matches!(refused, Err(OpportunisticError::TooLarge)));
}

#[tokio::test]
async fn a_direct_resource_is_delivered_on_its_proof() {
    let pair = pair(DeliveryAnnounce::named(b"Direct")).await;
    let source = register_delivery(&pair.sender, &DeliveryAnnounce::named(b"Sender")).unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while pair.receiver.next_announcement().await.unwrap().destination != source {}
    })
    .await
    .expect("sender announce arrives");
    let receiver = Arc::clone(&pair.receiver);
    let receiving = tokio::spawn(async move {
        let accepted = receiver.accept_resource().await.unwrap();
        receive_direct(&receiver, accepted, &DeliveredCache::default(), 64 * 1024)
            .await
            .unwrap()
    });

    let content: Vec<u8> = (0..4096_u32).map(|n| n.wrapping_mul(97) as u8).collect();
    let receipt = send_direct_stamped(
        &pair.sender,
        &pair.sender_identity,
        &pair.receiver_announce,
        &LxmfPayload::text(1_753_603_221.5, b"TITLE", content),
        [0; 32],
        0,
    )
    .await
    .unwrap();
    assert_eq!(receipt.mode, PayloadMode::Resource);
    assert!(receipt.delivered);
    assert_eq!(
        receiving.await.unwrap().message.message_id,
        receipt.message_id
    );
}
