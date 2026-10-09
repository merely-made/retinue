//! The bounded, caller-persisted propagation store.

use std::collections::VecDeque;
use std::time::Duration;

use rmpv::Value;

use super::msgpack::{decode_one, encode_value};
use super::{
    DEFAULT_MAX_PROPAGATION_ENTRIES, DEFAULT_MAX_PROPAGATION_STORE_SNAPSHOT_BYTES,
    DEFAULT_MAX_STORED_MESSAGE_BYTES, PropagationBatch, PropagationError, PropagationMessage,
};

const PROPAGATION_STORE_SNAPSHOT_MAGIC: &[u8] = b"outrider-propagation-store";
const PROPAGATION_STORE_SNAPSHOT_VERSION: u64 = 1;

#[derive(Clone, Debug)]
pub struct PropagationStoreLimits {
    pub max_entries: usize,
    pub max_bytes: usize,
    pub max_message_bytes: usize,
    pub max_age: Duration,
    pub max_per_fetch: usize,
}

impl Default for PropagationStoreLimits {
    fn default() -> Self {
        Self {
            max_entries: 4_096,
            max_bytes: 8 * 1024 * 1024,
            // Keeps the default store conservative. Callers may raise this;
            // large fetch responses then use a request-bound Resource.
            max_message_bytes: DEFAULT_MAX_STORED_MESSAGE_BYTES,
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
            max_per_fetch: 1,
        }
    }
}

