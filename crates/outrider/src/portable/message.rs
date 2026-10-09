//! The LXMF object: header, payload, message id and signing preimage.

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use super::msgpack::{at_map, read_array_len, read_bin, read_f64, skip, write_bin, write_f64};

pub const DESTINATION_LEN: usize = 16;
pub const SOURCE_LEN: usize = 16;
pub const SIGNATURE_LEN: usize = 64;
pub const HEADER_LEN: usize = DESTINATION_LEN + SOURCE_LEN + SIGNATURE_LEN;

/// Every way an LXMF message can fail to be one.
///
/// Defined here and re-exported by [`crate::codec`], so a board can name these failures
/// without linking a MessagePack value tree.
#[derive(Debug, PartialEq, thiserror::Error)]
pub enum CodecError {
    #[error("LXMF message exceeds the configured byte limit")]
    TooLarge,
    #[error("LXMF message is shorter than its fixed header")]
    TruncatedHeader,
    #[error("LXMF payload is not one complete MessagePack value")]
    MalformedMessagePack,
    #[error("LXMF payload must be a four- or five-item array")]
    InvalidPayloadShape,
    #[error("LXMF timestamp must be a finite double-precision value")]
    InvalidTimestamp,
    #[error("LXMF title and content must be MessagePack binary values")]
    InvalidTextParts,
    #[error("LXMF fields must be a MessagePack map")]
    InvalidFields,
    #[error("LXMF stamp must be a MessagePack binary value")]
    InvalidStamp,
    #[error("LXMF payload could not be encoded")]
    Encode,
}

/// An LXMF payload whose fields travel as the bytes they arrived as.
#[derive(Clone, Debug, PartialEq)]
pub struct Payload {
    pub timestamp: f64,
    pub title: Vec<u8>,
    pub content: Vec<u8>,
    /// The `fields` map, as MessagePack bytes. Never interpreted here.
    pub fields: Vec<u8>,
    pub stamp: Option<Vec<u8>>,
}

impl Payload {
    /// A text message with an empty field map (`0x80`, MessagePack's empty fixmap).
    pub fn text(timestamp: f64, title: impl Into<Vec<u8>>, content: impl Into<Vec<u8>>) -> Self {
        Self {
            timestamp,
            title: title.into(),
            content: content.into(),
            fields: alloc::vec![0x80],
            stamp: None,
        }
    }
}

/// A decoded LXMF object.
#[derive(Clone, Debug, PartialEq)]
pub struct Decoded {
    pub destination: [u8; DESTINATION_LEN],
    pub source: [u8; SOURCE_LEN],
    pub signature: [u8; SIGNATURE_LEN],
    pub payload: Payload,
    pub message_id: [u8; 32],
    signing_bytes: Vec<u8>,
}

impl Decoded {
    pub fn signing_bytes(&self) -> &[u8] {
        &self.signing_bytes
    }

    pub fn verify_with(&self, verify: impl FnOnce(&[u8], &[u8; SIGNATURE_LEN]) -> bool) -> bool {
        verify(&self.signing_bytes, &self.signature)
    }
}

/// Decode one complete LXMF object.
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError> {
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
    let encoded = &bytes[HEADER_LEN..];

    let mut at = 0;
    let parts = read_array_len(encoded, &mut at)?;
    if !(4..=5).contains(&parts) {
        return Err(CodecError::InvalidPayloadShape);
    }
    let timestamp = read_f64(encoded, &mut at)?;
    if !timestamp.is_finite() {
        return Err(CodecError::InvalidTimestamp);
    }
    let title = read_bin(encoded, &mut at)
        .map_err(|_| CodecError::InvalidTextParts)?
        .to_vec();
    let content = read_bin(encoded, &mut at)
        .map_err(|_| CodecError::InvalidTextParts)?
        .to_vec();

    // The whole point: find where the map ends rather than parsing what is in it.
    let fields_start = at;
    if !at_map(encoded, at) {
        return Err(CodecError::InvalidFields);
    }
    skip(encoded, &mut at)?;
    let fields = encoded[fields_start..at].to_vec();

    let stamp = if parts == 5 {
        Some(
            read_bin(encoded, &mut at)
                .map_err(|_| CodecError::InvalidStamp)?
                .to_vec(),
        )
    } else {
        None
    };
    if at != encoded.len() {
        return Err(CodecError::MalformedMessagePack);
    }

    let payload = Payload {
        timestamp,
        title,
        content,
        fields,
        stamp,
    };
    // The message id covers the *unstamped* form, so a stamped message is re-encoded
    // without its stamp; a four-part message already is that form.
    let hashed = if parts == 4 {
        encoded.to_vec()
    } else {
        encode_payload(&payload, false)?
    };
    let message_id = message_id(destination, source, &hashed);
    let signing_bytes = signing_bytes(destination, source, &hashed, message_id);
    Ok(Decoded {
        destination,
        source,
        signature,
        payload,
        message_id,
        signing_bytes,
    })
}

/// Encode a payload, with or without its stamp.
pub fn encode_payload(payload: &Payload, include_stamp: bool) -> Result<Vec<u8>, CodecError> {
    if !payload.timestamp.is_finite() {
        return Err(CodecError::InvalidTimestamp);
    }
    if !at_map(&payload.fields, 0) {
        return Err(CodecError::InvalidFields);
    }
    let parts: u8 = if include_stamp { 5 } else { 4 };
    let mut out = Vec::with_capacity(16 + payload.title.len() + payload.content.len());
    // A fixarray, which is what four or five items always encode as.
    out.push(0x90 | parts);
    write_f64(&mut out, payload.timestamp);
    write_bin(&mut out, &payload.title);
    write_bin(&mut out, &payload.content);
    out.extend_from_slice(&payload.fields);
    if include_stamp {
        write_bin(&mut out, payload.stamp.as_deref().unwrap_or_default());
    }
    Ok(out)
}

/// The message id: SHA-256 over destination, source, and the unstamped payload.
pub fn message_id(
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

/// The exact preimage an identity signs.
pub fn signing_bytes(
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
