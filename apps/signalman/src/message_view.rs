//! Read-only conversation presentation over Signalman's retained message facts.
//!
//! The borrowed record remains authoritative, including transport IDs, mode,
//! direction, authorship and content. This projection does not infer a read
//! receipt, privacy guarantee or another protocol from a delivery observation.

use comms::{DeliveryQueueReason, DeliveryStatus};

use crate::message::{MessageRecord, MessageStatus, QueuedReason};

/// Presentation facts for an existing journal record; never a second history.
#[derive(Debug)]
pub struct MessageView<'a> {
    pub record: &'a MessageRecord,
    pub delivery: DeliveryStatus,
}

impl<'a> MessageView<'a> {
    pub fn new(record: &'a MessageRecord) -> Self {
        let delivery = match &record.status {
            MessageStatus::Queued(reason) => DeliveryStatus::Queued(match reason {
                QueuedReason::Offline => DeliveryQueueReason::Offline,
                QueuedReason::ReadyForCarriage => DeliveryQueueReason::ReadyForCarriage,
                QueuedReason::WaitingForPeer => DeliveryQueueReason::WaitingForPeer,
            }),
            MessageStatus::HandedToRadio { .. } => DeliveryStatus::HandedToRadio,
            MessageStatus::AcceptedByPropagationNode => DeliveryStatus::AcceptedByPropagationNode,
            MessageStatus::FetchedFromPropagationNode { .. } => {
                DeliveryStatus::FetchedFromPropagationNode
            }
            MessageStatus::ReceivedDirect { .. } => DeliveryStatus::ReceivedDirect,
            MessageStatus::Cancelled => DeliveryStatus::Cancelled,
            MessageStatus::Failed(detail) => DeliveryStatus::Failed {
                detail: detail.clone(),
            },
        };
        Self { record, delivery }
    }

    /// Status text used by the existing Messages face, retaining failure detail.
    pub fn delivery_text(&self) -> String {
        match &self.delivery {
            DeliveryStatus::Failed { detail } => format!("{}: {detail}", self.delivery.label()),
            other => other.label().to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{MessageBook, MessageEvent, MessagePeer, MessageTransport, TextMessage};

    fn message() -> TextMessage {
        TextMessage::compose(
            MessagePeer::new([1; 16], Some([1; 32])),
            MessagePeer::new([2; 16], Some([2; 32])),
            100,
            [3; 32],
            "hello",
        )
    }

    #[test]
    fn outgoing_replay_preserves_queue_carriage_acceptance_and_terminal_facts() {
        let message = message();
        let mut events = vec![MessageEvent::OutgoingQueued {
            message: message.clone().into(),
            reason: QueuedReason::Offline,
            observed_unix_ms: 101,
        }];
        let cases = [
            (
                MessageStatus::Queued(QueuedReason::Offline),
                "offline, queued",
            ),
            (
                MessageStatus::Queued(QueuedReason::ReadyForCarriage),
                "queued for station",
            ),
            (
                MessageStatus::Queued(QueuedReason::WaitingForPeer),
                "queued, waiting for peer",
            ),
            (
                MessageStatus::HandedToRadio {
                    transport_id: [4; 32],
                    mode: MessageTransport::Resource,
                },
                "handed to radio",
            ),
            (
                MessageStatus::AcceptedByPropagationNode,
                "accepted by propagation node",
            ),
            (
                MessageStatus::Failed("radio closed".into()),
                "failed: radio closed",
            ),
            (MessageStatus::Cancelled, "cancelled"),
        ];
        for (index, (status, expected)) in cases.into_iter().enumerate() {
            events.push(MessageEvent::StatusChanged {
                id: message.id,
                status: status.clone(),
                observed_unix_ms: 102 + index as u64,
            });
            let book = MessageBook::replay(&events).unwrap();
            assert_eq!(book.len(), 1);
            let record = book.iter().next().unwrap();
            let view = MessageView::new(record);
            assert!(std::ptr::eq(view.record, record));
            assert_eq!(view.record.status, status);
            assert_eq!(view.record.message.id(), message.id);
            assert_eq!(view.delivery_text(), expected);
            events.pop();
        }
    }

    #[test]
    fn incoming_replay_distinguishes_direct_receipt_from_fetch_without_duplication() {
        let message = message();
        for (event, expected) in [
            (
                MessageEvent::IncomingReceived {
                    message: message.clone().into(),
                    transport_id: [8; 32],
                    mode: MessageTransport::Data,
                    observed_unix_ms: 110,
                },
                DeliveryStatus::ReceivedDirect,
            ),
            (
                MessageEvent::IncomingFetched {
                    message: message.clone().into(),
                    transport_id: [9; 32],
                    mode: MessageTransport::Resource,
                    observed_unix_ms: 110,
                },
                DeliveryStatus::FetchedFromPropagationNode,
            ),
        ] {
            let book = MessageBook::replay([&event, &event]).unwrap();
            assert_eq!(book.len(), 1);
            let view = MessageView::new(book.iter().next().unwrap());
            assert_eq!(view.delivery, expected);
            assert_eq!(view.record.observed_unix_ms, 110);
        }
    }
}
