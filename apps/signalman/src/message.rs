//! Signalman's durable message vocabulary.
//!
//! Transport reports facts; this module turns them into an append-only log and
//! a deterministic read model. It deliberately does not own persistence or a
//! contact book. Those are host adapters around this authority.

mod book;
mod content;
mod error;
mod observe;
#[cfg(test)]
mod tests;

pub use book::{
    ApplyOutcome, MessageBook, MessageDirection, MessageEvent, MessageRecord, MessageStatus,
    MessageTransport, QueuedReason,
};
pub use content::{
    Message, MessageId, MessagePeer, TextMessage, VOICE_WIRE_TITLE, VoiceMessage, WIRE_TITLE,
};
pub use error::MessageError;
pub use observe::{
    MessageObservation, fetched_voice_event, incoming_event, incoming_voice_event,
    observe_station_event, sent_event,
};