#[derive(Clone, Debug)]
struct StoredPropagation {
    transient_id: [u8; 32],
    message: PropagationMessage,
    received_at: f64,
    bytes: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreRestoreReceipt {
    pub loaded: usize,
    pub duplicates: usize,
    pub rejected_too_large: usize,
    pub expired: usize,
    pub evicted: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreReceipt {
    pub inserted: usize,
    pub duplicates: usize,
    pub rejected_too_large: usize,
    pub evicted: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StoreInsert {
    Inserted { evicted: usize },
    Duplicate,
    TooLarge,
}

/// Bounded propagation store with caller-persisted state.
///
/// This type owns admission, duplicate suppression, expiry, capacity eviction,
/// and owner-scoped offers. The host owns storage and durability policy: call
/// [`Self::encode_snapshot`] after mutations and restore those bytes with
/// [`Self::restore`]. Restoration re-derives transient ids and byte counts and
/// re-applies the current limits instead of trusting persisted indexes.
#[derive(Clone, Debug)]
pub struct PropagationStore {
    pub(super) limits: PropagationStoreLimits,
    entries: VecDeque<StoredPropagation>,
    bytes: usize,
}

impl PropagationStore {
    pub fn new(limits: PropagationStoreLimits) -> Self {
        Self {
            limits,
            entries: VecDeque::new(),
            bytes: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Restore a complete versioned snapshot under the supplied current limits.
    ///
    /// The operation is atomic from the caller's perspective: malformed input
    /// returns an error without exposing a partially restored store.
    pub fn restore(
        limits: PropagationStoreLimits,
        snapshot: &[u8],
        now: f64,
    ) -> Result<(Self, StoreRestoreReceipt), PropagationError> {
        Self::restore_bounded(
            limits,
            snapshot,
            DEFAULT_MAX_PROPAGATION_STORE_SNAPSHOT_BYTES,
            now,
        )
    }

    /// Restore with a host-selected maximum snapshot size.
    pub fn restore_bounded(
        limits: PropagationStoreLimits,
        snapshot: &[u8],
        max_snapshot_bytes: usize,
        now: f64,
    ) -> Result<(Self, StoreRestoreReceipt), PropagationError> {
        if snapshot.len() > max_snapshot_bytes {
            return Err(PropagationError::StoreSnapshotTooLarge);
        }
        if !now.is_finite() {
            return Err(PropagationError::InvalidStoreSnapshot);
        }
        let Value::Array(parts) = decode_one(snapshot)? else {
            return Err(PropagationError::InvalidStoreSnapshot);
        };
        if parts.len() != 3
            || !matches!(&parts[0], Value::Binary(magic) if magic == PROPAGATION_STORE_SNAPSHOT_MAGIC)
        {
            return Err(PropagationError::InvalidStoreSnapshot);
        }
        let version = parts[1]
            .as_u64()
            .ok_or(PropagationError::InvalidStoreSnapshot)?;
        if version != PROPAGATION_STORE_SNAPSHOT_VERSION {
            return Err(PropagationError::UnsupportedStoreSnapshotVersion(version));
        }
        let Value::Array(entries) = &parts[2] else {
            return Err(PropagationError::InvalidStoreSnapshot);
        };
        if entries.len() > DEFAULT_MAX_PROPAGATION_ENTRIES {
            return Err(PropagationError::TooManyEntries);
        }

        let mut store = Self::new(limits);
        let mut receipt = StoreRestoreReceipt::default();
        for entry in entries {
            let Value::Array(parts) = entry else {
                return Err(PropagationError::InvalidStoreSnapshot);
            };
            if parts.len() != 2 {
                return Err(PropagationError::InvalidStoreSnapshot);
            }
            let Value::F64(received_at) = parts[0] else {
                return Err(PropagationError::InvalidStoreSnapshot);
            };
            if !received_at.is_finite() {
                return Err(PropagationError::InvalidStoreSnapshot);
            }
            let Value::Binary(message) = &parts[1] else {
                return Err(PropagationError::InvalidStoreSnapshot);
            };
            let message = PropagationMessage::decode(message, message.len())?;
            if now - received_at > store.limits.max_age.as_secs_f64() {
                receipt.expired += 1;
                continue;
            }
            match store.insert(message, received_at) {
                StoreInsert::Inserted { evicted } => {
                    receipt.loaded += 1;
                    receipt.evicted += evicted;
                }
                StoreInsert::Duplicate => receipt.duplicates += 1,
                StoreInsert::TooLarge => receipt.rejected_too_large += 1,
            }
        }
        Ok((store, receipt))
    }

    /// Encode the complete logical store as a versioned MessagePack snapshot.
    ///
    /// The snapshot omits derived transient ids and byte counts. Hosts should
    /// durably replace their previous record only after this method succeeds.
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, PropagationError> {
        let entries = self
            .entries
            .iter()
            .map(|entry| {
                Value::Array(vec![
                    Value::F64(entry.received_at),
                    Value::Binary(entry.message.encode()),
                ])
            })
            .collect();
        let encoded = encode_value(&Value::Array(vec![
            Value::Binary(PROPAGATION_STORE_SNAPSHOT_MAGIC.to_vec()),
            Value::from(PROPAGATION_STORE_SNAPSHOT_VERSION),
            Value::Array(entries),
        ]))?;
        Ok(encoded)
    }

    pub fn ingest(&mut self, batch: &PropagationBatch, now: f64) -> StoreReceipt {
        self.prune(now);
        let mut receipt = StoreReceipt::default();
        for entry in &batch.entries {
            match self.insert(entry.message().clone(), now) {
                StoreInsert::Inserted { evicted } => {
                    receipt.inserted += 1;
                    receipt.evicted += evicted;
                }
                StoreInsert::Duplicate => receipt.duplicates += 1,
                StoreInsert::TooLarge => receipt.rejected_too_large += 1,
            }
        }
        receipt
    }

    pub fn prune(&mut self, now: f64) -> usize {
        let max_age = self.limits.max_age.as_secs_f64();
        let before = self.entries.len();
        self.entries.retain(|entry| {
            let keep = now - entry.received_at <= max_age;
            if !keep {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
            keep
        });
        before - self.entries.len()
    }

    pub(super) fn acknowledge(&mut self, destination: [u8; 16], handled: &[[u8; 32]]) -> usize {
        let before = self.entries.len();
        self.entries.retain(|entry| {
            let remove =
                entry.message.destination == destination && handled.contains(&entry.transient_id);
            if remove {
                self.bytes = self.bytes.saturating_sub(entry.bytes);
            }
            !remove
        });
        before - self.entries.len()
    }

    pub(super) fn offer(&self, destination: [u8; 16], max_messages: usize) -> Vec<[u8; 32]> {
        self.entries
            .iter()
            .filter(|entry| entry.message.destination == destination)
            .take(max_messages.min(self.limits.max_per_fetch))
            .map(|entry| entry.transient_id)
            .collect()
    }

    pub(super) fn messages(
        &self,
        destination: [u8; 16],
        wanted: &[[u8; 32]],
    ) -> Vec<PropagationMessage> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.message.destination == destination && wanted.contains(&entry.transient_id)
            })
            .take(self.limits.max_per_fetch)
            .map(|entry| entry.message.clone())
            .collect()
    }

    fn insert(&mut self, message: PropagationMessage, received_at: f64) -> StoreInsert {
        let transient_id = message.transient_id();
        if self
            .entries
            .iter()
            .any(|stored| stored.transient_id == transient_id)
        {
            return StoreInsert::Duplicate;
        }
        let bytes = message.encode().len();
        if bytes > self.limits.max_message_bytes || bytes > self.limits.max_bytes {
            return StoreInsert::TooLarge;
        }
        self.entries.push_back(StoredPropagation {
            transient_id,
            message,
            received_at,
            bytes,
        });
        self.bytes += bytes;
        let mut evicted = 0;
        while self.entries.len() > self.limits.max_entries || self.bytes > self.limits.max_bytes {
            if self.evict_oldest() {
                evicted += 1;
            } else {
                break;
            }
        }
        StoreInsert::Inserted { evicted }
    }

    fn evict_oldest(&mut self) -> bool {
        let Some(entry) = self.entries.pop_front() else {
            return false;
        };
        self.bytes = self.bytes.saturating_sub(entry.bytes);
        true
    }
}
