//! Wire values: the node announce, stamped entries, messages, and submission batches.

use retinue::hash::full_hash;
use retinue::identity::{Identity, PrivateIdentity};
use retinue::token::decrypt_to_identity;
use rmpv::Value;

use super::msgpack::{byte, decode_one, encode_value};
use super::{
    DEFAULT_MAX_PROPAGATION_ANNOUNCE_BYTES, DEFAULT_MAX_PROPAGATION_BATCH_BYTES,
    DEFAULT_MAX_PROPAGATION_ENTRIES, MIN_ENCRYPTED_MESSAGE_BYTES, PROPAGATION_METADATA_NAME,
    PropagationError,
};
use crate::announce::delivery_destination;
use crate::codec::{DEFAULT_MAX_MESSAGE_BYTES, DecodedLxmf, decode_bounded};
use crate::stamp::{PROPAGATION_WORKBLOCK_ROUNDS, STAMP_LEN, valid_streamed, value_streamed};

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
    pub(super) message: PropagationMessage,
    pub(super) stamp: [u8; STAMP_LEN],
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
        value_streamed(&transient_id, PROPAGATION_WORKBLOCK_ROUNDS, &self.stamp)
    }

    pub fn validate_stamp(&self, target: u16) -> bool {
        let transient_id = self.transient_id();
        valid_streamed(
            &transient_id,
            PROPAGATION_WORKBLOCK_ROUNDS,
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
    pub(super) destination: [u8; 16],
    pub(super) encrypted: Vec<u8>,
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
        self.open(&remainder, max_message_bytes)
    }

    /// Rebuild the signed LXMF object from this message's decrypted remainder.
    pub(super) fn open(
        &self,
        remainder: &[u8],
        max_message_bytes: usize,
    ) -> Result<DecodedLxmf, PropagationError> {
        let mut packed = Vec::with_capacity(16 + remainder.len());
        packed.extend_from_slice(&self.destination);
        packed.extend_from_slice(remainder);
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
