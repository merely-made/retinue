//! Memory of what was already delivered here, so a sender's retry is not a second message.
//!
//! Stock keeps one table, `locally_delivered_transient_ids`, of message ids and propagation
//! transient ids with the time each was delivered, and forgets an entry after six message
//! lifetimes (`LXMRouter.py` 1013-1033, 1975-1980, 2565-2577). This is that table, bounded,
//! with the host owning durability as for [`PropagationStore`](crate::PropagationStore).

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::Mutex;

use rmpv::Value;

/// How long a delivered id is remembered: six 30-day message lifetimes, as stock.
pub const DELIVERED_EXPIRY_SECONDS: f64 = 6.0 * 30.0 * 24.0 * 60.0 * 60.0;
/// Ids remembered before the oldest is forgotten early. Stock has no bound.
pub const DEFAULT_MAX_DELIVERED_IDS: usize = 65_536;

const SNAPSHOT_MAGIC: &[u8] = b"outrider-delivered";
const SNAPSHOT_VERSION: u64 = 1;
/// An entry's encoded size: fixarray, bin8 header, the id, and an f64.
const SNAPSHOT_ENTRY_BYTES: usize = 1 + 2 + 32 + 9;

/// Ids of messages, or propagation transients, delivered here, with when.
///
/// Shared between lanes by reference: the same message can arrive opportunistically, then
/// directly, then from a propagation node. Persist with [`encode_snapshot`](Self::encode_snapshot)
/// and [`restore`](Self::restore).
#[derive(Debug)]
pub struct DeliveredCache {
    max_ids: usize,
    ids: Mutex<HashMap<[u8; 32], f64>>,
}

impl Default for DeliveredCache {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_DELIVERED_IDS)
    }
}

impl DeliveredCache {
    pub fn new(max_ids: usize) -> Self {
        Self {
            max_ids: max_ids.max(1),
            ids: Mutex::new(HashMap::new()),
        }
    }

    /// Record `id` as delivered at `now`. False when it already was, within the expiry.
    pub fn admit(&self, id: [u8; 32], now: f64) -> bool {
        let mut ids = self.ids.lock().unwrap();
        if ids.get(&id).is_some_and(|&at| live(at, now)) {
            return false;
        }
        if ids.len() >= self.max_ids && !ids.contains_key(&id) {
            ids.retain(|_, at| live(*at, now));
            if ids.len() >= self.max_ids
                && let Some(oldest) = oldest(&ids)
            {
                ids.remove(&oldest);
            }
        }
        ids.insert(id, now);
        true
    }

    /// Whether `id` was delivered here within the expiry (stock `has_message`).
    pub fn contains(&self, id: &[u8; 32], now: f64) -> bool {
        self.ids
            .lock()
            .unwrap()
            .get(id)
            .is_some_and(|&at| live(at, now))
    }

    /// Forget expired ids, returning how many.
    pub fn prune(&self, now: f64) -> usize {
        let mut ids = self.ids.lock().unwrap();
        let before = ids.len();
        ids.retain(|_, at| live(*at, now));
        before - ids.len()
    }

