use std::sync::Arc;
use std::time::Duration;

use outrider::{
    DeliveryAnnounce, PropagationAnnounce, PropagationBatch, PropagationCosts, PropagationStore,
    PropagationStoreLimits, prepare_propagation, receive_direct_with_stamp_cost,
    receive_submission, register_delivery, register_propagation, send_direct_stamped, serve_fetch,
    submit_propagation_with_resource_config,
};
use postilion::Event;
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceTransferConfig};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use signalman::message::{
    ApplyOutcome, MessageBook, MessageEvent, MessagePeer, MessageStatus, MessageTransport,
    QueuedReason, VoiceMessage, fetched_voice_event, incoming_event,
};
use signalman::voice::{VoiceClip, VoiceEncoding};

const NOW: f64 = 1_753_603_205.0;

fn peer(identity: &PrivateIdentity) -> MessagePeer {
    MessagePeer::new(
        *outrider::delivery_destination(identity.public()).as_bytes(),
        Some(*identity.public().ed25519_bytes()),
    )
}

#[tokio::test]
async fn direct_voice_keeps_field_seven_through_postilions_authenticated_event() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x51; 64]);
    let recipient_identity = PrivateIdentity::from_secret_bytes(&[0x52; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let recipient = Arc::new(Endpoint::new(recipient_identity.clone()));
    connect(&sender, &recipient, LossModel::new(51), LossModel::new(52));
    register_delivery(&sender, &DeliveryAnnounce::named(b"Direct voice sender")).unwrap();
    wait_for_announce(
        &recipient,
        *outrider::delivery_destination(sender_identity.public()).as_bytes(),
    )
    .await;
    register_delivery(
        &recipient,
        &DeliveryAnnounce::named(b"Direct voice recipient"),
    )
    .unwrap();
    let announce = wait_for_announce(
        &sender,
        *outrider::delivery_destination(recipient_identity.public()).as_bytes(),
    )
    .await;

    let clip = VoiceClip::encode_pcm(&vec![1_000_i16; 8_000], VoiceEncoding::Lpc10).unwrap();
    let message = VoiceMessage::compose(
        peer(&sender_identity),
        peer(&recipient_identity),
        (NOW * 1_000.0) as u64,
        [0x53; 32],
        clip.clone(),
    )
    .unwrap();
    let payload = message.encode_payload(NOW).unwrap();
    let receive = tokio::spawn({
        let recipient = Arc::clone(&recipient);
        async move {
            let accepted = recipient.accept_resource().await.unwrap();
            receive_direct_with_stamp_cost(&recipient, accepted, 64 * 1024, None)
                .await
                .unwrap()
        }
    });
    let sent = send_direct_stamped(&sender, &sender_identity, &announce, &payload, [0; 32], 0)
        .await
        .unwrap();
    let event = Event::authenticated_message(receive.await.unwrap());
    let Event::Message {
        message_id,
        payload: received_payload,
        ..
    } = &event
    else {
        unreachable!()
    };
    assert_eq!(*message_id, sent.message_id);
    let (_, received_clip) =
        outrider::voice::audio_at(received_payload, outrider::voice::FieldKey::AUDIO).unwrap();
    assert_eq!(received_clip, clip.encoded());

    let incoming = incoming_event(
        &event,
        peer(&recipient_identity),
        (NOW * 1_000.0) as u64 + 1,
    )
    .unwrap();
    let MessageEvent::IncomingReceived {
        message: recovered,
        transport_id,
        ..
    } = incoming
    else {
        unreachable!()
    };
    assert_eq!(transport_id, sent.message_id);
    assert_eq!(recovered.voice().unwrap().clip, clip);
}

async fn wait_for_announce(endpoint: &Endpoint, destination: [u8; 16]) -> PeerAnnounce {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let announce = endpoint.next_announcement().await.unwrap();
            if *announce.destination.as_bytes() == destination {
                break announce;
            }
        }
    })
    .await
    .expect("named announce arrives")
}

fn resource_config() -> ResourceTransferConfig {
    ResourceTransferConfig {
        timeout: Duration::from_secs(5),
        retry_interval: Duration::from_millis(50),
        request_window: 1,
    }
}

