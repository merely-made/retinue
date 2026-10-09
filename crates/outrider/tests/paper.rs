//! Paper messages between two endpoints: sealed to the recipient's advertised ratchet, as
//! stock packs one, and opened through the propagated-message decrypt path.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, PropagationError, PropagationMessage, delivery_name,
    prepare_paper, register_delivery, register_opportunistic, verify_message,
};
use retinue::endpoint::Endpoint;
use retinue::hash::AddressHash;
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::RatchetStore;

async fn heard(endpoint: &Endpoint, destination: AddressHash) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while endpoint.next_announcement().await.unwrap().destination != destination {}
    })
    .await
    .expect("announce arrives");
}

#[tokio::test]
async fn a_paper_message_is_sealed_to_the_recipients_ratchet_and_opens_with_it() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x23; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let recipient = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x43; 64],
    )));
    connect(&sender, &recipient, LossModel::new(23), LossModel::new(43));
    let source = register_delivery(&sender, &DeliveryAnnounce::named(b"Paper Sender")).unwrap();
    heard(&recipient, source).await;
    let destination = register_opportunistic(
        &recipient,
        &DeliveryAnnounce::named(b"Paper Recipient"),
        RatchetStore::new(Default::default()).unwrap(),
    )
    .unwrap();
    heard(&sender, destination).await;

    let payload = LxmfPayload::text(1_753_603_202.5, b"paper", b"sealed to a ratchet");
    let paper = prepare_paper(&sender, &sender_identity, destination, &payload).unwrap();
    let current = recipient.current_ratchet_id(&delivery_name());
    assert!(current.is_some());
    assert_eq!(paper.ratchet_id, current);

    let read = PropagationMessage::from_uri(&paper.message.to_uri().unwrap(), 4096).unwrap();
    let (message, ratchet_id) = read.decrypt_for(&recipient, 4096).unwrap();
    assert_eq!(ratchet_id, current);
    assert_eq!(message.message_id, paper.message_id);
    assert_eq!(message.payload.content, payload.content);
    let source_identity = recipient.resolve(source);
    assert_eq!(
        verify_message(&message, source_identity.as_ref()),
        outrider::Verification::Verified
    );

    // A ratchet-sealed token does not open with the identity key alone.
    let recipient_key = PrivateIdentity::from_secret_bytes(&[0x43; 64]);
    assert!(matches!(
        read.decrypt(&recipient_key, 4096),
        Err(PropagationError::Crypto(_))
    ));
}