    pub fn len(&self) -> usize {
        self.ids.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The whole cache as a versioned MessagePack snapshot.
    pub fn encode_snapshot(&self) -> Vec<u8> {
        let entries = self
            .ids
            .lock()
            .unwrap()
            .iter()
            .map(|(id, at)| Value::Array(vec![Value::Binary(id.to_vec()), Value::F64(*at)]))
            .collect();
        let mut encoded = Vec::new();
        rmpv::encode::write_value(
            &mut encoded,
            &Value::Array(vec![
                Value::Binary(SNAPSHOT_MAGIC.to_vec()),
                Value::from(SNAPSHOT_VERSION),
                Value::Array(entries),
            ]),
        )
        .expect("encoding into a Vec cannot fail");
        encoded
    }

    /// Restore a snapshot under `max_ids`, dropping what expired by `now` and, past the
    /// bound, the oldest. Atomic: an invalid snapshot restores nothing.
    pub fn restore(
        snapshot: &[u8],
        max_ids: usize,
        now: f64,
    ) -> Result<Self, DeliveredSnapshotError> {
        let cache = Self::new(max_ids);
        // Room for twice the larger bound, so a host that lowers it can still restore.
        let ids = cache
            .max_ids
            .max(DEFAULT_MAX_DELIVERED_IDS)
            .saturating_mul(2);
        let limit = 64 + SNAPSHOT_ENTRY_BYTES.saturating_mul(ids);
        if snapshot.len() > limit {
            return Err(DeliveredSnapshotError::TooLarge);
        }
        let mut cursor = Cursor::new(snapshot);
        let value =
            rmpv::decode::read_value(&mut cursor).map_err(|_| DeliveredSnapshotError::Invalid)?;
        if cursor.position() as usize != snapshot.len() {
            return Err(DeliveredSnapshotError::Invalid);
        }
        let Value::Array(parts) = value else {
            return Err(DeliveredSnapshotError::Invalid);
        };
        let [magic, version, Value::Array(entries)] = parts.as_slice() else {
            return Err(DeliveredSnapshotError::Invalid);
        };
        if !matches!(magic, Value::Binary(magic) if magic == SNAPSHOT_MAGIC) {
            return Err(DeliveredSnapshotError::Invalid);
        }
        match version.as_u64() {
            Some(SNAPSHOT_VERSION) => {}
            Some(other) => return Err(DeliveredSnapshotError::UnsupportedVersion(other)),
            None => return Err(DeliveredSnapshotError::Invalid),
        }
        let mut restored = Vec::with_capacity(entries.len());
        for entry in entries {
            let Value::Array(pair) = entry else {
                return Err(DeliveredSnapshotError::Invalid);
            };
            let [Value::Binary(id), Value::F64(at)] = pair.as_slice() else {
                return Err(DeliveredSnapshotError::Invalid);
            };
            let id: [u8; 32] = id
                .as_slice()
                .try_into()
                .map_err(|_| DeliveredSnapshotError::Invalid)?;
            if !at.is_finite() {
                return Err(DeliveredSnapshotError::Invalid);
            }
            if live(*at, now) {
                restored.push((id, *at));
            }
        }
        restored.sort_unstable_by(|a, b| b.1.total_cmp(&a.1));
        restored.truncate(cache.max_ids);
        cache.ids.lock().unwrap().extend(restored);
        Ok(cache)
    }
}

fn live(at: f64, now: f64) -> bool {
    now <= at + DELIVERED_EXPIRY_SECONDS
}

fn oldest(ids: &HashMap<[u8; 32], f64>) -> Option<[u8; 32]> {
    ids.iter()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .map(|(id, _)| *id)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum DeliveredSnapshotError {
    #[error("the delivered-id snapshot exceeds what its bound can hold")]
    TooLarge,
    #[error("the delivered-id snapshot is malformed")]
    Invalid,
    #[error("the delivered-id snapshot version {0} is not supported")]
    UnsupportedVersion(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_is_delivered_once_until_it_expires() {
        let cache = DeliveredCache::default();
        assert!(cache.admit([1; 32], 100.0));
        assert!(!cache.admit([1; 32], 200.0));
        assert!(cache.contains(&[1; 32], 100.0 + DELIVERED_EXPIRY_SECONDS));
        assert!(!cache.contains(&[1; 32], 101.0 + DELIVERED_EXPIRY_SECONDS));
        assert!(cache.admit([1; 32], 101.0 + DELIVERED_EXPIRY_SECONDS));
    }

    #[test]
    fn the_bound_forgets_expired_ids_first_then_the_oldest() {
        let cache = DeliveredCache::new(2);
        assert!(cache.admit([1; 32], 10.0));
        assert!(cache.admit([2; 32], 20.0));
        assert!(cache.admit([3; 32], 30.0));
        assert!(!cache.contains(&[1; 32], 30.0));
        assert!(cache.contains(&[2; 32], 30.0) && cache.contains(&[3; 32], 30.0));

        let later = 25.0 + DELIVERED_EXPIRY_SECONDS;
        assert!(cache.admit([4; 32], later));
        assert!(!cache.contains(&[2; 32], later) && cache.contains(&[3; 32], later));
        assert_eq!(cache.prune(31.0 + DELIVERED_EXPIRY_SECONDS), 1);
    }

    #[test]
    fn a_snapshot_restores_what_is_still_live() {
        let cache = DeliveredCache::default();
        cache.admit([1; 32], 10.0);
        cache.admit([2; 32], 20.0);
        cache.admit([3; 32], 30.0);
        let snapshot = cache.encode_snapshot();

        let now = 15.0 + DELIVERED_EXPIRY_SECONDS;
        let restored = DeliveredCache::restore(&snapshot, 1, now).unwrap();
        assert_eq!(restored.len(), 1);
        assert!(restored.contains(&[3; 32], now));

        let all = DeliveredCache::restore(&snapshot, 8, 30.0).unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn malformed_snapshots_restore_nothing() {
        let restore = |bytes: &[u8]| DeliveredCache::restore(bytes, 8, 0.0).map(|_| ());
        assert_eq!(restore(&[0xc0]), Err(DeliveredSnapshotError::Invalid));
        let mut future = Vec::new();
        rmpv::encode::write_value(
            &mut future,
            &Value::Array(vec![
                Value::Binary(SNAPSHOT_MAGIC.to_vec()),
                Value::from(2),
                Value::Array(Vec::new()),
            ]),
        )
        .unwrap();
        assert_eq!(
            restore(&future),
            Err(DeliveredSnapshotError::UnsupportedVersion(2))
        );
        let cache = DeliveredCache::default();
        cache.admit([7; 32], 1.0);
        let mut bytes = cache.encode_snapshot();
        let id_len = bytes.windows(2).position(|w| w == [0xc4, 0x20]).unwrap() + 1;
        bytes[id_len] = 0x1f;
        assert!(restore(&bytes).is_err());
        assert_eq!(
            DeliveredCache::restore(&vec![0; 8 << 20], 1, 0.0).map(|_| ()),
            Err(DeliveredSnapshotError::TooLarge)
        );
    }
}
