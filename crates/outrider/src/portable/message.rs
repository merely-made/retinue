//! The LXMF object: header, payload, message id and signing preimage.

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use super::msgpack::{
    at_map, read_array_len, read_bin, read_f64, read_text, skip, write_bin, write_f64, write_text,
};

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
    #[error("LXMF title and content must be MessagePack binary or UTF-8 string values")]
    InvalidTextParts,
    #[error("LXMF fields must be a MessagePack map")]
    InvalidFields,
    #[error("LXMF stamp must be a MessagePack binary value")]
    InvalidStamp,
    #[error("LXMF payload could not be encoded")]
    Encode,
}

/// Which text parts travel as MessagePack str rather than bin. Stock writes bin, and its decode
/// takes either without checking (`LXMessage.py` 772-774, 807-808), so a str part is accepted
/// as its UTF-8 bytes and written back as str.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StrParts {
    pub title: bool,
    pub content: bool,
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
    pub str_parts: StrParts,
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
            str_parts: StrParts::default(),
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

/// One LXMF object, located in its bytes but not copied out of them.
pub(crate) struct Parts<'a> {
    pub destination: [u8; DESTINATION_LEN],
    pub source: [u8; SOURCE_LEN],
    pub signature: [u8; SIGNATURE_LEN],
    pub timestamp: f64,
    pub title: &'a [u8],
    pub content: &'a [u8],
    pub str_parts: StrParts,
    pub fields: &'a [u8],
    pub stamp: Option<&'a [u8]>,
    /// The unstamped payload the id covers: `hashed_head ‖ hashed_body`.
    hashed_head: &'static [u8],
    hashed_body: &'a [u8],
}

/// Locate every part of one complete LXMF object.
///
/// The message id covers the unstamped four-element payload. Unstamped, that is the payload
/// as received. Stamped, stock re-packs the first four elements (`LXMessage.py` 762-769);
/// here they are hashed as received behind a four-element header instead, which is the same
/// bytes for every canonical encoder, stock's included. A stamped payload whose first four
/// elements are not canonically encoded therefore gets the id its sender hashed, where stock
/// would compute another and refuse the signature.
pub(crate) fn parse(bytes: &[u8]) -> Result<Parts<'_>, CodecError> {
    if bytes.len() < HEADER_LEN {
        return Err(CodecError::TruncatedHeader);
    }
    let encoded = &bytes[HEADER_LEN..];
    let mut at = 0;
    let parts = read_array_len(encoded, &mut at)?;
    if !(4..=5).contains(&parts) {
        return Err(CodecError::InvalidPayloadShape);
    }
    let body_start = at;
    let timestamp = read_f64(encoded, &mut at)?;
    if !timestamp.is_finite() {
        return Err(CodecError::InvalidTimestamp);
    }
    let (title, title_str) = read_text(encoded, &mut at)?;
    let (content, content_str) = read_text(encoded, &mut at)?;

    // Find where the map ends rather than parsing what is in it.
    let fields_start = at;
    if !at_map(encoded, at) {
        return Err(CodecError::InvalidFields);
    }
    skip(encoded, &mut at)?;
    let fields = &encoded[fields_start..at];

    let (stamp, hashed_head, hashed_body) = if parts == 5 {
        let body = &encoded[body_start..at];
        let stamp = read_bin(encoded, &mut at).map_err(|_| CodecError::InvalidStamp)?;
        (Some(stamp), &[0x94_u8][..], body)
    } else {
        (None, &[][..], encoded)
    };
    if at != encoded.len() {
        return Err(CodecError::MalformedMessagePack);
    }
    Ok(Parts {
        destination: bytes[..DESTINATION_LEN].try_into().unwrap(),
        source: bytes[DESTINATION_LEN..DESTINATION_LEN + SOURCE_LEN]
            .try_into()
            .unwrap(),
        signature: bytes[DESTINATION_LEN + SOURCE_LEN..HEADER_LEN]
            .try_into()
            .unwrap(),
        timestamp,
        title,
        content,
        str_parts: StrParts {
            title: title_str,
            content: content_str,
        },
        fields,
        stamp,
        hashed_head,
        hashed_body,
    })
}

impl Parts<'_> {
    /// The message id and the signing preimage, from the hashed span in place.
    pub(crate) fn identity(&self) -> ([u8; 32], Vec<u8>) {
        hashed_identity(
            self.destination,
            self.source,
            self.hashed_head,
            self.hashed_body,
        )
    }
}

/// The message id and signing preimage of an unstamped payload given as `head ‖ body`.
pub(crate) fn hashed_identity(
    destination: [u8; DESTINATION_LEN],
    source: [u8; SOURCE_LEN],
    head: &[u8],
    body: &[u8],
) -> ([u8; 32], Vec<u8>) {
    let mut preimage =
        Vec::with_capacity(DESTINATION_LEN + SOURCE_LEN + head.len() + body.len() + 32);
    preimage.extend_from_slice(&destination);
    preimage.extend_from_slice(&source);
    preimage.extend_from_slice(head);
    preimage.extend_from_slice(body);
    let message_id: [u8; 32] = Sha256::digest(&preimage).into();
    preimage.extend_from_slice(&message_id);
    (message_id, preimage)
}

/// Decode one complete LXMF object.
pub fn decode(bytes: &[u8]) -> Result<Decoded, CodecError> {
    let parts = parse(bytes)?;
    let (message_id, signing_bytes) = parts.identity();
    Ok(Decoded {
        destination: parts.destination,
        source: parts.source,
        signature: parts.signature,
        payload: Payload {
            timestamp: parts.timestamp,
            title: parts.title.to_vec(),
            content: parts.content.to_vec(),
            fields: parts.fields.to_vec(),
            stamp: parts.stamp.map(<[u8]>::to_vec),
            str_parts: parts.str_parts,
        },
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
    write_text(&mut out, &payload.title, payload.str_parts.title)?;
    write_text(&mut out, &payload.content, payload.str_parts.content)?;
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
