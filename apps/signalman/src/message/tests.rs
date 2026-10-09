use outrider::LxmfPayload;
use postilion::Event;
use retinue::endpoint::PayloadMode;

use super::*;
use crate::voice::VoiceClip;

fn peer(byte: u8) -> MessagePeer {
    MessagePeer::new([byte; 16], Some([byte; 32]))
}

#[test]
fn restart_replay_is_exact_and_duplicate_receive_is_idempotent() {
    let message = TextMessage::compose(peer(1), peer(2), 100, [3; 32], "hello");
    let events = vec![
        MessageEvent::OutgoingQueued {
            message: message.clone().into(),
            reason: QueuedReason::Offline,
            observed_unix_ms: 101,
        },
        MessageEvent::StatusChanged {
            id: message.id,
            status: MessageStatus::HandedToRadio {
                transport_id: [9; 32],
                mode: MessageTransport::Data,
            },
            observed_unix_ms: 102,
        },
    ];
    let first = MessageBook::replay(&events).unwrap();
    let second = MessageBook::replay(&events).unwrap();
    assert_eq!(first, second);

    let incoming = TextMessage::compose(peer(2), peer(1), 103, [4; 32], "back");
    let duplicate = MessageEvent::IncomingReceived {
        message: incoming.into(),
        transport_id: [8; 32],
        mode: MessageTransport::Resource,
        observed_unix_ms: 104,
    };
    let mut book = first;
    assert_eq!(book.apply(&duplicate).unwrap(), ApplyOutcome::Applied);
    assert_eq!(book.apply(&duplicate).unwrap(), ApplyOutcome::Duplicate);
    assert_eq!(book.len(), 2);
    assert_eq!(
        book.iter()
            .filter_map(|record| record.message.text())
            .collect::<Vec<_>>(),
        vec!["hello", "back"]
    );
}

#[test]
fn status_words_keep_transport_facts_distinct() {
    assert_eq!(
        MessageStatus::Queued(QueuedReason::ReadyForCarriage).label(),
        "queued for station"
    );
    assert_ne!(
        MessageStatus::Queued(QueuedReason::Offline).label(),
        MessageStatus::HandedToRadio {
            transport_id: [0; 32],
            mode: MessageTransport::Data,
        }
        .label()
    );
    assert_ne!(
        MessageStatus::AcceptedByPropagationNode.label(),
        MessageStatus::FetchedFromPropagationNode {
            transport_id: [1; 32],
            mode: MessageTransport::Resource,
        }
        .label()
    );
    assert_eq!(
        MessageStatus::Failed("radio closed".into()).label(),
        "failed"
    );
}

#[test]
fn authenticated_sender_must_match_the_wire_envelope() {
    let local = peer(1);
    let remote = peer(2);
    let message = TextMessage::compose(remote, local, 100, [7; 32], "hello");
    let event = Event::Message {
        message_id: [8; 32],
        from: remote.address(),
        sender_identity: remote.identity.unwrap(),
        mode: PayloadMode::Data,
        payload: LxmfPayload::text(1.0, WIRE_TITLE, message.encode_wire().unwrap()),
    };
    let observed = incoming_event(&event, local, 110).unwrap();
    let mut book = MessageBook::default();
    assert_eq!(book.apply(&observed).unwrap(), ApplyOutcome::Applied);

    let forged_sender = MessagePeer::new(remote.destination, Some([9; 32]));
    let forged = TextMessage::compose(forged_sender, local, 100, [7; 32], "hello");
    let forged = Event::Message {
        message_id: [8; 32],
        from: remote.address(),
        sender_identity: remote.identity.unwrap(),
        mode: PayloadMode::Data,
        payload: LxmfPayload::text(1.0, WIRE_TITLE, forged.encode_wire().unwrap()),
    };
    assert!(matches!(
        incoming_event(&forged, local, 110),
        Err(MessageError::WireAuthorityMismatch)
    ));
}

#[test]
fn station_payload_keeps_text_and_voice_in_their_owned_lxmf_fields() {
    let text: Message = TextMessage::compose(peer(1), peer(2), 100, [3; 32], "hello").into();
    let text_payload = text.encode_payload(1.5).unwrap();
    assert_eq!(text_payload.title, WIRE_TITLE);
    assert_eq!(
        TextMessage::decode_wire(&text_payload.content)
            .unwrap()
            .text,
        "hello"
    );

    let clip =
        VoiceClip::encode_pcm(&vec![1_000_i16; 1_440], crate::voice::VoiceEncoding::Lpc10).unwrap();
    let voice: Message = VoiceMessage::compose(peer(1), peer(2), 100, [4; 32], clip.clone())
        .unwrap()
        .into();
    let voice_payload = voice.encode_payload(2.5).unwrap();
    assert_eq!(voice_payload.title, VOICE_WIRE_TITLE);
    assert_eq!(
        VoiceMessage::decode_payload(&voice_payload).unwrap().clip,
        clip
    );
}

#[test]
fn text_and_voice_share_one_replayable_log_without_retagging_text() {
    let text = TextMessage::compose(peer(1), peer(2), 100, [3; 32], "hello");
    let text_event = MessageEvent::OutgoingQueued {
        message: text.into(),
        reason: QueuedReason::Offline,
        observed_unix_ms: 101,
    };
    let text_json = serde_json::to_string(&text_event).unwrap();
    assert!(text_json.contains("\"text\":\"hello\""));
    assert!(!text_json.contains("\"Text\""));

    let clip =
        VoiceClip::encode_pcm(&vec![1_000_i16; 1_440], crate::voice::VoiceEncoding::Lpc10).unwrap();
    let voice = VoiceMessage::compose(peer(1), peer(2), 102, [4; 32], clip).unwrap();
    let voice_id = voice.id;
    let voice_event = MessageEvent::OutgoingQueued {
        message: voice.into(),
        reason: QueuedReason::Offline,
        observed_unix_ms: 103,
    };
    let persisted = serde_json::to_vec(&voice_event).unwrap();
    let restored: MessageEvent = serde_json::from_slice(&persisted).unwrap();
    let book = MessageBook::replay([&text_event, &restored]).unwrap();

    assert_eq!(book.len(), 2);
    assert!(book.get(voice_id).unwrap().message.voice().is_some());
}
