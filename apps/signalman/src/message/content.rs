//! Authored text and voice messages, their identities, and their wire forms.

use outrider::LxmfPayload;
use retinue::hash::{AddressHash, full_hash};
use serde::{Deserialize, Serialize};

use super::MessageError;
use crate::voice::{VoiceClip, VoiceClipFacts};

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

    /// Build the complete LXMF payload for the station boundary.
    ///
    /// Both forms validate their authored identity before carriage. Voice
    /// retains its encoded clip only in LXMF audio field 7; text remains the
    /// versioned body S4 already persists.
    pub fn encode_payload(&self, lxmf_timestamp: f64) -> Result<LxmfPayload, MessageError> {
        match self {
            Self::Text(message) => {
                message.validate()?;
                Ok(LxmfPayload::text(
                    lxmf_timestamp,
                    WIRE_TITLE,
                    message.encode_wire()?,
                ))
            }
            Self::Voice(message) => message.encode_payload(lxmf_timestamp),
        }
    }

    pub(super) fn validate(&self) -> Result<(), MessageError> {
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
