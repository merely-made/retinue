//! Tokens sealed with `encrypt_for` and opened with `decrypt_for`, outside any packet.

use std::io;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::RatchetStore;

async fn ratcheted_pair() -> (Endpoint, Endpoint, PrivateIdentity, DestinationName) {
    let receiver_id = PrivateIdentity::from_secret_bytes(&[0x52; 64]);
    let receiver = Endpoint::new(receiver_id.clone());
    let sender = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x25; 64]));
    connect(&sender, &receiver, LossModel::new(3), LossModel::new(4));
    let name = DestinationName::new("retinue", ["sealed"]);
    receiver
        .register_resource_with_ratchets(
            name.clone(),
            b"sealed",
            RatchetStore::new(Default::default()).unwrap(),
        )
        .unwrap();
    tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .expect("ratcheted announce arrives")
        .unwrap();
    (sender, receiver, receiver_id, name)
}

#[tokio::test]
async fn sealed_tokens_use_the_advertised_ratchet_and_open_with_it() {
    let (sender, receiver, receiver_id, name) = ratcheted_pair().await;
    let destination = name.destination_hash(receiver_id.public());
    let current = receiver.current_ratchet_id(&name);
    assert!(current.is_some());

    let (token, ratchet_id) = sender.encrypt_for(destination, b"stored").unwrap();
    assert_eq!(ratchet_id, current);
    assert_eq!(
        receiver.decrypt_for(&name, &token).unwrap(),
        (b"stored".to_vec(), current)
    );

    // The ratchet's token does not open with the identity key alone.
    assert!(retinue::token::decrypt_to_identity(&receiver_id, &token).is_err());

    let unknown =
        DestinationName::new("retinue", ["unknown"]).destination_hash(receiver_id.public());
    assert_eq!(
        sender.encrypt_for(unknown, b"x").unwrap_err().kind(),
        io::ErrorKind::NotFound
    );
    assert_eq!(
        receiver
            .decrypt_for(&DestinationName::new("retinue", ["unknown"]), &token)
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[tokio::test]
async fn identity_tokens_open_unless_ratchets_are_enforced() {
    let (_sender, receiver, receiver_id, name) = ratcheted_pair().await;
    let token = retinue::token::encrypt_to_identity(
        receiver_id.public(),
        &[0x11; 32],
        &[0x22; 16],
        b"identity",
    );
    assert_eq!(
        receiver.decrypt_for(&name, &token).unwrap(),
        (b"identity".to_vec(), None)
    );

    receiver.set_enforce_ratchets(&name, true).unwrap();
    assert_eq!(
        receiver.decrypt_for(&name, &token).unwrap_err().kind(),
        io::ErrorKind::PermissionDenied
    );
    assert_eq!(
        receiver
            .decrypt_for(&name, &token[..40])
            .unwrap_err()
            .kind(),
        io::ErrorKind::PermissionDenied
    );
    receiver.set_enforce_ratchets(&name, false).unwrap();
    assert_eq!(
        receiver
            .decrypt_for(&name, &token[..40])
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
}
