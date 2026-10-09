//! Wire values: the node announce, stamped entries, messages, and submission batches.

use retinue::hash::full_hash;
use retinue::identity::{Identity, PrivateIdentity};
use retinue::token::decrypt_to_identity;
use rmpv::Value;

use super::msgpack::{decode_one, encode_value};
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

/// The application data announced by an LXMF propagation node (`LXMRouter.py` 332-346):
/// `[legacy, timebase, active, transfer_limit, sync_limit, [costs...], metadata]`.
///
/// Decoding is as tolerant as stock's `pn_announce_data_is_valid` (`LXMF.py` 225-250): extra
/// elements and extra costs are ignored, the legacy flag may be any type, and numbers may be
/// anything `int()` takes, floats included (lxmd configures its limits as floats). A negative
/// timebase reads as 0 and costs saturate to 0..=255. Unknown metadata keys remain opaque
/// MessagePack.
#[derive(Clone, Debug, PartialEq)]
pub struct PropagationAnnounce {
    pub legacy: bool,
    /// The node's clock at announce. [`register_propagation`](super::register_propagation)
    /// and [`announce_propagation`](super::announce_propagation) set it to now.
    pub unix_time: u64,
    pub active: bool,
    /// Largest transfer the node accepts, in KB of 1000 bytes.
    pub transfer_limit_kb: f64,
    /// Largest peer sync the node accepts, in KB of 1000 bytes.
    pub sync_limit_kb: f64,
    pub costs: PropagationCosts,
    pub metadata: Vec<(Value, Value)>,
}

impl PropagationAnnounce {
    pub fn encode(&self) -> Result<Vec<u8>, PropagationError> {
        let value = Value::Array(vec![
            Value::Boolean(self.legacy),
            Value::from(self.unix_time),
            Value::Boolean(self.active),
            encode_number(self.transfer_limit_kb)?,
            encode_number(self.sync_limit_kb)?,
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
        let [
            legacy,
            unix_time,
            active,
            transfer,
            sync,
            costs,
            metadata,
            ..,
        ] = parts.as_slice()
        else {
            return Err(PropagationError::InvalidAnnounce);
        };
        let (Value::Array(costs), Value::Map(metadata)) = (costs, metadata) else {
            return Err(PropagationError::InvalidAnnounce);
        };
        let [propagation, flexibility, peering, ..] = costs.as_slice() else {
            return Err(PropagationError::InvalidAnnounce);
        };
        // Stock compares with `== True`, which a numeric 0 or 1 passes and text does not.
        let active = match active {
            Value::Boolean(active) => *active,
            Value::Integer(_) | Value::F32(_) | Value::F64(_) => match number(active)? {
                0.0 => false,
                1.0 => true,
                _ => return Err(PropagationError::InvalidAnnounce),
            },
            _ => return Err(PropagationError::InvalidAnnounce),
        };
        Ok(Self {
            legacy: matches!(legacy, Value::Boolean(true)),
            // Saturating: a negative timebase reads as 0.
            unix_time: number(unix_time)? as u64,
            active,
            transfer_limit_kb: number(transfer)?,
            sync_limit_kb: number(sync)?,
            costs: PropagationCosts {
                propagation: cost(propagation)?,
                flexibility: cost(flexibility)?,
                peering: cost(peering)?,
            },
            metadata: metadata.clone(),
        })
    }

    /// The transfer limit in bytes, rounded down, as stock compares it.
    pub fn transfer_limit_bytes(&self) -> u64 {
        (self.transfer_limit_kb * 1000.0) as u64
    }

    /// The sync limit in bytes, rounded down.
    pub fn sync_limit_bytes(&self) -> u64 {
        (self.sync_limit_kb * 1000.0) as u64
    }

    pub fn name(&self) -> Option<&[u8]> {
        self.metadata.iter().find_map(|(key, value)| {
            (key.as_u64() == Some(PROPAGATION_METADATA_NAME))
                .then(|| value.as_slice())
                .flatten()
        })
    }
}

/// A finite number as Python's `int()` takes it: an integer, float or bool, or text or
/// bytes spelling a decimal integer.
pub(super) fn number(value: &Value) -> Result<f64, PropagationError> {
    let text = |bytes: &[u8]| {
        let text = core::str::from_utf8(bytes).ok()?;
        text.trim().parse::<i64>().ok().map(|int| int as f64)
    };
    match value {
        Value::Integer(int) => int.as_f64(),
        Value::F32(float) => Some(f64::from(*float)),
        Value::F64(float) => Some(*float),
        Value::Boolean(flag) => Some(f64::from(u8::from(*flag))),
        Value::String(string) => text(string.as_bytes()),
        Value::Binary(bytes) => text(bytes),
        _ => None,
    }
    .filter(|number| number.is_finite())
    .ok_or(PropagationError::InvalidAnnounce)
}

/// A stamp cost, truncated as `int()` does and held to 0..=255 where stock keeps any integer.
fn cost(value: &Value) -> Result<u8, PropagationError> {
    Ok(number(value)?.trunc() as u8)
}

/// An integer when the value is whole, so integral limits stay byte-identical to stock's.
fn encode_number(value: f64) -> Result<Value, PropagationError> {
    if !value.is_finite() {
        return Err(PropagationError::InvalidAnnounce);
    }
    Ok(
        if value.fract() == 0.0 && (0.0..=u64::MAX as f64).contains(&value) {
            Value::from(value as u64)
        } else {
            Value::F64(value)
        },
    )
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
        // Stock's own type check here is always true (`LXMRouter.py` 2410); a number will do.
        let transfer_time = number(&parts[0]).map_err(|_| PropagationError::InvalidTransferTime)?;
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
