//! Persistence and cleaning of known destinations and received ratchets
//! (RNS `Identity.py` 100-349, 410-443, 484-508).

use std::io;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::address_book::{CleanReceipt, RestoreReceipt, Retention};
use crate::hash::AddressHash;

use super::runtime::Endpoint;
use super::shared::Shared;

/// How often known destinations are cleaned and persisted
/// (RNS `Transport.known_destinations_interval`).
pub(super) const KNOWN_DESTINATIONS_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The book ticks in wall-clock milliseconds, so persisted ticks stay meaningful across
/// restarts, as RNS's `time.time()` stamps do.
const RETENTION: Retention = Retention::rns(1_000);

/// The host's durable store for the address book's signed snapshot.
pub(super) type BookPersistence = alloc::boxed::Box<dyn FnMut(&[u8]) -> io::Result<()> + Send>;

/// The address book's clock: wall-clock milliseconds since the Unix epoch.
pub(super) fn book_clock_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            since.as_millis().min(u128::from(u64::MAX)) as u64
        })
}

impl Shared {
    /// Note a use of `destination`, as RNS's `Identity.recall` does, so cleaning keeps it.
    pub(super) fn mark_destination_used(&self, destination: AddressHash) {
        self.address_book
            .lock()
            .unwrap()
            .mark_used(destination, book_clock_ms());
    }

    /// Clean the book, then hand its signed snapshot to the host's hook, if one is set
    /// (`Identity.py` 285-349). Peers with a live path or link are kept.
    pub(super) fn clean_and_persist_book(&self) -> io::Result<CleanReceipt> {
        let in_use = self.destinations_in_use();
        let mut persistence = self.book_persistence.lock().unwrap();
        let (receipt, snapshot) = {
            let mut book = self.address_book.lock().unwrap();
            let receipt = book.clean(book_clock_ms(), &RETENTION, |d| in_use.contains(&d));
            let snapshot = persistence
                .is_some()
                .then(|| book.encode_snapshot(&self.identity));
            (receipt, snapshot)
        };
        if let (Some(persist), Some(snapshot)) = (persistence.as_mut(), snapshot) {
            persist(&snapshot)?;
        }
        Ok(receipt)
    }
}

impl Endpoint {
    /// Install the host's durable store for the address book: known destinations, their
    /// keys, app data and received ratchets, as a snapshot signed by this endpoint's identity.
    ///
    /// It receives a snapshot after each periodic clean and on
    /// [`persist_address_book`](Self::persist_address_book). It must not call back into this
    /// endpoint. Without a hook, peers are relearned from announces after a restart.
    pub fn set_address_book_persistence(
        &self,
        persist: impl FnMut(&[u8]) -> io::Result<()> + Send + 'static,
    ) {
        *self.shared.book_persistence.lock().unwrap() = Some(alloc::boxed::Box::new(persist));
    }

    /// Clean the address book now and hand its snapshot to the persistence hook, as RNS
    /// persists on exit. Fails only if the hook does.
    pub fn persist_address_book(&self) -> io::Result<CleanReceipt> {
        self.shared.clean_and_persist_book()
    }

    /// Restore peers from a snapshot this endpoint's identity signed. Peers already known
    /// keep their live entries. A tampered, foreign or malformed snapshot is refused with
    /// [`io::ErrorKind::InvalidData`] and changes nothing.
    pub fn restore_address_book(&self, snapshot: &[u8]) -> io::Result<RestoreReceipt> {
        self.shared
            .address_book
            .lock()
            .unwrap()
            .restore(snapshot, self.shared.identity.public())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    /// Keep `destination` through cleaning however long it goes unused, or release it
    /// (RNS `Identity._retain_destination_data`). Returns whether it is known.
    pub fn retain_destination(&self, destination: AddressHash, retained: bool) -> bool {
        self.shared.address_book.lock().unwrap().set_retained(
            destination,
            retained,
            book_clock_ms(),
        )
    }
}
