//! Station events and send results reduced to message events.

use outrider::LxmfPayload;
use postilion::{Event, Sent};
use retinue::endpoint::PayloadMode;

use super::{
    Message, MessageError, MessageEvent, MessageId, MessagePeer, MessageStatus, QueuedReason,
    TextMessage, VOICE_WIRE_TITLE, VoiceMessage, WIRE_TITLE,
};

/// A station event reduced to Signalman's correspondence vocabulary.
///
/// Peer discovery remains a radio fact rather than a message, and a refused
/// frame remains visible without manufacturing a conversation record.
// Incoming carries the full message event inline: observations are handed straight to the
// store and the face, and a per-message Box buys nothing at this rate.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum MessageObservation {
    Incoming(MessageEvent),
    PeerAppeared {
        destination: MessagePeer,
        name: Option<String>,
    },
    Dropped(String),
}

pub fn observe_station_event(
    event: &Event,
    local: MessagePeer,
    observed_unix_ms: u64,
) -> Result<MessageObservation, MessageError> {
    match event {
        Event::Message { .. } => {
            incoming_event(event, local, observed_unix_ms).map(MessageObservation::Incoming)
        }
        Event::PeerAppeared(peer) => Ok(MessageObservation::PeerAppeared {
            destination: peer.destination.into(),
            name: peer.name.clone(),
        }),
        Event::Dropped(message) => Ok(MessageObservation::Dropped(message.clone())),
    }
}

/// Convert an authenticated Postilion receive event into a replay event.
/// The wire's sender facts must match the facts Postilion proved.
pub fn incoming_event(
    event: &Event,
    local: MessagePeer,
    observed_unix_ms: u64,
) -> Result<MessageEvent, MessageError> {
    let Event::Message {
        message_id,
        from,
        sender_identity,
        mode,
        payload,
    } = event
    else {
        return Err(MessageError::NotMessageEvent);
    };
    let message: Message = if payload.title.as_slice() == WIRE_TITLE {
        TextMessage::decode_wire(&payload.content)?.into()
    } else if payload.title.as_slice() == VOICE_WIRE_TITLE {
        VoiceMessage::decode_payload(payload)?.into()
    } else {
        return Err(MessageError::WrongWireTitle);
    };
    if message.sender().destination != *from.as_bytes()
        || message.sender().identity != Some(*sender_identity)
        || message.recipient().destination != local.destination
        || message
            .recipient()
            .identity
            .is_some_and(|identity| Some(identity) != local.identity)
    {
        return Err(MessageError::WireAuthorityMismatch);
    }
    Ok(MessageEvent::IncomingReceived {
        message,
        transport_id: *message_id,
        mode: (*mode).into(),
        observed_unix_ms,
    })
}

/// Turn a field-7 voice payload into a direct receive fact after the caller
/// supplies the identity and destination facts its transport proved.
pub fn incoming_voice_event(
    payload: &LxmfPayload,
    authenticated_sender: MessagePeer,
    local: MessagePeer,
    transport_id: [u8; 32],
    mode: PayloadMode,
    observed_unix_ms: u64,
) -> Result<MessageEvent, MessageError> {
    let message = authenticated_voice(payload, authenticated_sender, local)?;
    Ok(MessageEvent::IncomingReceived {
        message: message.into(),
        transport_id,
        mode: mode.into(),
        observed_unix_ms,
    })
}

/// The same authenticated payload after a propagation fetch. This keeps the
/// visible receipt distinct from direct delivery.
pub fn fetched_voice_event(
    payload: &LxmfPayload,
    authenticated_sender: MessagePeer,
    local: MessagePeer,
    transport_id: [u8; 32],
    mode: PayloadMode,
    observed_unix_ms: u64,
) -> Result<MessageEvent, MessageError> {
    let message = authenticated_voice(payload, authenticated_sender, local)?;
    Ok(MessageEvent::IncomingFetched {
        message: message.into(),
        transport_id,
        mode: mode.into(),
        observed_unix_ms,
    })
}

fn authenticated_voice(
    payload: &LxmfPayload,
    authenticated_sender: MessagePeer,
    local: MessagePeer,
) -> Result<VoiceMessage, MessageError> {
    let message = VoiceMessage::decode_payload(payload)?;
    if message.sender.destination != authenticated_sender.destination
        || message.sender.identity != authenticated_sender.identity
        || message.recipient.destination != local.destination
        || message
            .recipient
            .identity
            .is_some_and(|identity| Some(identity) != local.identity)
    {
        return Err(MessageError::WireAuthorityMismatch);
    }
    Ok(message)
}

pub fn sent_event(id: MessageId, sent: &Sent, observed_unix_ms: u64) -> MessageEvent {
    let status = match sent {
        Sent::HandedToRadio { message_id, mode } => MessageStatus::HandedToRadio {
            transport_id: *message_id,
            mode: (*mode).into(),
        },
        Sent::NoSuchPeer => MessageStatus::Queued(QueuedReason::WaitingForPeer),
    };
    MessageEvent::StatusChanged {
        id,
        status,
        observed_unix_ms,
    }
}
