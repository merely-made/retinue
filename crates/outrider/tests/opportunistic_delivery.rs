use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveredCache, DeliveryAnnounce, LxmfPayload, OpportunisticError, StampOutcome, Verification,
    delivery_destination, receive_opportunistic_with_stamp_cost, register_delivery,
    register_opportunistic, send_opportunistic, send_opportunistic_stamped,
};
use retinue::endpoint::{Endpoint, PeerAnnounce, SingleDelivery, SinglePacketReceipt};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

fn ratchets() -> RatchetStore {
    RatchetStore::new(RatchetPolicy::default()).unwrap()
}

async fn peer(endpoint: &Endpoint, destination: retinue::AddressHash) -> PeerAnnounce {
    tokio::time::timeout(Duration::from_secs(4), async {
        loop {
            let announce = endpoint.next_announcement().await.unwrap();
            if announce.destination == destination {
                return announce;
            }
        }
    })
    .await
    .expect("delivery announce arrives")
}

#[tokio::test]
async fn stamped_opportunistic_delivery_authenticates_without_a_link() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x31; 64]);
    let receiver_identity = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let receiver = Arc::new(Endpoint::new(receiver_identity.clone()));
    connect(&sender, &receiver, LossModel::new(31), LossModel::new(42));

    let sender_destination =
        register_opportunistic(&sender, &DeliveryAnnounce::named(b"Sender"), ratchets()).unwrap();
    let receiver_destination = register_opportunistic(
        &receiver,
        &DeliveryAnnounce {
            display_name: Some(b"Receiver".to_vec()),
            stamp_cost: Some(8),
        },
        ratchets(),
    )
    .unwrap();
    let receiver_announce = peer(&sender, receiver_destination).await;
    let _sender_announce = peer(&receiver, sender_destination).await;

    let payload = LxmfPayload::text(1_753_603_210.5, b"TITLE", b"opportunistic body");
    let receipt = send_opportunistic_stamped(
        &sender,
        &sender_identity,
        &receiver_announce,
        &payload,
        [0; 32],
        100_000,
    )
    .unwrap();

    let single = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("single packet arrives")
        .unwrap();
    let received = receive_opportunistic_with_stamp_cost(
        &receiver,
        single,
        &DeliveredCache::default(),
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
        Some(8),
    )
    .unwrap();

    assert_eq!(received.message.message_id, receipt.message_id);
    assert_eq!(received.message.payload.title, b"TITLE");
    assert_eq!(received.message.payload.content, b"opportunistic body");
    assert_eq!(received.verification, Verification::Verified);
    assert_eq!(received.source_identity, Some(*sender_identity.public()));
    assert!(matches!(received.stamp, Some(StampOutcome::Work(value)) if value >= 8));
    assert_eq!(received.ratchet_id, receipt.packet.ratchet_id);
    assert!(received.ratchet_id.is_some());
    assert_eq!(received.packed, receipt.packed);
}

#[tokio::test]
async fn opportunistic_delivery_crosses_a_transport_node() {
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x61; 64]));
    hub.enable_routing();

    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let sender = Endpoint::new(sender_identity.clone());
    connect(&sender, &hub, LossModel::new(1), LossModel::new(2));

    let receiver_identity = PrivateIdentity::from_secret_bytes(&[0x63; 64]);
    let receiver = Endpoint::new(receiver_identity.clone());
    connect(&receiver, &hub, LossModel::new(3), LossModel::new(4));

    let sender_destination =
        register_opportunistic(&sender, &DeliveryAnnounce::named(b"Sender"), ratchets()).unwrap();
    let receiver_destination =
        register_opportunistic(&receiver, &DeliveryAnnounce::named(b"Receiver"), ratchets())
            .unwrap();
    let receiver_announce = peer(&sender, receiver_destination).await;
    let _sender_announce = peer(&receiver, sender_destination).await;

    let receipt = send_opportunistic(
        &sender,
        &sender_identity,
        &receiver_announce,
        &LxmfPayload::text(1_753_603_211.5, b"TITLE", b"through the hub"),
    )
    .unwrap();
    let single = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("forwarded single packet arrives")
        .unwrap();
    let received = outrider::receive_opportunistic(
        &receiver,
        single,
        &DeliveredCache::default(),
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
    )
    .unwrap();

    assert_eq!(received.message.message_id, receipt.message_id);
    assert_eq!(received.message.payload.content, b"through the hub");
    assert_eq!(
        received.message.destination,
        *delivery_destination(receiver_identity.public()).as_bytes()
    );
    assert_eq!(hub.routing_counters().forwarded_packets, 1);
}

/// Sign `payload` and send it as one single packet, keeping the receipt that learns of the
/// proof.
fn send_single(
    sender: &Endpoint,
    identity: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
) -> SinglePacketReceipt {
    let prepared = outrider::prepare(
        *peer.destination.as_bytes(),
        *delivery_destination(identity.public()).as_bytes(),
        payload,
    )
    .unwrap();
    let signature = identity.sign(prepared.signing_bytes());
    let packed = prepared.finish(signature);
    sender
        .send_single(peer.destination, &packed[outrider::DESTINATION_LEN..])
        .unwrap()
}

async fn receive(
    receiver: &Endpoint,
    delivered: &DeliveredCache,
) -> Result<outrider::ReceivedOpportunistic, OpportunisticError> {
    let single = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("single packet arrives")
        .unwrap();
    outrider::receive_opportunistic(
        receiver,
        single,
        delivered,
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
    )
}

