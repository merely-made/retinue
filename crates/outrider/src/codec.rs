//! Bounded LXMF message parsing and encoding.
//!
//! This crate ends at the protocol boundary. It does not define Retinue,
//! Commons, identity, routing, storage, or delivery semantics.

use std::io::Cursor;

use rmpv::Value;

use crate::portable::{hashed_identity, parse, write_bin, write_f64, write_text};
// The wire's fixed shape and its failure vocabulary are defined in the `no_std` codec and
// re-exported here, so that every path into this crate keeps naming them the same way.
pub use crate::portable::{
    CodecError, DESTINATION_LEN, HEADER_LEN, SIGNATURE_LEN, SOURCE_LEN, StrParts,
};

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
    /// Text parts that travel as MessagePack str rather than bin.
    pub str_parts: StrParts,
}

impl LxmfPayload {
    pub fn text(timestamp: f64, title: impl Into<Vec<u8>>, content: impl Into<Vec<u8>>) -> Self {
        Self {
            timestamp,
            title: title.into(),
            content: content.into(),
            fields: Value::Map(Vec::new()),
            stamp: None,
            str_parts: StrParts::default(),
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
    /// The whole object, its signature still zero, so signing fills it in place.
    packed: Vec<u8>,
    pub message_id: [u8; 32],
    signing_bytes: Vec<u8>,
}

impl PreparedLxmf {
    pub fn signing_bytes(&self) -> &[u8] {
        &self.signing_bytes
    }

    pub fn finish(mut self, signature: [u8; SIGNATURE_LEN]) -> Vec<u8> {
        self.packed[DESTINATION_LEN + SOURCE_LEN..HEADER_LEN].copy_from_slice(&signature);
        self.packed
    }
}

/// Prepare an LXMF object and exact signature preimage.
pub fn prepare(
    destination: [u8; DESTINATION_LEN],
    source: [u8; SOURCE_LEN],
    payload: &LxmfPayload,
) -> Result<PreparedLxmf, CodecError> {
    if !payload.timestamp.is_finite() {
        return Err(CodecError::InvalidTimestamp);
    }
    if !matches!(payload.fields, Value::Map(_)) {
        return Err(CodecError::InvalidFields);
    }
    let mut packed = Vec::with_capacity(
        HEADER_LEN + 32 + payload.title.len() + payload.content.len() + STAMP_ROOM,
    );
    packed.extend_from_slice(&destination);
    packed.extend_from_slice(&source);
    packed.resize(HEADER_LEN, 0);
    // A fixarray, which is what four or five items always encode as.
    packed.push(if payload.stamp.is_some() { 0x95 } else { 0x94 });
    write_f64(&mut packed, payload.timestamp);
    write_text(&mut packed, &payload.title, payload.str_parts.title)?;
    write_text(&mut packed, &payload.content, payload.str_parts.content)?;
    rmpv::encode::write_value(&mut packed, &payload.fields).map_err(|_| CodecError::Encode)?;
    let body = &packed[HEADER_LEN + 1..];
    let (message_id, signing_bytes) = hashed_identity(destination, source, &[0x94], body);
    if let Some(stamp) = &payload.stamp {
        write_bin(&mut packed, stamp);
    }
    if packed.len() > DEFAULT_MAX_MESSAGE_BYTES {
        return Err(CodecError::TooLarge);
    }
    Ok(PreparedLxmf {
        packed,
        message_id,
        signing_bytes,
    })
}

/// Room for a stamp and a small field map, so a typical message is written without regrowth.
const STAMP_ROOM: usize = 64;

pub fn decode(bytes: &[u8]) -> Result<DecodedLxmf, CodecError> {
    decode_bounded(bytes, DEFAULT_MAX_MESSAGE_BYTES)
}

/// Decode with the `no_std` codec's parser, reading only the field map into a value tree.
/// The id covers the unstamped payload; see [`crate::portable`] for the rule on a stamped one.
pub fn decode_bounded(bytes: &[u8], max_message_bytes: usize) -> Result<DecodedLxmf, CodecError> {
    if bytes.len() > max_message_bytes {
        return Err(CodecError::TooLarge);
    }
    let parts = parse(bytes)?;
    let fields = rmpv::decode::read_value(&mut Cursor::new(parts.fields))
        .map_err(|_| CodecError::MalformedMessagePack)?;
    let (message_id, signing_bytes) = parts.identity();
    Ok(DecodedLxmf {
        destination: parts.destination,
        source: parts.source,
        signature: parts.signature,
        payload: LxmfPayload {
            timestamp: parts.timestamp,
            title: parts.title.to_vec(),
            content: parts.content.to_vec(),
            fields,
            stamp: parts.stamp.map(<[u8]>::to_vec),
            str_parts: parts.str_parts,
        },
        message_id,
        signing_bytes,
    })
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
        serde_json::from_str(include_str!("../tests/fixtures/lxmf_message.json")).unwrap()
    }

    #[test]
    fn stock_oracle_capture_decodes_with_title_before_content() {
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
            str_parts: StrParts::default(),
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
