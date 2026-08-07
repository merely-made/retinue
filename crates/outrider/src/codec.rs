//! Bounded LXMF message parsing and encoding.
//!
//! This crate ends at the protocol boundary. It does not define Retinue,
//! Commons, identity, routing, storage, or delivery semantics.

use std::io::Cursor;

use rmpv::Value;
use sha2::{Digest, Sha256};

// The wire's fixed shape and its failure vocabulary are defined in the `no_std` codec and
// re-exported here, so that every path into this crate keeps naming them the same way.
pub use crate::portable::{CodecError, DESTINATION_LEN, HEADER_LEN, SIGNATURE_LEN, SOURCE_LEN};

pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// LXMF's MessagePack payload, in interoperable wire order:
/// timestamp, title, content, fields, and an optional stamp.
#[derive(Clone, Debug, PartialEq)]
pub struct LxmfPayload {
    pub timestamp: f64,
    pub title: Vec<u8>,
    pub content: Vec<u8>,
    pub fields: Value,
    pub stamp: Option<Vec<u8>>,
}

impl LxmfPayload {
    pub fn text(timestamp: f64, title: impl Into<Vec<u8>>, content: impl Into<Vec<u8>>) -> Self {
        Self {
            timestamp,
            title: title.into(),
            content: content.into(),
            fields: Value::Map(Vec::new()),
            stamp: None,
        }
    }
}

/// A parsed LXMF object. Signature verification remains with the caller's
/// Reticulum identity resolver because the 16-byte source hash is not a key.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedLxmf {
    pub destination: [u8; DESTINATION_LEN],
    pub source: [u8; SOURCE_LEN],
    pub signature: [u8; SIGNATURE_LEN],
    pub payload: LxmfPayload,
    pub message_id: [u8; 32],
    signing_bytes: Vec<u8>,
}

impl DecodedLxmf {
    pub fn signing_bytes(&self) -> &[u8] {
        &self.signing_bytes
    }

    pub fn verify_with(&self, verify: impl FnOnce(&[u8], &[u8; SIGNATURE_LEN]) -> bool) -> bool {
        verify(&self.signing_bytes, &self.signature)
    }
}

/// Prepared LXMF bytes before the Reticulum identity signs them.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedLxmf {
    destination: [u8; DESTINATION_LEN],
    source: [u8; SOURCE_LEN],
    payload: Vec<u8>,
    pub message_id: [u8; 32],
    signing_bytes: Vec<u8>,
}

impl PreparedLxmf {
    pub fn signing_bytes(&self) -> &[u8] {
        &self.signing_bytes
    }

    pub fn finish(self, signature: [u8; SIGNATURE_LEN]) -> Vec<u8> {
        let mut packed = Vec::with_capacity(HEADER_LEN + self.payload.len());
        packed.extend_from_slice(&self.destination);
        packed.extend_from_slice(&self.source);
        packed.extend_from_slice(&signature);
        packed.extend_from_slice(&self.payload);
        packed
    }
}

/// Prepare an LXMF object and exact signature preimage.
pub fn prepare(
    destination: [u8; DESTINATION_LEN],
    source: [u8; SOURCE_LEN],
    payload: &LxmfPayload,
) -> Result<PreparedLxmf, CodecError> {
    validate_payload(payload)?;
    let hashed_payload = encode_payload(payload, false)?;
    let message_id = message_id(destination, source, &hashed_payload);
    let signing_bytes = signing_bytes(destination, source, &hashed_payload, message_id);
    let packed_payload = encode_payload(payload, payload.stamp.is_some())?;
    if HEADER_LEN + packed_payload.len() > DEFAULT_MAX_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    Ok(PreparedLxmf {
        destination,
        source,
        payload: packed_payload,
        message_id,
        signing_bytes,
    })
}

pub fn decode(bytes: &[u8]) -> Result<DecodedLxmf, CodecError> {
    decode_bounded(bytes, DEFAULT_MAX_MESSAGE_BYTES)
}

pub fn decode_bounded(bytes: &[u8], max_message_bytes: usize) -> Result<DecodedLxmf, CodecError> {
    if bytes.len() > max_message_bytes {
        return Err(CodecError::TooLarge);
    }
    if bytes.len() < HEADER_LEN {
        return Err(CodecError::TruncatedHeader);
    }
    let destination = bytes[..DESTINATION_LEN].try_into().unwrap();
    let source = bytes[DESTINATION_LEN..DESTINATION_LEN + SOURCE_LEN]
        .try_into()
        .unwrap();
    let signature = bytes[DESTINATION_LEN + SOURCE_LEN..HEADER_LEN]
        .try_into()
        .unwrap();
    let encoded_payload = &bytes[HEADER_LEN..];
    let mut cursor = Cursor::new(encoded_payload);
    let value =
        rmpv::decode::read_value(&mut cursor).map_err(|_| CodecError::MalformedMessagePack)?;
    if cursor.position() as usize != encoded_payload.len() {
        return Err(CodecError::MalformedMessagePack);
    }
    let Value::Array(parts) = value else {
        return Err(CodecError::InvalidPayloadShape);
    };
    if !(4..=5).contains(&parts.len()) {
        return Err(CodecError::InvalidPayloadShape);
    }
    let timestamp = match parts[0] {
        Value::F64(value) if value.is_finite() => value,
        _ => return Err(CodecError::InvalidTimestamp),
    };
    let (Value::Binary(title), Value::Binary(content)) = (&parts[1], &parts[2]) else {
        return Err(CodecError::InvalidTextParts);
    };
    if !matches!(parts[3], Value::Map(_)) {
        return Err(CodecError::InvalidFields);
    }
    let stamp = match parts.get(4) {
        Some(Value::Binary(stamp)) => Some(stamp.clone()),
        Some(_) => return Err(CodecError::InvalidStamp),
        None => None,
    };
    let payload = LxmfPayload {
        timestamp,
        title: title.clone(),
        content: content.clone(),
        fields: parts[3].clone(),
        stamp,
    };
    let hashed_payload = if parts.len() == 4 {
        encoded_payload.to_vec()
    } else {
        encode_payload(&payload, false)?
    };
    let message_id = message_id(destination, source, &hashed_payload);
    let signing_bytes = signing_bytes(destination, source, &hashed_payload, message_id);
    Ok(DecodedLxmf {
        destination,
        source,
        signature,
        payload,
        message_id,
        signing_bytes,
    })
}

