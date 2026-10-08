//! Endpoint-level ratcheted single-packet delivery.

use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, Interface, InterfaceSink, ProofStrategy, SingleDelivery};
use retinue::hash::AddressHash;
use retinue::identity::{KEY_LEN, PrivateIdentity};
use retinue::lossy::{LossModel, connect};
use retinue::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
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
    assert_eq!(receipt.ratchet_id, Some(first));
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
    assert_eq!(old_receipt.ratchet_id, Some(first));
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
    assert_eq!(new_receipt.ratchet_id, Some(second));
    let current = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("current-ratchet packet arrives")
        .unwrap();
    assert_eq!(current.data, b"current epoch");
    assert_eq!(current.ratchet_id, Some(second));
}

/// A destination that advertises no ratchet is reached through its identity key, as RNS
/// does (`Destination.py` 602-611), rather than refused.
#[tokio::test]
async fn outbound_single_falls_back_to_the_identity_key_and_enforces_the_mdu() {
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

    let receipt = sender.send_single(destination, b"identity key").unwrap();
    assert_eq!(receipt.ratchet_id, None);
    let received = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("identity-key packet arrives")
        .unwrap();
    assert_eq!(received.data, b"identity key");
    assert_eq!(received.ratchet_id, None);

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

/// A sender and a receiver joined by hand-pumped interfaces, so a test sees and steers every
/// packet. The receiver has registered `name` and the sender has its announce.
struct Wired {
    sender: Endpoint,
    sender_wire: Interface,
    sender_sink: InterfaceSink,
    receiver: Endpoint,
    receiver_id: PrivateIdentity,
    receiver_wire: Interface,
    receiver_sink: InterfaceSink,
    name: DestinationName,
    destination: AddressHash,
}

async fn wired(seed: u8, ratchets: Option<&RatchetStore>) -> Wired {
    let receiver_id = PrivateIdentity::from_secret_bytes(&[seed; 64]);
    let receiver = Endpoint::new(receiver_id.clone());
    let sender = Endpoint::new(PrivateIdentity::from_secret_bytes(&[seed ^ 0xFF; 64]));
    let sender_wire = sender.attach_interface();
    let sender_sink = sender_wire.sink();
    let mut receiver_wire = receiver.attach_interface();
    let receiver_sink = receiver_wire.sink();

    let name = DestinationName::new("retinue", ["proved"]);
    let destination = name.destination_hash(receiver_id.public());
    match ratchets {
        Some(ratchets) => receiver
            .register_resource_with_ratchets(name.clone(), b"proved", ratchets)
            .unwrap(),
        None => receiver.register(name.clone(), b"proved"),
    }
    assert!(sender_sink.deliver(next(&mut receiver_wire).await));
    tokio::time::timeout(Duration::from_secs(1), sender.next_announcement())
        .await
        .expect("announce ingested")
        .unwrap();
    Wired {
        sender,
        sender_wire,
        sender_sink,
        receiver,
        receiver_id,
        receiver_wire,
        receiver_sink,
        name,
        destination,
    }
}

async fn next(wire: &mut Interface) -> Packet {
    tokio::time::timeout(Duration::from_secs(1), wire.next_outbound())
        .await
        .expect("a packet is queued")
        .expect("the interface remains live")
}

async fn silent(wire: &mut Interface) -> bool {
    tokio::time::timeout(Duration::from_millis(50), wire.next_outbound())
        .await
        .is_err()
}

/// A PROVE_ALL destination signs the packet hash and addresses the proof to its truncation;
/// the sender's receipt validates either proof form and concludes DELIVERED (RNS
/// `Transport.py` 2595-2605, `Identity.py` 946-957, `Packet.py` 495-539).
#[tokio::test]
async fn a_prove_all_destination_proves_and_the_receipt_is_delivered() {
    for (seed, implicit) in [(0x71, true), (0x72, false)] {
        let mut w = wired(seed, None).await;
        w.receiver
            .set_proof_strategy(&w.name, ProofStrategy::All)
            .unwrap();
        w.receiver.set_implicit_proofs(implicit);

        let receipt = w.sender.send_single(w.destination, b"prove me").unwrap();
        assert!(w.receiver_sink.deliver(next(&mut w.sender_wire).await));
        let received = w.receiver.accept_single().await.unwrap();
        assert_eq!(received.data, b"prove me");
        assert_eq!(received.packet_hash(), receipt.packet_hash);

        let proof = next(&mut w.receiver_wire).await;
        assert_eq!(proof.packet_type, PacketType::Proof);
        assert_eq!(proof.destination.as_slice(), &receipt.packet_hash[..16]);
        assert_eq!(proof.payload.len(), if implicit { 64 } else { 96 });
        assert!(w.sender_sink.deliver(proof));
        assert!(matches!(
            receipt.delivery().await,
            SingleDelivery::Delivered { .. }
        ));
    }
}

/// RNS's default strategy proves nothing, so the receipt times out; such a destination
/// refuses an application proof.
#[tokio::test(start_paused = true)]
async fn the_default_strategy_never_proves_and_the_receipt_times_out() {
    let mut w = wired(0x73, None).await;
    let receipt = w.sender.send_single(w.destination, b"unproved").unwrap();
    assert_eq!(receipt.timeout, Duration::from_secs(12));
    assert!(w.receiver_sink.deliver(next(&mut w.sender_wire).await));
    let received = w.receiver.accept_single().await.unwrap();
    assert!(silent(&mut w.receiver_wire).await);
    assert_eq!(
        w.receiver.prove_single(&received).unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
    assert_eq!(receipt.delivery().await, SingleDelivery::TimedOut);
}

/// Under PROVE_APP nothing is proved until the application asks; a forged proof addressed
/// to the receipt neither concludes nor evicts it.
#[tokio::test]
async fn an_app_proof_concludes_and_a_forged_one_does_not() {
    let mut w = wired(0x74, None).await;
    w.receiver
        .set_proof_strategy(&w.name, ProofStrategy::App)
        .unwrap();
    let receipt = w.sender.send_single(w.destination, b"app").unwrap();
    assert!(w.receiver_sink.deliver(next(&mut w.sender_wire).await));
    let received = w.receiver.accept_single().await.unwrap();
    assert!(silent(&mut w.receiver_wire).await);

    let stranger = PrivateIdentity::from_secret_bytes(&[0x75; 64]);
    let forged = retinue::proof::proof_packet(&stranger, &receipt.packet_hash, true);
    assert!(w.sender_sink.deliver(forged));

    w.receiver.prove_single(&received).unwrap();
    assert!(w.sender_sink.deliver(next(&mut w.receiver_wire).await));
    assert!(matches!(
        receipt.delivery().await,
        SingleDelivery::Delivered { .. }
    ));
}

/// A ratcheted destination accepts an identity-key packet unless it enforces ratchets, as
/// in RNS (`Destination.py` 502-513, `Identity.py` 868-892).
#[tokio::test]
async fn ratchet_enforcement_is_opt_in() {
    let mut ratchets = RatchetStore::new(RatchetPolicy::default()).unwrap();
    ratchets.rotate_if_due([0x76; KEY_LEN], 0.0).unwrap();
    let w = wired(0x77, Some(&ratchets)).await;
    let identity_packet = |data: &[u8]| Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: w.destination,
        context: 0,
        payload: retinue::token::encrypt_to_identity(
            w.receiver_id.public(),
            &[0x78; KEY_LEN],
            &[0x79; 16],
            data,
        ),
    };

    assert!(w.receiver_sink.deliver(identity_packet(b"identity")));
    let received = tokio::time::timeout(Duration::from_secs(1), w.receiver.accept_single())
        .await
        .expect("identity-key packet accepted by default")
        .unwrap();
    assert_eq!(received.data, b"identity");
    assert_eq!(received.ratchet_id, None);

    w.receiver.set_enforce_ratchets(&w.name, true).unwrap();
    assert!(w.receiver_sink.deliver(identity_packet(b"downgraded")));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), w.receiver.accept_single())
            .await
            .is_err(),
        "an enforcing destination drops identity-key packets"
    );

    // Ratcheted packets still arrive.
    w.sender.send_single(w.destination, b"ratcheted").unwrap();
    let mut sender_wire = w.sender_wire;
    assert!(w.receiver_sink.deliver(next(&mut sender_wire).await));
    let received = w.receiver.accept_single().await.unwrap();
    assert_eq!(received.data, b"ratcheted");
    assert_eq!(received.ratchet_id, ratchets.current_id());

    let plain = DestinationName::new("retinue", ["plain"]);
    w.receiver.register(plain.clone(), b"plain");
    assert_eq!(
        w.receiver
            .set_enforce_ratchets(&plain, true)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::InvalidInput
    );
    let unknown = DestinationName::new("retinue", ["unknown"]);
    assert_eq!(
        w.receiver
            .set_proof_strategy(&unknown, ProofStrategy::All)
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::NotFound
    );
}

