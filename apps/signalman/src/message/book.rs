//! Delivery status, the replayable event log, and its read model.

use std::collections::BTreeMap;

use retinue::endpoint::PayloadMode;
use serde::{Deserialize, Serialize};

use super::{Message, MessageError, MessageId};

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
    ReadyForCarriage,
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
            Self::Queued(QueuedReason::ReadyForCarriage) => "queued for station",
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