/// Stock proves what it receives, so its sender stops retrying, and drops the retries that
/// still come (`LXMRouter.py` 1975-1996). A retry is re-encrypted, so only the message id
/// can tell it is one.
#[tokio::test]
async fn a_verified_packet_is_proved_and_its_retry_is_a_proved_duplicate() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
    let sender = Endpoint::new(sender_identity.clone());
    let receiver = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x72; 64]));
    connect(&sender, &receiver, LossModel::new(71), LossModel::new(72));
    let sender_destination =
        register_opportunistic(&sender, &DeliveryAnnounce::named(b"Sender"), ratchets()).unwrap();
    let receiver_destination =
        register_opportunistic(&receiver, &DeliveryAnnounce::named(b"Receiver"), ratchets())
            .unwrap();
    let receiver_announce = peer(&sender, receiver_destination).await;
    let _ = peer(&receiver, sender_destination).await;

    let delivered = DeliveredCache::default();
    let payload = LxmfPayload::text(1_753_603_212.5, b"TITLE", b"once");
    let first = send_single(&sender, &sender_identity, &receiver_announce, &payload);
    let received = receive(&receiver, &delivered).await.unwrap();
    assert_eq!(received.verification, Verification::Verified);
    assert!(matches!(
        first.delivery().await,
        SingleDelivery::Delivered { .. }
    ));

    let retry = send_single(&sender, &sender_identity, &receiver_announce, &payload);
    let duplicate = receive(&receiver, &delivered).await;
    assert!(
        matches!(duplicate, Err(OpportunisticError::Duplicate(id)) if id == received.message.message_id),
        "{duplicate:?}"
    );
    assert!(matches!(
        retry.delivery().await,
        SingleDelivery::Delivered { .. }
    ));
}

/// A message from a sender we hold no keys for is handed over unverified and left unproved,
/// so the sender retries while our path request fetches its announce. The retry verifies.
#[tokio::test]
async fn an_unknown_source_is_returned_unproved_until_it_verifies() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x73; 64]);
    let sender = Endpoint::new(sender_identity.clone());
    let receiver = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x74; 64]));
    // Registered before any interface, so its announce reaches nobody.
    register_opportunistic(&sender, &DeliveryAnnounce::named(b"Stranger"), ratchets()).unwrap();
    connect(&sender, &receiver, LossModel::new(73), LossModel::new(74));
    let receiver_destination =
        register_opportunistic(&receiver, &DeliveryAnnounce::named(b"Receiver"), ratchets())
            .unwrap();
    let receiver_announce = peer(&sender, receiver_destination).await;

    let delivered = DeliveredCache::default();
    let payload = LxmfPayload::text(1_753_603_213.5, b"TITLE", b"who am I");
    let first = send_single(&sender, &sender_identity, &receiver_announce, &payload);
    let held = receive(&receiver, &delivered).await.unwrap();
    assert_eq!(held.verification, Verification::SourceUnknown);
    assert_eq!(held.source_identity, None);
    assert!(
        tokio::time::timeout(Duration::from_millis(500), first.delivery())
            .await
            .is_err(),
        "an unverified message is not proved"
    );

    // The path request is answered with the sender's announce; the held copy now verifies.
    let learned = tokio::time::timeout(Duration::from_secs(5), receiver.next_announcement())
        .await
        .expect("the path request was answered")
        .unwrap();
    assert_eq!(learned.identity, *sender_identity.public());
    assert_eq!(
        outrider::reverify(&receiver, &held.message),
        (Verification::Verified, Some(*sender_identity.public()))
    );

    let retry = send_single(&sender, &sender_identity, &receiver_announce, &payload);
    let received = receive(&receiver, &delivered).await.unwrap();
    assert_eq!(received.verification, Verification::Verified);
    assert_eq!(received.message.message_id, held.message.message_id);
    assert!(matches!(
        retry.delivery().await,
        SingleDelivery::Delivered { .. }
    ));
}

/// Stock enforces ratchets only when asked (`LXMRouter.py` 103, 370-371): a destination
/// without them, or a sender that knew none, uses the identity key.
#[tokio::test]
async fn an_unratcheted_packet_is_accepted() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x75; 64]);
    let sender = Endpoint::new(sender_identity.clone());
    let receiver = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x76; 64]));
    connect(&sender, &receiver, LossModel::new(75), LossModel::new(76));
    let sender_destination =
        register_opportunistic(&sender, &DeliveryAnnounce::named(b"Sender"), ratchets()).unwrap();
    let receiver_destination =
        register_delivery(&receiver, &DeliveryAnnounce::named(b"Receiver")).unwrap();
    let receiver_announce = peer(&sender, receiver_destination).await;
    let _ = peer(&receiver, sender_destination).await;

    let payload = LxmfPayload::text(1_753_603_214.5, b"TITLE", b"no ratchet");
    let receipt = send_single(&sender, &sender_identity, &receiver_announce, &payload);
    assert_eq!(receipt.ratchet_id, None);
    let received = receive(&receiver, &DeliveredCache::default())
        .await
        .unwrap();
    assert_eq!(received.ratchet_id, None);
    assert_eq!(received.verification, Verification::Verified);
    assert!(matches!(
        receipt.delivery().await,
        SingleDelivery::Delivered { .. }
    ));
}