fn validate_payload(payload: &LxmfPayload) -> Result<(), CodecError> {
    if !payload.timestamp.is_finite() {
        return Err(CodecError::InvalidTimestamp);
    }
    if !matches!(payload.fields, Value::Map(_)) {
        return Err(CodecError::InvalidFields);
    }
    Ok(())
}

fn encode_payload(payload: &LxmfPayload, include_stamp: bool) -> Result<Vec<u8>, CodecError> {
    let mut parts = vec![
        Value::F64(payload.timestamp),
        Value::Binary(payload.title.clone()),
        Value::Binary(payload.content.clone()),
        payload.fields.clone(),
    ];
    if include_stamp {
        parts.push(Value::Binary(payload.stamp.clone().unwrap_or_default()));
    }
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &Value::Array(parts)).map_err(|_| CodecError::Encode)?;
    Ok(bytes)
}

fn message_id(
    destination: [u8; DESTINATION_LEN],
    source: [u8; SOURCE_LEN],
    payload: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(destination);
    hasher.update(source);
    hasher.update(payload);
    hasher.finalize().into()
}

fn signing_bytes(
    destination: [u8; DESTINATION_LEN],
    source: [u8; SOURCE_LEN],
    payload: &[u8],
    message_id: [u8; 32],
) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(64 + payload.len());
    bytes.extend_from_slice(&destination);
    bytes.extend_from_slice(&source);
    bytes.extend_from_slice(payload);
    bytes.extend_from_slice(&message_id);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    struct OracleCapture {
        destination: String,
        source: String,
        message_id: String,
        signature: String,
        packed: String,
    }

    fn array<const N: usize>(hex_value: &str) -> [u8; N] {
        hex::decode(hex_value).unwrap().try_into().unwrap()
    }

    fn oracle() -> OracleCapture {
        serde_json::from_str(include_str!("../tests/fixtures/lxmf_0_9_6_message.json")).unwrap()
    }

    #[test]
    fn lxmf_0_9_6_oracle_capture_decodes_with_title_before_content() {
        let oracle = oracle();
        let packed = hex::decode(&oracle.packed).unwrap();
        let decoded = decode(&packed).unwrap();
        assert_eq!(decoded.destination, array(&oracle.destination));
        assert_eq!(decoded.source, array(&oracle.source));
        assert_eq!(decoded.signature, array(&oracle.signature));
        assert_eq!(decoded.message_id, array(&oracle.message_id));
        assert_eq!(decoded.payload.timestamp, 1_753_603_200.5);
        assert_eq!(decoded.payload.title, b"TITLE");
        assert_eq!(decoded.payload.content, b"BODY");
        assert_eq!(
            decoded.payload.fields,
            Value::Map(vec![(Value::from(7), Value::Binary(b"meta".to_vec()))])
        );
    }

    #[test]
    fn preparing_the_oracle_payload_reproduces_its_exact_bytes_and_id() {
        let oracle = oracle();
        let payload = LxmfPayload {
            timestamp: 1_753_603_200.5,
            title: b"TITLE".to_vec(),
            content: b"BODY".to_vec(),
            fields: Value::Map(vec![(Value::from(7), Value::Binary(b"meta".to_vec()))]),
            stamp: None,
        };
        let prepared =
            prepare(array(&oracle.destination), array(&oracle.source), &payload).unwrap();
        assert_eq!(prepared.message_id, array(&oracle.message_id));
        assert_eq!(
            prepared.finish(array(&oracle.signature)),
            hex::decode(&oracle.packed).unwrap()
        );
    }

    #[test]
    fn the_signature_verifier_is_an_explicit_identity_boundary() {
        let oracle = oracle();
        let decoded = decode(&hex::decode(&oracle.packed).unwrap()).unwrap();
        assert!(decoded.verify_with(|signed, signature| {
            signed == decoded.signing_bytes() && signature == &array(&oracle.signature)
        }));
    }

    #[test]
    fn an_optional_stamp_does_not_change_the_signed_message_id() {
        let mut payload = LxmfPayload::text(1_753_603_200.5, b"title", b"body");
        let unstamped = prepare([1; 16], [2; 16], &payload).unwrap();
        let message_id = unstamped.message_id;
        payload.stamp = Some(vec![3; 16]);
        let stamped = prepare([1; 16], [2; 16], &payload).unwrap();
        assert_eq!(stamped.message_id, message_id);
        let decoded = decode(&stamped.finish([4; 64])).unwrap();
        assert_eq!(decoded.message_id, message_id);
        assert_eq!(decoded.payload.stamp, Some(vec![3; 16]));
    }

    #[test]
    fn malformed_or_oversized_messages_are_refused_before_projection() {
        assert_eq!(decode(&[0; 95]), Err(CodecError::TruncatedHeader));
        assert_eq!(decode_bounded(&[0; 97], 96), Err(CodecError::TooLarge));
        let mut trailing = hex::decode(oracle().packed).unwrap();
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(CodecError::MalformedMessagePack));
    }
}
