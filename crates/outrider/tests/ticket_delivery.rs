//! A ticket issued in one opportunistic message pays for the reply in place of proof of work.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, LxmfPayload, OpportunisticError, TicketBook,
    receive_opportunistic_with_stamp_cost, receive_opportunistic_with_tickets,
    register_opportunistic, send_opportunistic, send_opportunistic_stamped,
};
use retinue::endpoint::{Endpoint, PeerAnnounce, ReceivedSingle};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

const NOW: f64 = 1_760_000_000.0;

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

async fn single(endpoint: &Endpoint) -> ReceivedSingle {
    tokio::time::timeout(Duration::from_secs(2), endpoint.accept_single())
        .await
        .expect("single packet arrives")
        .unwrap()
}

#[tokio::test]
async fn a_held_ticket_replaces_proof_of_work_on_the_reply() {
    let alice_identity = PrivateIdentity::from_secret_bytes(&[0x51; 64]);
    let bob_identity = PrivateIdentity::from_secret_bytes(&[0x52; 64]);
    let alice = Arc::new(Endpoint::new(alice_identity.clone()));
    let bob = Arc::new(Endpoint::new(bob_identity.clone()));
    connect(&alice, &bob, LossModel::new(51), LossModel::new(52));
    let ratchets = || RatchetStore::new(RatchetPolicy::default()).unwrap();
    let alice_destination =
        register_opportunistic(&alice, &DeliveryAnnounce::named(b"Alice"), ratchets()).unwrap();
    let costly = DeliveryAnnounce {
        display_name: Some(b"Bob".to_vec()),
        stamp_cost: Some(8),
    };
    let bob_destination = register_opportunistic(&bob, &costly, ratchets()).unwrap();
    let to_bob = peer(&alice, bob_destination).await;
    let to_alice = peer(&bob, alice_destination).await;

    // Bob hands Alice a ticket.
    let mut bob_book = TicketBook::new();
    let mut offer = LxmfPayload::text(NOW, b"", b"have a ticket");
    let issued = bob_book
        .include(&mut offer, alice_destination, NOW, [0x5a; 16])
        .unwrap();
    send_opportunistic(&bob, &bob_identity, &to_alice, &offer).unwrap();
    let received = outrider::receive_opportunistic(&alice, single(&alice).await, 4096).unwrap();
    let mut alice_book = TicketBook::new();
    let learned = alice_book.learn(&received.message, &received.source_identity, NOW);
    assert_eq!(learned, Some(issued));

    // Alice's reply carries the ticket stamp. With no attempt budget, proof of work would fail.
    let mut reply = LxmfPayload::text(NOW + 1.0, b"", b"thanks");
    assert!(
        alice_book
            .stamp(&mut reply, bob_destination, alice_identity.public(), NOW)
            .unwrap()
    );
    let receipt =
        send_opportunistic_stamped(&alice, &alice_identity, &to_bob, &reply, [0; 32], 0).unwrap();
    let packet = single(&bob).await;
    let accepted = receive_opportunistic_with_tickets(&bob, packet.clone(), 4096, Some(8), |s| {
        bob_book.inbound(s, NOW)
    })
    .unwrap();
    assert_eq!(accepted.message.message_id, receipt.message_id);

    // Without Bob's book the same stamp is no stamp at all.
    assert!(matches!(
        receive_opportunistic_with_stamp_cost(&bob, packet, 4096, Some(8)),
        Err(OpportunisticError::InvalidStamp)
    ));
}
