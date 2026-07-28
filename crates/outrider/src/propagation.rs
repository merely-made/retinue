//! LXMF propagation-node submission.
//!
//! This module covers the client-to-node store submission lane. Fetching and
//! propagation-node peering are separate wire protocols.

use std::collections::VecDeque;
use std::io::Cursor;
use std::time::Duration;

use retinue::destination::DestinationName;
use retinue::endpoint::{
    AcceptedResource, Endpoint, InterfaceId, PayloadMode, PeerAnnounce, ReceivedPayload,
};
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::{Identity, PrivateIdentity};
use retinue::token::{IV_LEN, decrypt_to_identity, encrypt_to_identity};
use rmpv::Value;

use crate::announce::delivery_destination;
use crate::codec::{
    CodecError, DEFAULT_MAX_MESSAGE_BYTES, DecodedLxmf, LxmfPayload, decode_bounded, prepare,
};
use crate::stamp::{
    PROPAGATION_WORKBLOCK_ROUNDS, STAMP_LEN, find, valid as stamp_valid, value as stamp_value,
    workblock,
};

pub const DEFAULT_MAX_PROPAGATION_ANNOUNCE_BYTES: usize = 4 * 1024;
pub const DEFAULT_MAX_PROPAGATION_BATCH_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_PROPAGATION_ENTRIES: usize = 4_096;
pub const MIN_ENCRYPTED_MESSAGE_BYTES: usize = 96;
pub const PROPAGATION_METADATA_NAME: u64 = 1;
pub const FETCH_LIMIT: u64 = 1_000;
pub const FETCH_PATH_HASH: [u8; 16] = [
    0x9d, 0xc1, 0xa7, 0x28, 0x83, 0x46, 0x8f, 0x57, 0xfe, 0xd5, 0x71, 0xe7, 0x96, 0xe9, 0xce, 0x98,
];
pub const DEFAULT_MAX_STORED_MESSAGE_BYTES: usize = 240;
pub const DEFAULT_MAX_PROPAGATION_STORE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;

type FetchSelection = (Vec<[u8; 32]>, Vec<[u8; 32]>, u64);
const PROPAGATION_STORE_SNAPSHOT_MAGIC: &[u8] = b"outrider-propagation-store";
const PROPAGATION_STORE_SNAPSHOT_VERSION: u64 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct PropagationCosts {
    pub propagation: u8,
    pub flexibility: u8,
    pub peering: u8,
}

/// The seven-item application data announced by an LXMF propagation node.
///
/// Field names here are limited to behavior independently varied in stock
/// black-box captures. Unknown metadata keys remain opaque MessagePack.
#[derive(Clone, Debug, PartialEq)]
pub struct PropagationAnnounce {
    pub legacy: bool,
    pub unix_time: u64,
    pub active: bool,
    pub transfer_limit_kib: u64,
    pub sync_limit_kib: u64,
    pub costs: PropagationCosts,
    pub metadata: Vec<(Value, Value)>,
}

