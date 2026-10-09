//! Text messages and saved contacts.

use signalman::message::{MessageEvent, MessageId, MessagePeer, QueuedReason, TextMessage};

use crate::messages::MessageStore;

use super::{DesktopState, parse_address};

impl DesktopState {
    pub fn replace_message_store(&mut self, store: MessageStore) {
        self.next_message_nonce = u64::try_from(store.log_len()).unwrap_or(u64::MAX);
        self.message_store = store;
        self.message_notice = None;
    }

    pub fn set_message_local(&mut self, local: MessagePeer) {
        self.message_local = Some(local);
        self.message_notice = None;
    }

    pub fn queue_message(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        self.queue_message_at(now);
    }

    pub fn queue_message_at(&mut self, observed_unix_ms: u64) {
        let Some(sender) = self.message_local else {
            self.message_notice =
                Some("Connect a station identity before queueing a message.".into());
            return;
        };
        let Some(destination) = parse_address(self.message_recipient.text()) else {
            self.message_notice = Some("A recipient is exactly 32 hexadecimal characters.".into());
            return;
        };
        let text = self.message_draft.text().trim();
        if text.is_empty() {
            self.message_notice = Some("Write a message before queueing it.".into());
            return;
        }
        let mut nonce = [0_u8; 32];
        nonce[..8].copy_from_slice(&self.next_message_nonce.to_be_bytes());
        nonce[8..16].copy_from_slice(&observed_unix_ms.to_be_bytes());
        let message = TextMessage::compose(
            sender,
            MessagePeer::new(destination, None),
            observed_unix_ms,
            nonce,
            text,
        );
        let id = message.id;
        match self.message_store.append(MessageEvent::OutgoingQueued {
            message: message.into(),
            reason: QueuedReason::Offline,
            observed_unix_ms,
        }) {
            Ok(_) => {
                self.next_message_nonce = self.next_message_nonce.saturating_add(1);
                self.message_draft = cambium::TextInput::default();
                self.selected_message = Some(id);
                self.message_notice = Some("Message persisted offline and queued.".into());
            }
            Err(error) => self.message_notice = Some(format!("Message was not queued: {error}")),
        }
    }

    pub fn apply_message_event(&mut self, event: MessageEvent) {
        if let Err(error) = self.message_store.append(event) {
            self.message_notice = Some(format!("Message event was not persisted: {error}"));
        }
    }

    pub fn select_message(&mut self, id: MessageId) {
        self.selected_message = Some(id);
        self.message_notice = None;
    }

    pub fn save_selected_sender(&mut self) {
        let Some(id) = self.selected_message else {
            self.message_notice = Some("Select an incoming message first.".into());
            return;
        };
        let Some(peer) = self
            .message_store
            .records()
            .find(|record| record.message.id() == id)
            .map(|record| record.message.sender())
        else {
            self.message_notice = Some("The selected message is no longer present.".into());
            return;
        };
        let petname = self.message_contact_name.text().trim();
        if petname.is_empty() {
            self.message_notice = Some("Give this contact your own name first.".into());
            return;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |duration| duration.as_millis() as u64);
        match self.message_store.save_contact(peer, petname, now) {
            Ok(()) => {
                self.message_contact_name = cambium::TextInput::default();
                self.message_notice = Some("Contact saved.".into());
            }
            Err(error) => self.message_notice = Some(format!("Contact was not saved: {error}")),
        }
    }
}
