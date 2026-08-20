use std::path::Path;

use signalman::message::{
    MessageDirection, MessageEvent, MessagePeer, MessageStatus, MessageTransport, TextMessage,
    sent_event,
};
use signalman_desktop::DesktopState;
use signalman_desktop::station::StationEvent;

fn peer(byte: u8) -> MessagePeer {
    MessagePeer::new([byte; 16], Some([byte; 32]))
}

#[test]
fn connected_station_drains_one_persisted_intent_and_records_radio_acceptance() {
    let local = peer(1);
    let mut state = DesktopState::new(Path::new("missing-catalog.toml"));
    state.apply_station_event_at(StationEvent::Connected { local }, 10);
    state.message_recipient = cambium::TextInput::new("02020202020202020202020202020202");
    state.message_draft = cambium::TextInput::new("Meet by the north gate");
    state.queue_message_at(11);

    let request = state.take_station_request().expect("station request");
    let id = request.id();
    assert_eq!(request.message.sender(), local);
    assert_eq!(request.message.recipient().destination, [2; 16]);
    assert!(state.take_station_request().is_none());

    state.apply_station_event_at(
        StationEvent::Message(Box::new(sent_event(
            id,
            &postilion::Sent::HandedToRadio {
                message_id: [9; 32],
                mode: retinue::endpoint::PayloadMode::Data,
            },
            12,
        ))),
        12,
    );
    let record = state.message_store.record(id).unwrap();
    assert_eq!(
        record.status,
        MessageStatus::HandedToRadio {
            transport_id: [9; 32],
            mode: MessageTransport::Data
        }
    );
    assert!(state.take_station_request().is_none());
}

#[test]
fn station_receive_and_disconnect_remain_distinct_durable_facts() {
    let local = peer(1);
    let remote = peer(2);
    let incoming = TextMessage::compose(remote, local, 20, [3; 32], "arrived");
    let id = incoming.id;
    let mut state = DesktopState::new(Path::new("missing-catalog.toml"));
    state.apply_station_event_at(StationEvent::Connected { local }, 21);
    state.apply_station_event_at(
        StationEvent::Message(Box::new(MessageEvent::IncomingReceived {
            message: incoming.into(),
            transport_id: [4; 32],
            mode: MessageTransport::Resource,
            observed_unix_ms: 22,
        })),
        22,
    );

    let record = state.message_store.record(id).unwrap();
    assert_eq!(record.direction, MessageDirection::Incoming);
    assert_eq!(
        record.status,
        MessageStatus::ReceivedDirect {
            transport_id: [4; 32],
            mode: MessageTransport::Resource
        }
    );
    assert_eq!(state.selected_message, Some(id));

    state.apply_station_event_at(StationEvent::Disconnected("radio closed".into()), 23);
    assert_eq!(state.message_local, None);
    assert_eq!(
        state.message_store.record(id).unwrap().message.text(),
        Some("arrived")
    );
    assert!(
        state
            .message_notice
            .as_deref()
            .is_some_and(|notice| notice.contains("radio closed"))
    );
}
