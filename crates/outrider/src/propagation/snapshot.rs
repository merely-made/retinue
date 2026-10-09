//! Versioned store snapshots. Version 2 keeps each entry's stamp and stamp value and the
//! processed transient ids; version 1 (messages only) still restores.

use rmpv::Value;

use super::msgpack::{decode_one, encode_value};
use super::store::{StoreInsert, StoredPropagation};
use super::{
    DEFAULT_MAX_PROPAGATION_STORE_SNAPSHOT_BYTES, PropagationError, PropagationMessage,
    PropagationStore, PropagationStoreLimits, StoreRestoreReceipt,
};
use crate::stamp::STAMP_LEN;

const MAGIC: &[u8] = b"outrider-propagation-store";
const VERSION: u64 = 2;

struct RestoredEntry {
    received_at: f64,
    message: PropagationMessage,
    stamp: Option<[u8; STAMP_LEN]>,
    stamp_value: u16,
}

impl PropagationStore {
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

    /// Restore with a host-selected maximum snapshot size. Entries past the current
    /// limits are expired or evicted as live ones would be.
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
        if parts.len() < 3 || !matches!(&parts[0], Value::Binary(magic) if magic == MAGIC) {
            return Err(PropagationError::InvalidStoreSnapshot);
        }
        let version = parts[1]
            .as_u64()
            .ok_or(PropagationError::InvalidStoreSnapshot)?;
        if !(1..=VERSION).contains(&version) {
            return Err(PropagationError::UnsupportedStoreSnapshotVersion(version));
        }
        let processed = match (version, &parts[3..]) {
            (1, []) => Vec::new(),
            (2, [Value::Array(processed)]) => decode_processed(processed)?,
            _ => return Err(PropagationError::InvalidStoreSnapshot),
        };
        let Value::Array(entries) = &parts[2] else {
            return Err(PropagationError::InvalidStoreSnapshot);
        };
        let entries = entries
            .iter()
            .map(|entry| decode_entry(entry, version))
            .collect::<Result<Vec<_>, _>>()?;

        let mut store = Self::new(limits);
        for (id, at) in processed {
            store.processed.insert(id, at);
        }
        let mut receipt = StoreRestoreReceipt::default();
        let max_age = store.limits.max_age.as_secs_f64();
        for entry in entries {
            if now - entry.received_at > max_age {
                receipt.expired += 1;
                continue;
            }
            if version == 1 {
                let id = entry.message.transient_id();
                store.processed.insert(id, entry.received_at);
            }
            match store.insert(
                entry.message,
                entry.stamp,
                entry.stamp_value,
                entry.received_at,
                now,
            ) {
                StoreInsert::Inserted { evicted } => {
                    receipt.loaded += 1;
                    receipt.evicted += evicted;
                }
                StoreInsert::Duplicate => receipt.duplicates += 1,
                StoreInsert::TooLarge => receipt.rejected_too_large += 1,
            }
        }
        store.prune(now);
        Ok((store, receipt))
    }

    /// Encode the complete logical store as a versioned MessagePack snapshot.
    ///
    /// The snapshot omits derived transient ids and byte counts. Hosts should
    /// durably replace their previous record only after this method succeeds.
    pub fn encode_snapshot(&self) -> Result<Vec<u8>, PropagationError> {
        let mut stored: Vec<&StoredPropagation> = self.entries.values().collect();
        stored.sort_unstable_by_key(|entry| entry.seq);
        let entries = stored
            .into_iter()
            .map(|entry| {
                Value::Array(vec![
                    Value::F64(entry.received_at),
                    Value::Binary(entry.message.encode()),
                    entry
                        .stamp
                        .map_or(Value::Nil, |stamp| Value::Binary(stamp.to_vec())),
                    Value::from(entry.stamp_value),
                ])
            })
            .collect();
        let processed = self
            .processed
            .order
            .iter()
            .map(|id| {
                Value::Array(vec![
                    Value::Binary(id.to_vec()),
                    Value::F64(self.processed.at[id]),
                ])
            })
            .collect();
        encode_value(&Value::Array(vec![
            Value::Binary(MAGIC.to_vec()),
            Value::from(VERSION),
            Value::Array(entries),
            Value::Array(processed),
        ]))
    }
}

fn finite(value: &Value) -> Result<f64, PropagationError> {
    match value {
        Value::F64(at) if at.is_finite() => Ok(*at),
        _ => Err(PropagationError::InvalidStoreSnapshot),
    }
}

fn decode_entry(entry: &Value, version: u64) -> Result<RestoredEntry, PropagationError> {
    let Value::Array(parts) = entry else {
        return Err(PropagationError::InvalidStoreSnapshot);
    };
    let expected = if version == 1 { 2 } else { 4 };
    if parts.len() != expected {
        return Err(PropagationError::InvalidStoreSnapshot);
    }
    let received_at = finite(&parts[0])?;
    let Value::Binary(message) = &parts[1] else {
        return Err(PropagationError::InvalidStoreSnapshot);
    };
    let message = PropagationMessage::decode(message, message.len())?;
    if version == 1 {
        return Ok(RestoredEntry {
            received_at,
            message,
            stamp: None,
            stamp_value: 0,
        });
    }
    let stamp = match &parts[2] {
        Value::Nil => None,
        Value::Binary(stamp) => Some(
            stamp
                .as_slice()
                .try_into()
                .map_err(|_| PropagationError::InvalidStoreSnapshot)?,
        ),
        _ => return Err(PropagationError::InvalidStoreSnapshot),
    };
    let stamp_value = parts[3]
        .as_u64()
        .and_then(|value| u16::try_from(value).ok())
        .ok_or(PropagationError::InvalidStoreSnapshot)?;
    Ok(RestoredEntry {
        received_at,
        message,
        stamp,
        stamp_value,
    })
}

fn decode_processed(values: &[Value]) -> Result<Vec<([u8; 32], f64)>, PropagationError> {
    values
        .iter()
        .map(|value| match value {
            Value::Array(pair) if pair.len() == 2 => {
                let Value::Binary(id) = &pair[0] else {
                    return Err(PropagationError::InvalidStoreSnapshot);
                };
                let id = id
                    .as_slice()
                    .try_into()
                    .map_err(|_| PropagationError::InvalidStoreSnapshot)?;
                Ok((id, finite(&pair[1])?))
            }
            _ => Err(PropagationError::InvalidStoreSnapshot),
        })
        .collect()
}