impl PropagationAnnounce {
    pub fn encode(&self) -> Result<Vec<u8>, PropagationError> {
        let value = Value::Array(vec![
            Value::Boolean(self.legacy),
            Value::from(self.unix_time),
            Value::Boolean(self.active),
            Value::from(self.transfer_limit_kib),
            Value::from(self.sync_limit_kib),
            Value::Array(vec![
                Value::from(self.costs.propagation),
                Value::from(self.costs.flexibility),
                Value::from(self.costs.peering),
            ]),
            Value::Map(self.metadata.clone()),
        ]);
        encode_value(&value)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, PropagationError> {
        if bytes.len() > DEFAULT_MAX_PROPAGATION_ANNOUNCE_BYTES {
            return Err(PropagationError::AnnounceTooLarge);
        }
        let Value::Array(parts) = decode_one(bytes)? else {
            return Err(PropagationError::InvalidAnnounce);
        };
        if parts.len() != 7 {
            return Err(PropagationError::InvalidAnnounce);
        }
        let (
            Value::Boolean(legacy),
            Some(unix_time),
            Value::Boolean(active),
            Some(transfer_limit_kib),
            Some(sync_limit_kib),
            Value::Array(costs),
            Value::Map(metadata),
        ) = (
            &parts[0],
            parts[1].as_u64(),
            &parts[2],
            parts[3].as_u64(),
            parts[4].as_u64(),
            &parts[5],
            &parts[6],
        )
        else {
            return Err(PropagationError::InvalidAnnounce);
        };
        if costs.len() != 3 {
            return Err(PropagationError::InvalidAnnounce);
        }
        let propagation = byte(&costs[0])?;
        let flexibility = byte(&costs[1])?;
        let peering = byte(&costs[2])?;
        Ok(Self {
            legacy: *legacy,
            unix_time,
            active: *active,
            transfer_limit_kib,
            sync_limit_kib,
            costs: PropagationCosts {
                propagation,
                flexibility,
                peering,
            },
            metadata: metadata.clone(),
        })
    }

    pub fn name(&self) -> Option<&[u8]> {
        self.metadata.iter().find_map(|(key, value)| {
            (key.as_u64() == Some(PROPAGATION_METADATA_NAME))
                .then(|| value.as_slice())
                .flatten()
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagationEntry {
    message: PropagationMessage,
    stamp: [u8; STAMP_LEN],
}

impl PropagationEntry {
    pub fn decode(bytes: &[u8], max_entry_bytes: usize) -> Result<Self, PropagationError> {
        if bytes.len() > max_entry_bytes {
            return Err(PropagationError::EntryTooLarge);
        }
        if bytes.len() < 16 + MIN_ENCRYPTED_MESSAGE_BYTES + STAMP_LEN {
            return Err(PropagationError::TruncatedEntry);
        }
        let stamp = bytes[bytes.len() - STAMP_LEN..]
            .try_into()
            .expect("fixed stamp suffix");
        let message =
            PropagationMessage::decode(&bytes[..bytes.len() - STAMP_LEN], max_entry_bytes)?;
        Ok(Self { message, stamp })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = self.message.encode();
        bytes.extend_from_slice(&self.stamp);
        bytes
    }

    pub const fn destination(&self) -> &[u8; 16] {
        self.message.destination()
    }

    pub fn encrypted(&self) -> &[u8] {
        self.message.encrypted()
    }

    pub const fn stamp(&self) -> &[u8; STAMP_LEN] {
        &self.stamp
    }

    pub fn transient_id(&self) -> [u8; 32] {
        self.message.transient_id()
    }

    pub fn stamp_value(&self) -> u16 {
        let transient_id = self.transient_id();
        stamp_value(
            &workblock(&transient_id, PROPAGATION_WORKBLOCK_ROUNDS),
            &self.stamp,
        )
    }

    pub fn validate_stamp(&self, target: u16) -> bool {
        let transient_id = self.transient_id();
        stamp_valid(
            &workblock(&transient_id, PROPAGATION_WORKBLOCK_ROUNDS),
            &self.stamp,
            target,
        )
    }

    /// Decrypt the complete signed LXMF object for its recipient.
    pub fn decrypt(
        &self,
        recipient: &PrivateIdentity,
        max_message_bytes: usize,
    ) -> Result<DecodedLxmf, PropagationError> {
        self.message.decrypt(recipient, max_message_bytes)
    }

    pub fn decrypt_and_verify(
        &self,
        recipient: &PrivateIdentity,
        source: &Identity,
        max_message_bytes: usize,
    ) -> Result<DecodedLxmf, PropagationError> {
        let message = self.decrypt(recipient, max_message_bytes)?;
        if message.source != *delivery_destination(source).as_bytes() {
            return Err(PropagationError::WrongSource);
        }
        if !message.verify_with(|bytes, signature| source.verify(bytes, signature)) {
            return Err(PropagationError::BadSignature);
        }
        Ok(message)
    }

    /// The encrypted message stored and later served by a propagation node.
    pub fn message(&self) -> &PropagationMessage {
        &self.message
    }
}

/// Recipient destination plus identity-encrypted signed LXMF object.
///
/// Submission appends a propagation stamp to this value. Fetch responses
/// return this value without that ingress stamp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagationMessage {
    destination: [u8; 16],
    encrypted: Vec<u8>,
}

impl PropagationMessage {
    pub fn decode(bytes: &[u8], max_message_bytes: usize) -> Result<Self, PropagationError> {
        if bytes.len() > max_message_bytes {
            return Err(PropagationError::EntryTooLarge);
        }
        if bytes.len() < 16 + MIN_ENCRYPTED_MESSAGE_BYTES {
            return Err(PropagationError::TruncatedEntry);
        }
        Ok(Self {
            destination: bytes[..16].try_into().expect("checked message length"),
            encrypted: bytes[16..].to_vec(),
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + self.encrypted.len());
        bytes.extend_from_slice(&self.destination);
        bytes.extend_from_slice(&self.encrypted);
        bytes
    }

    pub const fn destination(&self) -> &[u8; 16] {
        &self.destination
    }

    pub fn encrypted(&self) -> &[u8] {
        &self.encrypted
    }

    pub fn transient_id(&self) -> [u8; 32] {
        full_hash(&self.encode())
    }

    pub fn decrypt(
        &self,
        recipient: &PrivateIdentity,
        max_message_bytes: usize,
    ) -> Result<DecodedLxmf, PropagationError> {
        let expected = delivery_destination(recipient.public());
        if self.destination != *expected.as_bytes() {
            return Err(PropagationError::WrongDestination);
        }
        let remainder = decrypt_to_identity(recipient, &self.encrypted)?;
        let mut packed = Vec::with_capacity(16 + remainder.len());
        packed.extend_from_slice(&self.destination);
        packed.extend_from_slice(&remainder);
        let message = decode_bounded(&packed, max_message_bytes.min(DEFAULT_MAX_MESSAGE_BYTES))?;
        if message.destination != self.destination {
            return Err(PropagationError::WrongDestination);
        }
        Ok(message)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropagationBatch {
    pub transfer_time: f64,
    pub entries: Vec<PropagationEntry>,
}

impl PropagationBatch {
    pub fn encode(&self) -> Result<Vec<u8>, PropagationError> {
        if !self.transfer_time.is_finite() {
            return Err(PropagationError::InvalidTransferTime);
        }
        if self.entries.len() > DEFAULT_MAX_PROPAGATION_ENTRIES {
            return Err(PropagationError::TooManyEntries);
        }
        let value = Value::Array(vec![
            Value::F64(self.transfer_time),
            Value::Array(
                self.entries
                    .iter()
                    .map(|entry| Value::Binary(entry.encode()))
                    .collect(),
            ),
        ]);
        let encoded = encode_value(&value)?;
        if encoded.len() > DEFAULT_MAX_PROPAGATION_BATCH_BYTES {
            return Err(PropagationError::BatchTooLarge);
        }
        Ok(encoded)
    }

    pub fn decode(
        bytes: &[u8],
        max_batch_bytes: usize,
        max_entries: usize,
    ) -> Result<Self, PropagationError> {
        if bytes.len() > max_batch_bytes.min(DEFAULT_MAX_PROPAGATION_BATCH_BYTES) {
            return Err(PropagationError::BatchTooLarge);
        }
        let Value::Array(parts) = decode_one(bytes)? else {
            return Err(PropagationError::InvalidBatch);
        };
        if parts.len() != 2 {
            return Err(PropagationError::InvalidBatch);
        }
        let Value::F64(transfer_time) = parts[0] else {
            return Err(PropagationError::InvalidTransferTime);
        };
        if !transfer_time.is_finite() {
            return Err(PropagationError::InvalidTransferTime);
        }
        let Value::Array(entries) = &parts[1] else {
            return Err(PropagationError::InvalidBatch);
        };
        if entries.len() > max_entries.min(DEFAULT_MAX_PROPAGATION_ENTRIES) {
            return Err(PropagationError::TooManyEntries);
        }
        let mut decoded = Vec::with_capacity(entries.len());
        for entry in entries {
            let Value::Binary(entry) = entry else {
                return Err(PropagationError::InvalidBatch);
            };
            decoded.push(PropagationEntry::decode(entry, max_batch_bytes)?);
        }
        Ok(Self {
            transfer_time,
            entries: decoded,
        })
    }
}

#[derive(Clone, Debug)]
pub struct PreparedPropagation {
    pub message_id: [u8; 32],
    pub transient_id: [u8; 32],
    pub stamp_value: u16,
    pub packed_message: Vec<u8>,
    pub entry: PropagationEntry,
}

/// Build, sign, encrypt, and stamp one message for propagation submission.
///
/// `ephemeral_secret` and `iv` must be fresh and unpredictable. The stamp seed
/// need not be secret. Keeping all three explicit makes deterministic receipt
/// tests possible and leaves entropy policy with the runtime.
#[allow(clippy::too_many_arguments)]
pub fn prepare_propagation(
    sender: &PrivateIdentity,
    recipient: &Identity,
    payload: &LxmfPayload,
    ephemeral_secret: &[u8; 32],
    iv: &[u8; IV_LEN],
    stamp_seed: [u8; STAMP_LEN],
    target_cost: u16,
    max_stamp_attempts: u64,
) -> Result<PreparedPropagation, PropagationError> {
    let destination = delivery_destination(recipient);
    let source = delivery_destination(sender.public());
    let prepared = prepare(*destination.as_bytes(), *source.as_bytes(), payload)?;
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed_message = prepared.finish(signature);
    let encrypted = encrypt_to_identity(recipient, ephemeral_secret, iv, &packed_message[16..]);
    let mut transient_input = Vec::with_capacity(16 + encrypted.len());
    transient_input.extend_from_slice(destination.as_slice());
    transient_input.extend_from_slice(&encrypted);
    let transient_id = full_hash(&transient_input);
    let block = workblock(&transient_id, PROPAGATION_WORKBLOCK_ROUNDS);
    let (stamp, stamp_value) = find(&block, target_cost, stamp_seed, max_stamp_attempts)
        .ok_or(PropagationError::StampBudgetExhausted)?;
    let entry = PropagationEntry {
        message: PropagationMessage {
            destination: *destination.as_bytes(),
            encrypted,
        },
        stamp,
    };
    Ok(PreparedPropagation {
        message_id,
        transient_id,
        stamp_value,
        packed_message,
        entry,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagationSubmitReceipt {
    pub transient_ids: Vec<[u8; 32]>,
    pub mode: PayloadMode,
    pub packed_batch: Vec<u8>,
}

pub async fn submit(
    endpoint: &Endpoint,
    node: &PeerAnnounce,
    batch: &PropagationBatch,
) -> Result<PropagationSubmitReceipt, PropagationError> {
    if node.destination != propagation_destination(&node.identity) {
        return Err(PropagationError::WrongDestination);
    }
    let announce = PropagationAnnounce::decode(&node.app_data)?;
    if !announce.active {
        return Err(PropagationError::InactiveNode);
    }
    let target = u16::from(announce.costs.propagation);
    if batch
        .entries
        .iter()
        .any(|entry| !entry.validate_stamp(target))
    {
        return Err(PropagationError::InvalidStamp);
    }
    let packed_batch = batch.encode()?;
    let announced_limit = announce
        .transfer_limit_kib
        .saturating_mul(1_000)
        .min(usize::MAX as u64) as usize;
    if packed_batch.len() > announced_limit {
        return Err(PropagationError::BatchTooLarge);
    }
    let transient_ids = batch
        .entries
        .iter()
        .map(PropagationEntry::transient_id)
        .collect();
    let mode = endpoint
        .send_payload(node.destination, node.identity, &packed_batch)
        .await?;
    Ok(PropagationSubmitReceipt {
        transient_ids,
        mode,
        packed_batch,
    })
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedPropagationBatch {
    pub batch: PropagationBatch,
    pub mode: PayloadMode,
    pub interface: InterfaceId,
    pub packed_batch: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct FetchedPropagation {
    pub transient_id: [u8; 32],
    pub entry: PropagationMessage,
    pub message: DecodedLxmf,
    pub source_identity: Identity,
}

#[derive(Clone, Debug)]
pub struct PropagationFetchReceipt {
    pub offered: Vec<[u8; 32]>,
    pub messages: Vec<FetchedPropagation>,
}

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
    limits: PropagationStoreLimits,
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

    fn acknowledge(&mut self, destination: [u8; 16], handled: &[[u8; 32]]) -> usize {
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

    fn offer(&self, destination: [u8; 16], max_messages: usize) -> Vec<[u8; 32]> {
        self.entries
            .iter()
            .filter(|entry| entry.message.destination == destination)
            .take(max_messages.min(self.limits.max_per_fetch))
            .map(|entry| entry.transient_id)
            .collect()
    }

    fn messages(&self, destination: [u8; 16], wanted: &[[u8; 32]]) -> Vec<PropagationMessage> {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedFetch {
    pub owner: Identity,
    pub offered: Vec<[u8; 32]>,
    pub served: Vec<[u8; 32]>,
    pub acknowledged: usize,
}

/// Serve one stock-compatible two-request fetch session.
pub async fn serve_fetch(
    endpoint: &Endpoint,
    accepted: &mut AcceptedResource,
    store: &mut PropagationStore,
    now: f64,
) -> Result<ServedFetch, PropagationError> {
    if accepted.destination != propagation_destination(endpoint.identity()) {
        return Err(PropagationError::WrongDestination);
    }
    store.prune(now);
    let offer_request = accepted
        .session
        .receive_raw_request()
        .await
        .map_err(PropagationError::OfferRequest)?;
    let owner = offer_request
        .peer
        .ok_or(PropagationError::UnidentifiedFetch)?;
    decode_offer_request(&offer_request.packed)?;
    let destination = *delivery_destination(&owner).as_bytes();
    let offered = store.offer(destination, store.limits.max_per_fetch);
    let offer_value = Value::Array(
        offered
            .iter()
            .map(|id| Value::Binary(id.to_vec()))
            .collect(),
    );
    let packed_offer = encode_value(&offer_value)?;
    accepted
        .session
        .respond_value_auto(offer_request.request_id, &packed_offer)
        .await?;

    if offered.is_empty() {
        return Ok(ServedFetch {
            owner,
            offered,
            served: Vec::new(),
            acknowledged: 0,
        });
    }

    let fetch_request = accepted
        .session
        .receive_raw_request()
        .await
        .map_err(PropagationError::SelectionRequest)?;
    if fetch_request.peer != Some(owner) {
        return Err(PropagationError::FetchIdentityChanged);
    }
    let (wanted, handled, limit) = decode_fetch_selection(&fetch_request.packed)?;
    let acknowledged = store.acknowledge(destination, &handled);
    let wanted: Vec<[u8; 32]> = wanted
        .into_iter()
        .filter(|id| offered.contains(id))
        .take(
            usize::try_from(limit)
                .unwrap_or(usize::MAX)
                .min(store.limits.max_per_fetch),
        )
        .collect();
    let messages = store.messages(destination, &wanted);
    let served: Vec<[u8; 32]> = messages
        .iter()
        .map(PropagationMessage::transient_id)
        .collect();
    let packed_messages = encode_value(&Value::Array(
        messages
            .iter()
            .map(|message| Value::Binary(message.encode()))
            .collect(),
    ))?;
    accepted
        .session
        .respond_value_auto(fetch_request.request_id, &packed_messages)
        .await?;
    Ok(ServedFetch {
        owner,
        offered,
        served,
        acknowledged,
    })
}

/// Fetch messages for this endpoint's delivery identity from a propagation
/// node.
///
/// The two-stage exchange first asks what is available, then requests a
/// bounded subset while reporting already handled transient ids.
#[allow(clippy::too_many_arguments)]
pub async fn fetch(
    endpoint: &Endpoint,
    recipient: &PrivateIdentity,
    node: &PeerAnnounce,
    handled: &[[u8; 32]],
    max_messages: usize,
    request_time: f64,
    max_entry_bytes: usize,
    max_message_bytes: usize,
) -> Result<PropagationFetchReceipt, PropagationError> {
    if recipient.public() != endpoint.identity() {
        return Err(PropagationError::LocalIdentityMismatch);
    }
    if node.destination != propagation_destination(&node.identity) {
        return Err(PropagationError::WrongDestination);
    }
    PropagationAnnounce::decode(&node.app_data)?;
    if !request_time.is_finite() {
        return Err(PropagationError::InvalidTransferTime);
    }

    let mut session = endpoint
        .open_resource(node.destination, node.identity)
        .await?;
    session.identify();
    let offer_request =
        encode_fetch_request(request_time, Value::Array(vec![Value::Nil, Value::Nil]))?;
    let offer_response = session.request_raw(&offer_request).await?;
    let offered = decode_id_response(&offer_response.packed)?;
    let wanted: Vec<[u8; 32]> = offered.iter().copied().take(max_messages).collect();
    if wanted.is_empty() {
        return Ok(PropagationFetchReceipt {
            offered,
            messages: Vec::new(),
        });
    }

    let fetch_data = Value::Array(vec![
        Value::Array(wanted.iter().map(|id| Value::Binary(id.to_vec())).collect()),
        Value::Array(
            handled
                .iter()
                .map(|id| Value::Binary(id.to_vec()))
                .collect(),
        ),
        Value::from(FETCH_LIMIT),
    ]);
    let fetch_request = encode_fetch_request(request_time, fetch_data)?;
    let fetch_response = session.request_raw(&fetch_request).await?;
    let entries = decode_entry_response(&fetch_response.packed, max_entry_bytes)?;
    let mut messages = Vec::with_capacity(entries.len());
    for entry in entries {
        let transient_id = entry.transient_id();
        if !wanted.contains(&transient_id) {
            return Err(PropagationError::UnexpectedTransientId);
        }
        let message = entry.decrypt(recipient, max_message_bytes)?;
        let source_destination = AddressHash::from_bytes(message.source);
        let source_identity = endpoint
            .resolve(source_destination)
            .ok_or(PropagationError::UnknownSource)?;
        if message.source != *delivery_destination(&source_identity).as_bytes()
            || !message.verify_with(|bytes, signature| source_identity.verify(bytes, signature))
        {
            return Err(PropagationError::BadSignature);
        }
        messages.push(FetchedPropagation {
            transient_id,
            entry,
            message,
            source_identity,
        });
    }
    Ok(PropagationFetchReceipt { offered, messages })
}

pub async fn receive_submission(
    endpoint: &Endpoint,
    mut accepted: AcceptedResource,
    target_cost: u16,
    max_batch_bytes: usize,
    max_entries: usize,
) -> Result<ReceivedPropagationBatch, PropagationError> {
    if accepted.destination != propagation_destination(endpoint.identity()) {
        return Err(PropagationError::WrongDestination);
    }
    let interface = accepted.interface;
    let (mode, packed_batch) = match accepted.session.receive().await? {
        ReceivedPayload::Data(bytes) => (PayloadMode::Data, bytes),
        ReceivedPayload::Resource(bytes) => (PayloadMode::Resource, bytes),
    };
    let batch = PropagationBatch::decode(&packed_batch, max_batch_bytes, max_entries)?;
    if batch
        .entries
        .iter()
        .any(|entry| !entry.validate_stamp(target_cost))
    {
        return Err(PropagationError::InvalidStamp);
    }
    Ok(ReceivedPropagationBatch {
        batch,
        mode,
        interface,
        packed_batch,
    })
}

pub fn register_propagation(
    endpoint: &Endpoint,
    announce: &PropagationAnnounce,
) -> Result<AddressHash, PropagationError> {
    let app_data = announce.encode()?;
    let name = propagation_name();
    let destination = name.destination_hash(endpoint.identity());
    endpoint.register_resource(name, &app_data);
    Ok(destination)
}

pub fn announce_propagation(
    endpoint: &Endpoint,
    announce: &PropagationAnnounce,
) -> Result<(), PropagationError> {
    endpoint.announce(&propagation_name(), &announce.encode()?);
    Ok(())
}

pub fn propagation_name() -> DestinationName {
    DestinationName::new("lxmf", ["propagation"])
}

pub fn propagation_destination(identity: &Identity) -> AddressHash {
    propagation_name().destination_hash(identity)
}

fn encode_value(value: &Value) -> Result<Vec<u8>, PropagationError> {
    let mut encoded = Vec::new();
    rmpv::encode::write_value(&mut encoded, value).map_err(|_| PropagationError::Encode)?;
    Ok(encoded)
}

fn encode_fetch_request(time: f64, data: Value) -> Result<Vec<u8>, PropagationError> {
    encode_value(&Value::Array(vec![
        Value::F64(time),
        Value::Binary(FETCH_PATH_HASH.to_vec()),
        data,
    ]))
}

fn decode_response(bytes: &[u8]) -> Result<Value, PropagationError> {
    let Value::Array(mut envelope) = decode_one(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    if envelope.len() != 2
        || !matches!(&envelope[0], Value::Binary(request_id) if request_id.len() == 16)
    {
        return Err(PropagationError::InvalidFetchResponse);
    }
    Ok(envelope.pop().expect("two-item response"))
}

fn decode_id_response(bytes: &[u8]) -> Result<Vec<[u8; 32]>, PropagationError> {
    let Value::Array(values) = decode_response(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    values
        .into_iter()
        .map(|value| match value {
            Value::Binary(id) if id.len() == 32 => {
                Ok(id.try_into().expect("checked transient id length"))
            }
            _ => Err(PropagationError::InvalidFetchResponse),
        })
        .collect()
}

fn decode_entry_response(
    bytes: &[u8],
    max_entry_bytes: usize,
) -> Result<Vec<PropagationMessage>, PropagationError> {
    let Value::Array(values) = decode_response(bytes)? else {
        return Err(PropagationError::InvalidFetchResponse);
    };
    if values.len() > DEFAULT_MAX_PROPAGATION_ENTRIES {
        return Err(PropagationError::TooManyEntries);
    }
    values
        .into_iter()
        .map(|value| match value {
            Value::Binary(entry) => PropagationMessage::decode(&entry, max_entry_bytes),
            _ => Err(PropagationError::InvalidFetchResponse),
        })
        .collect()
}

fn decode_offer_request(bytes: &[u8]) -> Result<(), PropagationError> {
    let data = decode_fetch_request(bytes)?;
    match data {
        Value::Array(parts) if parts == vec![Value::Nil, Value::Nil] => Ok(()),
        _ => Err(PropagationError::InvalidFetchRequest),
    }
}

fn decode_fetch_selection(bytes: &[u8]) -> Result<FetchSelection, PropagationError> {
    let Value::Array(parts) = decode_fetch_request(bytes)? else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    if parts.len() != 3 {
        return Err(PropagationError::InvalidFetchRequest);
    }
    let Value::Array(wanted) = &parts[0] else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    let Value::Array(handled) = &parts[1] else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    let limit = parts[2]
        .as_u64()
        .ok_or(PropagationError::InvalidFetchRequest)?;
    Ok((decode_ids(wanted)?, decode_ids(handled)?, limit))
}

fn decode_fetch_request(bytes: &[u8]) -> Result<Value, PropagationError> {
    let Value::Array(mut parts) = decode_one(bytes)? else {
        return Err(PropagationError::InvalidFetchRequest);
    };
    if parts.len() != 3
        || !matches!(&parts[0], Value::F64(time) if time.is_finite())
        || !matches!(&parts[1], Value::Binary(path) if path.as_slice() == FETCH_PATH_HASH)
    {
        return Err(PropagationError::InvalidFetchRequest);
    }
    Ok(parts.pop().expect("three-item request"))
}

fn decode_ids(values: &[Value]) -> Result<Vec<[u8; 32]>, PropagationError> {
    values
        .iter()
        .map(|value| match value {
            Value::Binary(id) if id.len() == 32 => Ok(id
                .as_slice()
                .try_into()
                .expect("checked transient id length")),
            _ => Err(PropagationError::InvalidFetchRequest),
        })
        .collect()
}

fn decode_one(bytes: &[u8]) -> Result<Value, PropagationError> {
    let mut cursor = Cursor::new(bytes);
    let value = rmpv::decode::read_value(&mut cursor)
        .map_err(|_| PropagationError::MalformedMessagePack)?;
    if cursor.position() as usize != bytes.len() {
        return Err(PropagationError::MalformedMessagePack);
    }
    Ok(value)
}

fn byte(value: &Value) -> Result<u8, PropagationError> {
    value
        .as_u64()
        .and_then(|value| value.try_into().ok())
        .ok_or(PropagationError::InvalidAnnounce)
}

#[derive(Debug, thiserror::Error)]
pub enum PropagationError {
    #[error("Retinue propagation transfer failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("propagation offer request failed: {0}")]
    OfferRequest(#[source] std::io::Error),
    #[error("propagation selection request failed: {0}")]
    SelectionRequest(#[source] std::io::Error),
    #[error("Retinue identity-token operation failed: {0}")]
    Crypto(#[from] retinue::Error),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("propagation MessagePack could not be encoded")]
    Encode,
    #[error("propagation data is not one complete MessagePack value")]
    MalformedMessagePack,
    #[error("propagation announce exceeds the byte limit")]
    AnnounceTooLarge,
    #[error("propagation announce has the wrong shape")]
    InvalidAnnounce,
    #[error("propagation batch exceeds the byte limit")]
    BatchTooLarge,
    #[error("propagation batch has the wrong shape")]
    InvalidBatch,
    #[error("propagation batch has an invalid transfer timestamp")]
    InvalidTransferTime,
    #[error("propagation batch has too many entries")]
    TooManyEntries,
    #[error("propagation entry exceeds the byte limit")]
    EntryTooLarge,
    #[error("propagation entry is truncated")]
    TruncatedEntry,
    #[error("propagation entry stamp does not meet the node cost")]
    InvalidStamp,
    #[error("the configured proof-of-work attempt budget was exhausted")]
    StampBudgetExhausted,
    #[error("the propagation destination does not match the recipient or node identity")]
    WrongDestination,
    #[error("the decrypted LXMF source does not match the supplied source identity")]
    WrongSource,
    #[error("the decrypted LXMF signature is invalid")]
    BadSignature,
    #[error("the announced propagation node is inactive")]
    InactiveNode,
    #[error("the local recipient identity is not the endpoint identity")]
    LocalIdentityMismatch,
    #[error("propagation fetch response has the wrong shape")]
    InvalidFetchResponse,
    #[error("propagation node returned an entry it did not offer")]
    UnexpectedTransientId,
    #[error("the decrypted message source has no validated delivery announce")]
    UnknownSource,
    #[error("propagation fetch request has the wrong shape")]
    InvalidFetchRequest,
    #[error("propagation fetch link did not identify its owner")]
    UnidentifiedFetch,
    #[error("propagation fetch identity changed during the session")]
    FetchIdentityChanged,
    #[error("propagation-store snapshot exceeds the byte limit")]
    StoreSnapshotTooLarge,
    #[error("propagation-store snapshot has the wrong shape")]
    InvalidStoreSnapshot,
    #[error("unsupported propagation-store snapshot version {0}")]
    UnsupportedStoreSnapshotVersion(u64),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn captured_announce() -> Vec<u8> {
        hex::decode(
            "97c2ce6a680a26c3cd0100cd2800930d03088101c41853746f636b2050726f7061676174696f6e204f7261636c65",
        )
        .unwrap()
    }

    #[test]
    fn stock_announce_decodes_and_round_trips_exactly() {
        let bytes = captured_announce();
        let announce = PropagationAnnounce::decode(&bytes).unwrap();
        assert!(!announce.legacy);
        assert!(announce.active);
        assert_eq!(announce.transfer_limit_kib, 256);
        assert_eq!(announce.sync_limit_kib, 10_240);
        assert_eq!(
            announce.costs,
            PropagationCosts {
                propagation: 13,
                flexibility: 3,
                peering: 8
            }
        );
        assert_eq!(
            announce.name(),
            Some(b"Stock Propagation Oracle".as_slice())
        );
        assert_eq!(announce.encode().unwrap(), bytes);
    }

    #[test]
    fn prepared_entry_decrypts_and_authenticates() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
        let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
        let payload = LxmfPayload::text(1_753_603_204.5, b"TITLE", b"BODY");
        let prepared = prepare_propagation(
            &sender,
            recipient.public(),
            &payload,
            &[0x31; 32],
            &[0x41; IV_LEN],
            [0; STAMP_LEN],
            8,
            100_000,
        )
        .unwrap();
        assert_eq!(prepared.transient_id, prepared.entry.transient_id());
        assert!(prepared.entry.validate_stamp(8));
        let decoded = prepared
            .entry
            .decrypt_and_verify(&recipient, sender.public(), 4_096)
            .unwrap();
        assert_eq!(decoded.message_id, prepared.message_id);
        assert_eq!(decoded.payload.title, b"TITLE");
        assert_eq!(decoded.payload.content, b"BODY");
    }

    #[test]
    fn batch_is_one_timestamp_and_binary_entry_list() {
        let entry = PropagationEntry {
            message: PropagationMessage {
                destination: [1; 16],
                encrypted: vec![2; MIN_ENCRYPTED_MESSAGE_BYTES],
            },
            stamp: [3; STAMP_LEN],
        };
        let batch = PropagationBatch {
            transfer_time: 1_753_603_204.5,
            entries: vec![entry],
        };
        let encoded = batch.encode().unwrap();
        assert_eq!(
            PropagationBatch::decode(&encoded, encoded.len(), 1).unwrap(),
            batch
        );
    }

    #[test]
    fn store_is_bounded_expires_and_acknowledges_by_owner() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
        let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
        let first = prepare_propagation(
            &sender,
            recipient.public(),
            &LxmfPayload::text(1.0, b"A", b"one"),
            &[0x31; 32],
            &[0x41; IV_LEN],
            [0; STAMP_LEN],
            0,
            1,
        )
        .unwrap();
        let second = prepare_propagation(
            &sender,
            recipient.public(),
            &LxmfPayload::text(2.0, b"B", b"two"),
            &[0x32; 32],
            &[0x42; IV_LEN],
            [0; STAMP_LEN],
            0,
            1,
        )
        .unwrap();
        let mut store = PropagationStore::new(PropagationStoreLimits {
            max_entries: 1,
            max_bytes: 1_024,
            max_message_bytes: 512,
            max_age: Duration::from_secs(10),
            max_per_fetch: 1,
        });
        let batch = PropagationBatch {
            transfer_time: 3.0,
            entries: vec![first.entry, second.entry],
        };
        let receipt = store.ingest(&batch, 3.0);
        assert_eq!(receipt.inserted, 2);
        assert_eq!(receipt.evicted, 1);
        assert_eq!(
            store.offer(
                *delivery_destination(recipient.public()).as_bytes(),
                usize::MAX
            ),
            vec![second.transient_id]
        );
        assert_eq!(
            store.acknowledge(
                *delivery_destination(recipient.public()).as_bytes(),
                &[second.transient_id]
            ),
            1
        );
        assert!(store.is_empty());

        let later = PropagationBatch {
            transfer_time: 4.0,
            entries: vec![
                prepare_propagation(
                    &sender,
                    recipient.public(),
                    &LxmfPayload::text(4.0, b"C", b"three"),
                    &[0x33; 32],
                    &[0x43; IV_LEN],
                    [0; STAMP_LEN],
                    0,
                    1,
                )
                .unwrap()
                .entry,
            ],
        };
        store.ingest(&later, 4.0);
        assert_eq!(store.prune(15.0), 1);
        assert!(store.is_empty());
    }

    #[test]
    fn store_snapshot_round_trip_rederives_ids_bytes_and_owner_scope() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
        let first_recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
        let second_recipient = PrivateIdentity::from_secret_bytes(&[0x63; 64]);
        let first = prepare_propagation(
            &sender,
            first_recipient.public(),
            &LxmfPayload::text(10.0, b"A", b"first"),
            &[0x31; 32],
            &[0x41; IV_LEN],
            [0; STAMP_LEN],
            0,
            1,
        )
        .unwrap();
        let second = prepare_propagation(
            &sender,
            second_recipient.public(),
            &LxmfPayload::text(11.0, b"B", b"second"),
            &[0x32; 32],
            &[0x42; IV_LEN],
            [0; STAMP_LEN],
            0,
            1,
        )
        .unwrap();
        let limits = PropagationStoreLimits {
            max_entries: 4,
            max_bytes: 4_096,
            max_message_bytes: 1_024,
            max_age: Duration::from_secs(60),
            max_per_fetch: 4,
        };
        let mut store = PropagationStore::new(limits.clone());
        assert_eq!(
            store
                .ingest(
                    &PropagationBatch {
                        transfer_time: 12.0,
                        entries: vec![first.entry.clone()],
                    },
                    12.0,
                )
                .inserted,
            1
        );
        assert_eq!(
            store
                .ingest(
                    &PropagationBatch {
                        transfer_time: 13.0,
                        entries: vec![second.entry.clone()],
                    },
                    13.0,
                )
                .inserted,
            1
        );
        let original_bytes = store.bytes();
        let snapshot = store.encode_snapshot().unwrap();
        let (mut restored, receipt) = PropagationStore::restore(limits, &snapshot, 14.0).unwrap();

        assert_eq!(
            receipt,
            StoreRestoreReceipt {
                loaded: 2,
                ..StoreRestoreReceipt::default()
            }
        );
        assert_eq!(restored.bytes(), original_bytes);
        assert_eq!(restored.encode_snapshot().unwrap(), snapshot);
        assert_eq!(
            restored.offer(
                *delivery_destination(first_recipient.public()).as_bytes(),
                usize::MAX,
            ),
            vec![first.transient_id]
        );
        assert_eq!(
            restored.offer(
                *delivery_destination(second_recipient.public()).as_bytes(),
                usize::MAX,
            ),
            vec![second.transient_id]
        );
        assert_eq!(
            restored
                .ingest(
                    &PropagationBatch {
                        transfer_time: 15.0,
                        entries: vec![first.entry],
                    },
                    15.0,
                )
                .duplicates,
            1
        );
    }

    #[test]
    fn restore_reapplies_expiry_and_current_capacity_limits() {
        let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
        let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
        let prepared: Vec<_> = (0..3_u8)
            .map(|index| {
                prepare_propagation(
                    &sender,
                    recipient.public(),
                    &LxmfPayload::text(f64::from(index), [index], [index; 8]),
                    &[0x31 + index; 32],
                    &[0x41 + index; IV_LEN],
                    [0; STAMP_LEN],
                    0,
                    1,
                )
                .unwrap()
            })
            .collect();
        let original_limits = PropagationStoreLimits {
            max_entries: 3,
            max_bytes: 4_096,
            max_message_bytes: 1_024,
            max_age: Duration::from_secs(60),
            max_per_fetch: 3,
        };
        let mut original = PropagationStore::new(original_limits);
        for (index, entry) in prepared.iter().enumerate() {
            original.ingest(
                &PropagationBatch {
                    transfer_time: index as f64,
                    entries: vec![entry.entry.clone()],
                },
                [1.0, 5.0, 9.0][index],
            );
        }

        let limits = PropagationStoreLimits {
            max_entries: 1,
            max_bytes: 1_024,
            max_message_bytes: 512,
            max_age: Duration::from_secs(5),
            max_per_fetch: 1,
        };
        let snapshot = original.encode_snapshot().unwrap();
        let (restored, receipt) = PropagationStore::restore(limits, &snapshot, 10.0).unwrap();
        assert_eq!(
            receipt,
            StoreRestoreReceipt {
                loaded: 2,
                expired: 1,
                evicted: 1,
                ..StoreRestoreReceipt::default()
            }
        );
        assert_eq!(restored.len(), 1);
        assert_eq!(
            restored.offer(
                *delivery_destination(recipient.public()).as_bytes(),
                usize::MAX,
            ),
            vec![prepared[2].transient_id]
        );

        let small_message_limit = PropagationStoreLimits {
            max_entries: 3,
            max_bytes: 4_096,
            max_message_bytes: 64,
            max_age: Duration::from_secs(60),
            max_per_fetch: 3,
        };
        let (restored, receipt) =
            PropagationStore::restore(small_message_limit, &snapshot, 10.0).unwrap();
        assert_eq!(receipt.rejected_too_large, 3);
        assert!(restored.is_empty());
    }

    #[test]
    fn corrupt_or_unknown_store_snapshots_are_rejected_atomically() {
        let limits = PropagationStoreLimits::default();
        let empty = PropagationStore::new(limits.clone())
            .encode_snapshot()
            .unwrap();
        let mut wrong_magic = decode_one(&empty).unwrap();
        let Value::Array(parts) = &mut wrong_magic else {
            unreachable!()
        };
        parts[0] = Value::Binary(b"not-outrider".to_vec());
        assert!(matches!(
            PropagationStore::restore(limits.clone(), &encode_value(&wrong_magic).unwrap(), 1.0),
            Err(PropagationError::InvalidStoreSnapshot)
        ));

        let mut wrong_version = decode_one(&empty).unwrap();
        let Value::Array(parts) = &mut wrong_version else {
            unreachable!()
        };
        parts[1] = Value::from(2);
        assert!(matches!(
            PropagationStore::restore(limits.clone(), &encode_value(&wrong_version).unwrap(), 1.0),
            Err(PropagationError::UnsupportedStoreSnapshotVersion(2))
        ));

        let mut trailing = empty;
        trailing.push(0);
        assert!(matches!(
            PropagationStore::restore(limits, &trailing, 1.0),
            Err(PropagationError::MalformedMessagePack)
        ));

        assert!(matches!(
            PropagationStore::restore_bounded(
                PropagationStoreLimits::default(),
                &trailing,
                trailing.len() - 1,
                1.0,
            ),
            Err(PropagationError::StoreSnapshotTooLarge)
        ));
    }

    #[test]
    fn captured_fetch_requests_decode_to_offer_and_selection() {
        let offer =
            hex::decode("93cb41da9a05b2533e92c4109dc1a72883468f57fed571e796e9ce9892c0c0").unwrap();
        decode_offer_request(&offer).unwrap();

        let followup = hex::decode(
            "93cb41da9a060120c7b3c4109dc1a72883468f57fed571e796e9ce989391c420444444444444444444444444444444444444444444444444444444444444444490cd03e8",
        )
        .unwrap();
        let (wanted, handled, limit) = decode_fetch_selection(&followup).unwrap();
        assert_eq!(wanted, vec![[0x44; 32]]);
        assert!(handled.is_empty());
        assert_eq!(limit, FETCH_LIMIT);
    }
}