/// A proof crosses an Endpoint transport hop back to the sender by the hop's reverse table
/// (RNS `Transport.py` 2104-2110, 2733-2744).
#[tokio::test]
async fn a_proof_crosses_an_endpoint_transport_hop() {
    let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x81; 64]));
    hub.enable_routing();
    let sender = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x82; 64]));
    connect(&sender, &hub, LossModel::new(1), LossModel::new(2));
    let receiver_id = PrivateIdentity::from_secret_bytes(&[0x83; 64]);
    let receiver = Endpoint::new(receiver_id.clone());
    connect(&receiver, &hub, LossModel::new(3), LossModel::new(4));

    let name = DestinationName::new("retinue", ["relayed"]);
    let destination = name.destination_hash(receiver_id.public());
    receiver.register(name.clone(), b"relayed");
    receiver
        .set_proof_strategy(&name, ProofStrategy::All)
        .unwrap();
    let announced = tokio::time::timeout(Duration::from_secs(2), sender.next_announcement())
        .await
        .expect("relayed announce arrives")
        .unwrap();
    assert_eq!(announced.destination, destination);
    assert_eq!(announced.transport, Some(hub.identity().hash()));

    let receipt = sender.send_single(destination, b"via hub").unwrap();
    let received = tokio::time::timeout(Duration::from_secs(2), receiver.accept_single())
        .await
        .expect("carried packet arrives")
        .unwrap();
    assert_eq!(received.data, b"via hub");
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(2), receipt.delivery())
            .await
            .expect("the proof comes back before the receipt times out"),
        SingleDelivery::Delivered { .. }
    ));
    assert!(hub.routing_counters().forwarded_packets >= 2);
}
