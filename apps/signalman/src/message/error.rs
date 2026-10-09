use super::{MessageId, MessageStatus};
use crate::voice::VoiceClipError;

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
