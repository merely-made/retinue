//! Signalman's durable message vocabulary.
//!
//! Transport reports facts; this module turns them into an append-only log and
//! a deterministic read model. It deliberately does not own persistence or a
//! contact book. Those are host adapters around this authority.

use std::collections::BTreeMap;

use outrider::LxmfPayload;
use postilion::{Event, Sent};
use retinue::endpoint::PayloadMode;
use retinue::hash::{AddressHash, full_hash};
use serde::{Deserialize, Serialize};

use crate::voice::{VoiceClip, VoiceClipError, VoiceClipFacts};

pub const WIRE_TITLE: &[u8] = b"signalman.message.v1";
pub const VOICE_WIRE_TITLE: &[u8] = b"signalman.voice.v1";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct MessageId(pub [u8; 32]);

impl MessageId {
    /// Derive an application identity before transmission. The caller supplies
    /// the nonce so composing an outgoing intent never depends on hidden I/O.
    pub fn derive(
        sender: MessagePeer,
        recipient: MessagePeer,
        authored_unix_ms: u64,
        nonce: [u8; 32],
        text: &str,
    ) -> Self {
        let mut bytes = Vec::with_capacity(16 + 16 + 8 + 32 + text.len());
        bytes.extend_from_slice(&sender.destination);
        bytes.extend_from_slice(&recipient.destination);
        bytes.extend_from_slice(&authored_unix_ms.to_be_bytes());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&(text.len() as u64).to_be_bytes());
        bytes.extend_from_slice(text.as_bytes());
        Self(full_hash(&bytes))
    }

    fn derive_voice(
        sender: MessagePeer,
        recipient: MessagePeer,
        authored_unix_ms: u64,
        nonce: [u8; 32],
        clip_hash: [u8; 32],
    ) -> Self {
        let mut bytes = Vec::with_capacity(16 + 16 + 8 + 32 + 32 + 22);
        bytes.extend_from_slice(b"signalman.voice.v1\0");
        bytes.extend_from_slice(&sender.destination);
        bytes.extend_from_slice(&recipient.destination);
        bytes.extend_from_slice(&authored_unix_ms.to_be_bytes());
        bytes.extend_from_slice(&nonce);
        bytes.extend_from_slice(&clip_hash);
        Self(full_hash(&bytes))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessagePeer {
    pub destination: [u8; 16],
    /// A proven Ed25519 key when one is known. Its presence makes a sender
    /// addressable; it does not make that sender a saved contact.
    pub identity: Option<[u8; 32]>,
}

impl MessagePeer {
    pub fn new(destination: [u8; 16], identity: Option<[u8; 32]>) -> Self {
        Self {
            destination,
            identity,
        }
    }

    pub fn address(self) -> AddressHash {
        AddressHash::from_bytes(self.destination)
    }
}

impl From<AddressHash> for MessagePeer {
    fn from(value: AddressHash) -> Self {
        Self::new(*value.as_bytes(), None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageDirection {
    Outgoing,
    Incoming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageTransport {
    Data,
    Resource,
}

impl From<PayloadMode> for MessageTransport {
    fn from(value: PayloadMode) -> Self {
        match value {
            PayloadMode::Data => Self::Data,
            PayloadMode::Resource => Self::Resource,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuedReason {
    Offline,
    WaitingForPeer,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageStatus {
    Queued(QueuedReason),
    HandedToRadio {
        transport_id: [u8; 32],
        mode: MessageTransport,
    },
    AcceptedByPropagationNode,
    FetchedFromPropagationNode {
        transport_id: [u8; 32],
        mode: MessageTransport,
    },
    ReceivedDirect {
        transport_id: [u8; 32],
        mode: MessageTransport,
    },
    Cancelled,
    Failed(String),
}

impl MessageStatus {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Queued(QueuedReason::Offline) => "offline, queued",
            Self::Queued(QueuedReason::WaitingForPeer) => "queued, waiting for peer",
            Self::HandedToRadio { .. } => "handed to radio",
            Self::AcceptedByPropagationNode => "accepted by propagation node",
            Self::FetchedFromPropagationNode { .. } => "fetched from propagation node",
            Self::ReceivedDirect { .. } => "received directly",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TextMessage {
    pub id: MessageId,
    pub sender: MessagePeer,
    pub recipient: MessagePeer,
    pub authored_unix_ms: u64,
    nonce: [u8; 32],
    pub text: String,
}

impl TextMessage {
    pub fn compose(
        sender: MessagePeer,
        recipient: MessagePeer,
        authored_unix_ms: u64,
        nonce: [u8; 32],
        text: impl Into<String>,
    ) -> Self {
        let text = text.into();
        let id = MessageId::derive(sender, recipient, authored_unix_ms, nonce, &text);
        Self {
            id,
            sender,
            recipient,
            authored_unix_ms,
            nonce,
            text,
        }
    }

    pub fn encode_wire(&self) -> Result<Vec<u8>, MessageError> {
        serde_json::to_vec(&WireEnvelope::V1(self.clone())).map_err(MessageError::Wire)
    }

    pub fn decode_wire(bytes: &[u8]) -> Result<Self, MessageError> {
        let WireEnvelope::V1(message) =
            serde_json::from_slice(bytes).map_err(MessageError::Wire)?;
        let expected = MessageId::derive(
            message.sender,
            message.recipient,
            message.authored_unix_ms,
            message.nonce,
            &message.text,
        );
        if message.id != expected {
            return Err(MessageError::InvalidMessageId);
        }
        Ok(message)
    }

    fn validate(&self) -> Result<(), MessageError> {
        let expected = MessageId::derive(
            self.sender,
            self.recipient,
            self.authored_unix_ms,
            self.nonce,
            &self.text,
        );
        if self.id != expected {
            return Err(MessageError::InvalidMessageId);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "version", content = "message")]
enum WireEnvelope {
    #[serde(rename = "1")]
    V1(TextMessage),
}

/// A Pipit clip whose routing and authorship remain outside the clip itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceMessage {
    pub id: MessageId,
    pub sender: MessagePeer,
    pub recipient: MessagePeer,
    pub authored_unix_ms: u64,
    nonce: [u8; 32],
    clip_hash: [u8; 32],
    pub clip: VoiceClip,
}

impl VoiceMessage {
    pub fn compose(
        sender: MessagePeer,
        recipient: MessagePeer,
        authored_unix_ms: u64,
        nonce: [u8; 32],
        clip: VoiceClip,
    ) -> Result<Self, MessageError> {
        clip.validate()?;
        let clip_hash = full_hash(clip.encoded());
        let id = MessageId::derive_voice(sender, recipient, authored_unix_ms, nonce, clip_hash);
        Ok(Self {
            id,
            sender,
            recipient,
            authored_unix_ms,
            nonce,
            clip_hash,
            clip,
        })
    }

    /// Build one LXMF payload without duplicating the clip in Signalman's
    /// metadata body. The clip bytes live only in audio field 7.
    pub fn encode_payload(&self, lxmf_timestamp: f64) -> Result<LxmfPayload, MessageError> {
        self.validate()?;
        let wire = VoiceWireEnvelope::V1(VoiceWireV1 {
            id: self.id,
            sender: self.sender,
            recipient: self.recipient,
            authored_unix_ms: self.authored_unix_ms,
            nonce: self.nonce,
            clip_hash: self.clip_hash,
        });
        let mut payload = LxmfPayload::text(
            lxmf_timestamp,
            VOICE_WIRE_TITLE,
            serde_json::to_vec(&wire).map_err(MessageError::Wire)?,
        );
        self.clip.attach(&mut payload)?;
        Ok(payload)
    }

    pub fn decode_payload(payload: &LxmfPayload) -> Result<Self, MessageError> {
        if payload.title.as_slice() != VOICE_WIRE_TITLE {
            return Err(MessageError::WrongVoiceWireTitle);
        }
        let VoiceWireEnvelope::V1(wire) =
            serde_json::from_slice(&payload.content).map_err(MessageError::Wire)?;
        let clip = VoiceClip::from_payload(payload)?;
        let message = Self {
            id: wire.id,
            sender: wire.sender,
            recipient: wire.recipient,
            authored_unix_ms: wire.authored_unix_ms,
            nonce: wire.nonce,
            clip_hash: wire.clip_hash,
            clip,
        };
        message.validate()?;
        Ok(message)
    }

    pub fn facts(&self) -> VoiceClipFacts {
        self.clip.facts()
    }

    fn validate(&self) -> Result<(), MessageError> {
        self.clip.validate()?;
        let clip_hash = full_hash(self.clip.encoded());
        if self.clip_hash != clip_hash
            || self.id
                != MessageId::derive_voice(
                    self.sender,
                    self.recipient,
                    self.authored_unix_ms,
                    self.nonce,
                    clip_hash,
                )
        {
            return Err(MessageError::InvalidMessageId);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "version", content = "message")]
enum VoiceWireEnvelope {
    #[serde(rename = "1")]
    V1(VoiceWireV1),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct VoiceWireV1 {
    id: MessageId,
    sender: MessagePeer,
    recipient: MessagePeer,
    authored_unix_ms: u64,
    nonce: [u8; 32],
    clip_hash: [u8; 32],
}

/// Text and voice share one event log without changing the serialized shape
/// of the text records S4 already wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Text(TextMessage),
    Voice(VoiceMessage),
}

impl Message {
    pub fn id(&self) -> MessageId {
        match self {
            Self::Text(message) => message.id,
            Self::Voice(message) => message.id,
        }
    }

    pub fn sender(&self) -> MessagePeer {
        match self {
            Self::Text(message) => message.sender,
            Self::Voice(message) => message.sender,
        }
    }

    pub fn recipient(&self) -> MessagePeer {
        match self {
            Self::Text(message) => message.recipient,
            Self::Voice(message) => message.recipient,
        }
    }

    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(message) => Some(&message.text),
            Self::Voice(_) => None,
        }
    }

    pub fn voice(&self) -> Option<&VoiceMessage> {
        match self {
            Self::Text(_) => None,
            Self::Voice(message) => Some(message),
        }
    }

    fn validate(&self) -> Result<(), MessageError> {
        match self {
            Self::Text(message) => message.validate(),
            Self::Voice(message) => message.validate(),
        }
    }
}

impl From<TextMessage> for Message {
    fn from(value: TextMessage) -> Self {
        Self::Text(value)
    }
}

impl From<VoiceMessage> for Message {
    fn from(value: VoiceMessage) -> Self {
        Self::Voice(value)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageEvent {
    OutgoingQueued {
        message: Message,
        reason: QueuedReason,
        observed_unix_ms: u64,
    },
    IncomingReceived {
        message: Message,
        transport_id: [u8; 32],
        mode: MessageTransport,
        observed_unix_ms: u64,
    },
    IncomingFetched {
        message: Message,
        transport_id: [u8; 32],
        mode: MessageTransport,
        observed_unix_ms: u64,
    },
    StatusChanged {
        id: MessageId,
        status: MessageStatus,
        observed_unix_ms: u64,
    },
}

impl MessageEvent {
    pub fn message_id(&self) -> MessageId {
        match self {
            Self::OutgoingQueued { message, .. }
            | Self::IncomingReceived { message, .. }
            | Self::IncomingFetched { message, .. } => message.id(),
            Self::StatusChanged { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageRecord {
    pub message: Message,
    pub direction: MessageDirection,
    pub status: MessageStatus,
    pub observed_unix_ms: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MessageBook {
    messages: BTreeMap<MessageId, MessageRecord>,
    order: Vec<MessageId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyOutcome {
    Applied,
    Duplicate,
}

impl MessageBook {
    pub fn replay<'a>(
        events: impl IntoIterator<Item = &'a MessageEvent>,
    ) -> Result<Self, MessageError> {
        let mut book = Self::default();
        for event in events {
            book.apply(event)?;
        }
        Ok(book)
    }

    pub fn apply(&mut self, event: &MessageEvent) -> Result<ApplyOutcome, MessageError> {
        match event {
            MessageEvent::OutgoingQueued {
                message,
                reason,
                observed_unix_ms,
            } => self.insert(
                message,
                MessageDirection::Outgoing,
                MessageStatus::Queued(reason.clone()),
                *observed_unix_ms,
            ),
            MessageEvent::IncomingReceived {
                message,
                transport_id,
                mode,
                observed_unix_ms,
            } => self.insert(
                message,
                MessageDirection::Incoming,
                MessageStatus::ReceivedDirect {
                    transport_id: *transport_id,
                    mode: *mode,
                },
                *observed_unix_ms,
            ),
            MessageEvent::IncomingFetched {
                message,
                transport_id,
                mode,
                observed_unix_ms,
            } => self.insert(
                message,
                MessageDirection::Incoming,
                MessageStatus::FetchedFromPropagationNode {
                    transport_id: *transport_id,
                    mode: *mode,
                },
                *observed_unix_ms,
            ),
            MessageEvent::StatusChanged {
                id,
                status,
                observed_unix_ms,
            } => {
                let record = self
                    .messages
                    .get_mut(id)
                    .ok_or(MessageError::UnknownMessage(*id))?;
                if &record.status == status && record.observed_unix_ms == *observed_unix_ms {
                    return Ok(ApplyOutcome::Duplicate);
                }
                if record.direction == MessageDirection::Incoming {
                    return Err(MessageError::IncomingStatusChange(*id));
                }
                if !valid_transition(&record.status, status) {
                    return Err(MessageError::InvalidTransition {
                        id: *id,
                        from: record.status.clone(),
                        to: status.clone(),
                    });
                }
                record.status = status.clone();
                record.observed_unix_ms = *observed_unix_ms;
                Ok(ApplyOutcome::Applied)
            }
        }
    }

    fn insert(
        &mut self,
        message: &Message,
        direction: MessageDirection,
        status: MessageStatus,
        observed_unix_ms: u64,
    ) -> Result<ApplyOutcome, MessageError> {
        message.validate()?;
        let id = message.id();
        if let Some(existing) = self.messages.get(&id) {
            if existing.message == *message && existing.direction == direction {
                return Ok(ApplyOutcome::Duplicate);
            }
            return Err(MessageError::ConflictingMessage(id));
        }
        self.messages.insert(
            id,
            MessageRecord {
                message: message.clone(),
                direction,
                status,
                observed_unix_ms,
            },
        );
        self.order.push(id);
        Ok(ApplyOutcome::Applied)
    }

    pub fn get(&self, id: MessageId) -> Option<&MessageRecord> {
        self.messages.get(&id)
    }

    pub fn iter(&self) -> impl DoubleEndedIterator<Item = &MessageRecord> {
        self.order.iter().filter_map(|id| self.messages.get(id))
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }
}

fn valid_transition(from: &MessageStatus, to: &MessageStatus) -> bool {
    use MessageStatus::*;
    matches!(
        (from, to),
        (Queued(_), Queued(_))
            | (Queued(_), HandedToRadio { .. })
            | (Queued(_), AcceptedByPropagationNode)
            | (Queued(_), Cancelled)
            | (Queued(_), Failed(_))
            | (HandedToRadio { .. }, AcceptedByPropagationNode)
            | (HandedToRadio { .. }, FetchedFromPropagationNode { .. })
            | (HandedToRadio { .. }, Cancelled)
            | (HandedToRadio { .. }, Failed(_))
            | (AcceptedByPropagationNode, FetchedFromPropagationNode { .. })
            | (AcceptedByPropagationNode, Cancelled)
            | (AcceptedByPropagationNode, Failed(_))
    )
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

#[derive(Debug, thiserror::Error)]
pub enum MessageError {
    #[error("message wire data is invalid: {0}")]
    Wire(serde_json::Error),
    #[error("the Postilion event is not a message")]
    NotMessageEvent,
    #[error("the message does not use Signalman's wire title")]
    WrongWireTitle,
    #[error("the message does not use Signalman's voice wire title")]
    WrongVoiceWireTitle,
    #[error(transparent)]
    Voice(#[from] VoiceClipError),
    #[error("the message envelope disagrees with authenticated transport facts")]
    WireAuthorityMismatch,
    #[error("the message identity does not match its authored fields")]
    InvalidMessageId,
    #[error("message {0:?} has no queued or received record")]
    UnknownMessage(MessageId),
    #[error("message {0:?} conflicts with an existing object")]
    ConflictingMessage(MessageId),
    #[error("incoming message {0:?} cannot take an outgoing status")]
    IncomingStatusChange(MessageId),
    #[error("message {id:?} cannot move from {from:?} to {to:?}")]
    InvalidTransition {
        id: MessageId,
        from: MessageStatus,
        to: MessageStatus,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

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
            VoiceClip::encode_pcm(&vec![1_000_i16; 1_440], crate::voice::VoiceEncoding::Lpc10)
                .unwrap();
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
}