#[tokio::test]
async fn file_backed_voice_crosses_a_propagation_node_once_and_retains_receipts() {
    let sender_identity = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let node_identity = PrivateIdentity::from_secret_bytes(&[0x70; 64]);
    let sender = Arc::new(Endpoint::new(sender_identity.clone()));
    let recipient = Arc::new(Endpoint::new(recipient_identity.clone()));
    let node = Arc::new(Endpoint::new(node_identity));
    node.enable_routing();
    connect(&sender, &node, LossModel::new(61), LossModel::new(70));
    connect(&recipient, &node, LossModel::new(62), LossModel::new(71));

    register_delivery(&sender, &DeliveryAnnounce::named(b"Voice sender")).unwrap();
    wait_for_announce(
        &recipient,
        *outrider::delivery_destination(sender_identity.public()).as_bytes(),
    )
    .await;

    let propagation = PropagationAnnounce {
        legacy: false,
        unix_time: NOW as u64,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
        costs: PropagationCosts {
            propagation: 0,
            flexibility: 0,
            peering: 0,
        },
        metadata: Vec::new(),
    };
    register_propagation(&node, &propagation).unwrap();
    let node_destination = *outrider::propagation_destination(node.identity()).as_bytes();
    let sender_node = wait_for_announce(&sender, node_destination).await;
    let recipient_node = wait_for_announce(&recipient, node_destination).await;

    let fixture = tempfile::tempdir().unwrap();
    let pcm_path = fixture.path().join("voice-drop.pcm16le");
    let pcm = (0..8_000)
        .map(|sample| {
            let phase = sample % 80;
            if phase < 40 { 5_000_i16 } else { -5_000_i16 }
        })
        .collect::<Vec<_>>();
    let pcm_bytes = pcm
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    std::fs::write(&pcm_path, &pcm_bytes).unwrap();

    let clip = VoiceClip::encode_pcm16le_file(&pcm_path, VoiceEncoding::Lpc10, 32 * 1024).unwrap();
    let sender_peer = peer(&sender_identity);
    let recipient_peer = peer(&recipient_identity);
    let message = VoiceMessage::compose(
        sender_peer,
        recipient_peer,
        (NOW * 1000.0) as u64,
        [0x77; 32],
        clip.clone(),
    )
    .unwrap();
    let app_id = message.id;
    let mut sender_book = MessageBook::default();
    sender_book
        .apply(&MessageEvent::OutgoingQueued {
            message: message.clone().into(),
            reason: QueuedReason::Offline,
            observed_unix_ms: (NOW * 1000.0) as u64 - 1,
        })
        .unwrap();

    let payload = message.encode_payload(NOW).unwrap();
    let (audio_mode, attached_clip) =
        outrider::voice::audio_at(&payload, outrider::voice::FieldKey::AUDIO).unwrap();
    assert_eq!(audio_mode, outrider::voice::AM_CUSTOM);
    assert_eq!(attached_clip, clip.encoded());

    let prepared = prepare_propagation(
        &sender_identity,
        recipient_identity.public(),
        &payload,
        &[0x31; 32],
        &[0x41; 16],
        [0; 32],
        0,
        100_000,
    )
    .unwrap();
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![prepared.entry.clone()],
    };
    let receive_task = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let accepted = node.accept_resource().await.unwrap();
            receive_submission(&node, accepted, 0, 64 * 1024, 1)
                .await
                .unwrap()
        }
    });
    let submitted =
        submit_propagation_with_resource_config(&sender, &sender_node, &batch, resource_config())
            .await
            .unwrap();
    let received = receive_task.await.unwrap();
    assert_eq!(submitted.mode, received.mode);
    assert_eq!(submitted.transient_ids, vec![prepared.transient_id]);

    sender_book
        .apply(&MessageEvent::StatusChanged {
            id: app_id,
            status: MessageStatus::HandedToRadio {
                transport_id: prepared.message_id,
                mode: submitted.mode.into(),
            },
            observed_unix_ms: (NOW * 1000.0) as u64 + 1,
        })
        .unwrap();

    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_entries: 4,
        max_bytes: 64 * 1024,
        max_message_bytes: 32 * 1024,
        max_age: Duration::from_secs(60),
        max_per_fetch: 1,
    });
    assert_eq!(store.ingest(&received.batch, NOW).inserted, 1);
    let server = tokio::spawn({
        let node = Arc::clone(&node);
        async move {
            let mut accepted = node.accept_resource().await.unwrap();
            accepted.session.set_config(resource_config());
            serve_fetch(&node, &mut accepted, &mut store, NOW + 1.0)
                .await
                .unwrap()
        }
    });
    let fetched = outrider::fetch_propagation_with_resource_config(
        &recipient,
        &recipient_identity,
        &recipient_node,
        &[],
        1,
        NOW + 1.0,
        64 * 1024,
        32 * 1024,
        resource_config(),
    )
    .await
    .unwrap();
    let served = server.await.unwrap();
    let transfer_mode = served.message_mode.expect("node served one batch");
    assert_eq!(served.served, fetched.offered);
    assert_eq!(fetched.messages.len(), 1);

    let fetched = &fetched.messages[0];
    let event = fetched_voice_event(
        &fetched.message.payload,
        MessagePeer::new(
            fetched.message.source,
            Some(*fetched.source_identity.ed25519_bytes()),
        ),
        recipient_peer,
        fetched.message.message_id,
        transfer_mode,
        (NOW * 1000.0) as u64 + 2,
    )
    .unwrap();
    let mut recipient_book = MessageBook::default();
    assert_eq!(recipient_book.apply(&event).unwrap(), ApplyOutcome::Applied);
    let record = recipient_book.get(app_id).unwrap();
    let recovered = record.message.voice().unwrap();
    let decoded = recovered.clip.decode().unwrap();
    let facts = recovered.facts();
    let receipt = recovered.clip.receipt(transfer_mode, &decoded).unwrap();

    assert_eq!(recovered.clip.encoded(), clip.encoded());
    assert_eq!(facts.encoding, VoiceEncoding::Lpc10);
    assert_eq!(facts.sample_rate, 8_000);
    assert_eq!(facts.duration_ms, 1_000);
    assert_eq!(facts.encoded_bytes, clip.encoded().len());
    assert_eq!(decoded.sample_rate, facts.sample_rate);
    assert_eq!(decoded.decoded_duration_ms, facts.duration_ms);
    assert_eq!(decoded.pcm.len(), pcm.len());
    assert_eq!(receipt.encoding, VoiceEncoding::Lpc10);
    assert_eq!(receipt.sample_rate, 8_000);
    assert_eq!(receipt.encoded_duration_ms, 1_000);
    assert_eq!(receipt.encoded_bytes, clip.encoded().len());
    assert_eq!(receipt.transfer_mode, transfer_mode);
    assert_eq!(receipt.decoded_duration_ms, 1_000);
    assert!(matches!(
        record.status,
        MessageStatus::FetchedFromPropagationNode { mode, .. }
            if mode == MessageTransport::from(transfer_mode)
    ));
}

