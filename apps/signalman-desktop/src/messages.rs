//! Desktop persistence adapters for Signalman's message authority.

use std::path::Path;

use codicil::Codicil;
use gaz::{Contact, ContactBook, ContactKey, Endpoint, EndpointKind, PersonaScope};
use muniment::{Backend, JsonSlots, MemoryBackend, RedbBackend, StoreError};
use signalman::message::{ApplyOutcome, MessageBook, MessageEvent, MessagePeer, MessageRecord};

const LOG_SLOT: &str = "signalman/messages/v1/log";
const CONTACTS_SLOT: &str = "signalman/messages/v1/contacts";

type DynBackend = Box<dyn Backend + Send + Sync>;

pub struct MessageStore {
    slots: JsonSlots<DynBackend>,
    log: Codicil<MessageEvent>,
    book: MessageBook,
    contacts: ContactBook,
}

impl MessageStore {
    pub fn memory(scope: impl Into<String>) -> Self {
        Self::from_backend(Box::new(MemoryBackend::new()), scope)
            .expect("a fresh memory message store loads")
    }

    pub fn open(
        path: impl AsRef<Path>,
        scope: impl Into<String>,
    ) -> Result<Self, MessageStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        Self::from_backend(Box::new(RedbBackend::open(path)?), scope)
    }

    fn from_backend(
        backend: DynBackend,
        scope: impl Into<String>,
    ) -> Result<Self, MessageStoreError> {
        let expected = PersonaScope::new(scope);
        let slots = JsonSlots::new(backend);
        let (log, contacts) = pollster::block_on(async {
            let log = Codicil::<MessageEvent>::load(&slots, LOG_SLOT).await?;
            let contacts = slots
                .load::<ContactBook>(CONTACTS_SLOT)
                .await?
                .unwrap_or_else(|| ContactBook::new(expected.clone()));
            Ok::<_, StoreError>((log, contacts))
        })?;
        contacts.verify_scope(&expected)?;
        let book = MessageBook::replay(log.entries())?;
        Ok(Self {
            slots,
            log,
            book,
            contacts,
        })
    }

    /// Validate and persist a candidate log before exposing its materialized
    /// state. A failed store write leaves both in-memory objects unchanged.
    pub fn append(&mut self, event: MessageEvent) -> Result<ApplyOutcome, MessageStoreError> {
        let mut candidate_book = self.book.clone();
        let outcome = candidate_book.apply(&event)?;
        if outcome == ApplyOutcome::Duplicate {
            return Ok(outcome);
        }
        let mut candidate_log = self.log.clone();
        candidate_log.append(event);
        pollster::block_on(candidate_log.save(&self.slots, LOG_SLOT))?;
        self.log = candidate_log;
        self.book = candidate_book;
        Ok(outcome)
    }

    pub fn records(&self) -> impl DoubleEndedIterator<Item = &MessageRecord> {
        self.book.iter()
    }

    pub fn len(&self) -> usize {
        self.book.len()
    }

    pub fn log_len(&self) -> usize {
        self.log.len()
    }

    pub fn contacts(&self) -> &ContactBook {
        &self.contacts
    }

    pub fn contact_name(&self, peer: MessagePeer) -> Option<&str> {
        let key = ContactKey::from_bytes(peer.identity?);
        self.contacts
            .by_key(&key)
            .map(|contact| contact.petname.as_str())
    }

    /// Saving a sender is an explicit owner action. Merely receiving a proven
    /// key never calls this method.
    pub fn save_contact(
        &mut self,
        peer: MessagePeer,
        petname: impl Into<String>,
        now_ms: u64,
    ) -> Result<(), MessageStoreError> {
        let identity = peer.identity.ok_or(MessageStoreError::UnprovenContact)?;
        let key = ContactKey::from_bytes(identity);
        let address = hex_bytes(&peer.destination);
        let mut contact = self.contacts.by_key(&key).cloned().unwrap_or_else(|| {
            Contact::new(petname, key).with_endpoint(Endpoint::new(
                EndpointKind::Other("reticulum".into()),
                address,
            ))
        });
        contact.mark_contacted(now_ms);
        let mut candidate = self.contacts.clone();
        candidate.insert(contact);
        pollster::block_on(self.slots.save(CONTACTS_SLOT, &candidate))?;
        self.contacts = candidate;
        Ok(())
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[derive(Debug, thiserror::Error)]
pub enum MessageStoreError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Replay(#[from] signalman::message::MessageError),
    #[error(transparent)]
    Scope(#[from] gaz::ScopeMismatch),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("a contact requires an authenticated sender key")]
    UnprovenContact,
}

#[cfg(test)]
mod tests {
    use super::*;
    use signalman::message::{QueuedReason, TextMessage};

    #[test]
    fn unknown_authenticated_sender_remains_outside_gaz_until_owner_saves_it() {
        let local = MessagePeer::new([1; 16], Some([1; 32]));
        let unknown = MessagePeer::new([2; 16], Some([2; 32]));
        let message = TextMessage::compose(unknown, local, 10, [3; 32], "hello");
        let mut store = MessageStore::memory("local");
        store
            .append(MessageEvent::IncomingReceived {
                message,
                transport_id: [4; 32],
                mode: signalman::message::MessageTransport::Data,
                observed_unix_ms: 11,
            })
            .unwrap();
        assert_eq!(store.len(), 1);
        assert!(store.contacts().is_empty());
        assert_eq!(store.contact_name(unknown), None);

        store.save_contact(unknown, "Alice", 12).unwrap();
        assert_eq!(store.contact_name(unknown), Some("Alice"));
    }

    #[test]
    fn duplicate_event_does_not_grow_codicil() {
        let message = TextMessage::compose(
            MessagePeer::new([1; 16], Some([1; 32])),
            MessagePeer::new([2; 16], None),
            10,
            [3; 32],
            "hello",
        );
        let event = MessageEvent::OutgoingQueued {
            message,
            reason: QueuedReason::Offline,
            observed_unix_ms: 11,
        };
        let mut store = MessageStore::memory("local");
        assert_eq!(store.append(event.clone()).unwrap(), ApplyOutcome::Applied);
        assert_eq!(store.append(event).unwrap(), ApplyOutcome::Duplicate);
        assert_eq!(store.log_len(), 1);
    }

    #[test]
    fn redb_reopen_replays_the_exact_codicil_material() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("messages.redb");
        let message = TextMessage::compose(
            MessagePeer::new([1; 16], Some([1; 32])),
            MessagePeer::new([2; 16], None),
            10,
            [3; 32],
            "survives restart",
        );
        let id = message.id;
        {
            let mut first = MessageStore::open(&path, "local").unwrap();
            first
                .append(MessageEvent::OutgoingQueued {
                    message,
                    reason: QueuedReason::Offline,
                    observed_unix_ms: 11,
                })
                .unwrap();
        }
        let reopened = MessageStore::open(&path, "local").unwrap();
        let record = reopened.records().next().unwrap();
        assert_eq!(record.message.id, id);
        assert_eq!(record.message.text, "survives restart");
        assert_eq!(record.status.label(), "offline, queued");
        assert_eq!(reopened.log_len(), 1);
    }
}
