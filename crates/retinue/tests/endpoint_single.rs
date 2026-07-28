//! Endpoint-level ratcheted single-packet delivery.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::Endpoint;
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::{RatchetPolicy, RatchetStore};

#[tokio::test]
async fn current_and_retained_ratchets_deliver_without_opening_a_link() {
    let receiver_id = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
    let receiver = Endpoint::new(receiver_id.clone());
    let sender = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x24; 64]));
    connect(&sender, &receiver, LossModel::new(1), LossModel::new(2));

    let name = DestinationName::new("retinue", ["single"]);
    let destination = name.destination_hash(receiver_id.public());
    let mut ratchets = RatchetStore::new(RatchetPolicy {
        max_count: 4,
        rotation_interval: Duration::from_secs(1),
        max_age: Duration::from_secs(60),
    })
    .unwrap();
    let first = ratchets
        .rotate_if_due([0x31; KEY_LEN], 0.0)
        .unwrap()
        .current;
    receiver
        .register_resource_with_ratchets(name.clone(), b"single", &ratchets)
        .unwrap();

    let announced = tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .expect("ratcheted announce arrives")
        .unwrap();
    assert_eq!(announced.destination, destination);

    let receipt = sender.send_single(destination, b"first epoch").unwrap();
    assert_eq!(receipt.ratchet_id, first);
    assert_eq!(receipt.queued_interfaces, 1);
    let received = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("single packet arrives")
        .unwrap();
    assert_eq!(received.destination, destination);
    assert_eq!(received.data, b"first epoch");
    assert_eq!(received.ratchet_id, Some(first));

    // Keep the old public ratchet in the sender's address book while installing a new
    // receiver epoch. A packet already encrypted to the old epoch must still decrypt.
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    let second = ratchets
        .rotate_if_due([0x32; KEY_LEN], 1.0)
        .unwrap()
        .current;
    receiver.update_ratchets(&name, &ratchets).unwrap();
    let old_receipt = sender.send_single(destination, b"retained epoch").unwrap();
    assert_eq!(old_receipt.ratchet_id, first);
    let retained = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("retained-ratchet packet arrives")
        .unwrap();
    assert_eq!(retained.data, b"retained epoch");
    assert_eq!(retained.ratchet_id, Some(first));

    // Once the refreshed announce is ingested, new sends select the new public ratchet.
    let refreshed = tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .expect("rotated announce arrives")
        .unwrap();
    assert_eq!(refreshed.destination, destination);
    let new_receipt = sender.send_single(destination, b"current epoch").unwrap();
    assert_eq!(new_receipt.ratchet_id, second);
    let current = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("current-ratchet packet arrives")
        .unwrap();
    assert_eq!(current.data, b"current epoch");
    assert_eq!(current.ratchet_id, Some(second));
}

#[tokio::test]
async fn outbound_single_requires_an_advertised_ratchet_and_enforces_the_mdu() {
    let receiver_id = PrivateIdentity::from_secret_bytes(&[0x52; 64]);
    let receiver = Endpoint::new(receiver_id.clone());
    let sender = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x25; 64]));
    connect(&sender, &receiver, LossModel::new(3), LossModel::new(4));

    let name = DestinationName::new("retinue", ["plain-single"]);
    let destination = name.destination_hash(receiver_id.public());
    receiver.register(name.clone(), b"plain");
    tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        sender
            .send_single(destination, b"missing ratchet")
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidData,
    );

    let mut ratchets = RatchetStore::new(RatchetPolicy::default()).unwrap();
    ratchets.rotate_if_due([0x53; KEY_LEN], 0.0).unwrap();
    receiver.update_ratchets(&name, &ratchets).unwrap();
    tokio::time::sleep(Duration::from_millis(1_050)).await;
    receiver.announce(&name, b"plain");
    tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        sender
            .send_single(destination, &vec![0; retinue::packet::ENCRYPTED_MDU + 1])
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput,
    );
}

#[tokio::test]
async fn single_packet_receipt_requires_a_frame_capable_interface() {
    let receiver_id = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let receiver = Endpoint::new(receiver_id.clone());
    let sender = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x26; 64]));
    let mut sender_wire = sender.attach_interface_with_frame_limit(255).unwrap();
    let sender_sink = sender_wire.sink();
    let mut receiver_wire = receiver.attach_interface();
    let receiver_sink = receiver_wire.sink();

    let name = DestinationName::new("retinue", ["capped-single"]);
    let destination = name.destination_hash(receiver_id.public());
    let mut ratchets = RatchetStore::new(RatchetPolicy::default()).unwrap();
    ratchets.rotate_if_due([0x63; KEY_LEN], 0.0).unwrap();
    receiver
        .register_resource_with_ratchets(name, b"capped", &ratchets)
        .unwrap();
    let announce = tokio::time::timeout(Duration::from_secs(1), receiver_wire.next_outbound())
        .await
        .expect("announce queued")
        .expect("receiver interface remains live");
    assert!(sender_sink.deliver(announce));
    tokio::time::timeout(Duration::from_secs(1), sender.next_announcement())
        .await
        .expect("announce ingested")
        .unwrap();

    let error = sender.send_single(destination, &[0; 189]).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidInput);
    assert_eq!(
        error.to_string(),
        "single packet is 291 bytes after encryption, interface frame limit is 255"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), sender_wire.next_outbound())
            .await
            .is_err(),
        "an unsendable packet must never enter the interface queue"
    );

    let receipt = sender.send_single(destination, &[0xA5; 143]).unwrap();
    assert_eq!(receipt.queued_interfaces, 1);
    let packet = tokio::time::timeout(Duration::from_secs(1), sender_wire.next_outbound())
        .await
        .expect("fitting packet queued")
        .expect("sender interface remains live");
    assert_eq!(packet.encoded_len(), 243);
    assert!(receiver_sink.deliver(packet));
    let received = tokio::time::timeout(Duration::from_secs(1), receiver.accept_single())
        .await
        .expect("fitting packet delivered")
        .unwrap();
    assert_eq!(received.data, &[0xA5; 143]);
}