#[test]
fn cancellation_is_terminal_and_never_becomes_fetched() {
    let clip = VoiceClip::encode_pcm(&vec![100_i16; 1_440], VoiceEncoding::Lpc10).unwrap();
    let message = VoiceMessage::compose(
        MessagePeer::new([1; 16], Some([1; 32])),
        MessagePeer::new([2; 16], Some([2; 32])),
        100,
        [3; 32],
        clip,
    )
    .unwrap();
    let mut book = MessageBook::default();
    book.apply(&MessageEvent::OutgoingQueued {
        message: message.clone().into(),
        reason: QueuedReason::Offline,
        observed_unix_ms: 101,
    })
    .unwrap();
    book.apply(&MessageEvent::StatusChanged {
        id: message.id,
        status: MessageStatus::Cancelled,
        observed_unix_ms: 102,
    })
    .unwrap();

    let late_fetch = MessageEvent::StatusChanged {
        id: message.id,
        status: MessageStatus::FetchedFromPropagationNode {
            transport_id: [4; 32],
            mode: MessageTransport::Resource,
        },
        observed_unix_ms: 103,
    };
    assert!(book.apply(&late_fetch).is_err());
    assert_eq!(
        book.get(message.id).unwrap().status,
        MessageStatus::Cancelled
    );
}
