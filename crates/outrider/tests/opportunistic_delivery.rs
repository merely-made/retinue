use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, delivery_destination, receive_opportunistic_with_stamp_cost,
    register_opportunistic, send_opportunistic, send_opportunistic_stamped,
};
use retinue::endpoint::{Endpoint, PeerAnnounce};
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

fn ratchets(secret: u8) -> RatchetStore {
    let mut store = RatchetStore::new(RatchetPolicy::default()).unwrap();
    store.rotate_if_due([secret; KEY_LEN], 0.0).unwrap();
    store
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

    let sender_destination = register_opportunistic(
        &sender,
        &DeliveryAnnounce::named(b"Sender"),
        &ratchets(0x51),
    )
    .unwrap();
    let receiver_destination = register_opportunistic(
        &receiver,
        &DeliveryAnnounce {
            display_name: Some(b"Receiver".to_vec()),
            stamp_cost: Some(8),
        },
        &ratchets(0x52),
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
        outrider::DEFAULT_MAX_MESSAGE_BYTES,
        Some(8),
    )
    .unwrap();

    assert_eq!(received.message.message_id, receipt.message_id);
    assert_eq!(received.message.payload.title, b"TITLE");
    assert_eq!(received.message.payload.content, b"opportunistic body");
    assert_eq!(received.source_identity, *sender_identity.public());
    assert_eq!(received.ratchet_id, receipt.ratchet_id);
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

    let sender_destination = register_opportunistic(
        &sender,
        &DeliveryAnnounce::named(b"Sender"),
        &ratchets(0x71),
    )
    .unwrap();
    let receiver_destination = register_opportunistic(
        &receiver,
        &DeliveryAnnounce::named(b"Receiver"),
        &ratchets(0x72),
    )
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
    let received =
        outrider::receive_opportunistic(&receiver, single, outrider::DEFAULT_MAX_MESSAGE_BYTES)
            .unwrap();

    assert_eq!(received.message.message_id, receipt.message_id);
    assert_eq!(received.message.payload.content, b"through the hub");
    assert_eq!(
        received.message.destination,
        *delivery_destination(receiver_identity.public()).as_bytes()
    );
    assert_eq!(hub.routing_counters().forwarded_packets, 1);
}
