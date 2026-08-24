//! The LXMF message codec, written without `std` and without a MessagePack value tree.
//!
//! # Why a second codec exists, and what has to be true before it replaces the first
//!
//! A board is meant to become an LXMF endpoint: hold a delivery destination, take a message,
//! and show it, with no computer attached. [`crate::codec`] cannot go there. It reads the
//! payload into an `rmpv::Value`, and `rmpv` has no `no_std` mode — only the lower-level
//! `rmp` does. So the question this module answers is whether the codec *needs* a value tree
//! at all.
//!
//! It does not, and the evidence is in the crate: nothing anywhere reads inside `fields`. It
//! is decoded, checked to be a map, carried, and written back out. A receiver that never
//! interprets a structure does not need to parse it — it needs to find where it ends. So here
//! `fields` is the **original bytes**, sliced out of the message and spliced back in verbatim,
//! which is not merely equivalent to re-encoding but strictly more faithful: a map that
//! arrived in a wider encoding than it needed goes back out the way it came.
//!
//! Nothing depends on this module yet, deliberately. It is built beside the shipping codec
//! rather than replacing it, because that codec is byte-exact against a stock LXMF oracle and
//! its correctness is the whole crate's correctness. The bar for the swap is below, as tests:
//! this codec must reproduce the oracle capture's bytes, message id and signing preimage
//! exactly, and must agree with [`crate::codec`] on messages that codec produces. Those hold.
//! The swap is now a deletion rather than a leap, and waits only on a board that wants it.
//!
//! # What "runs on a board" is worth, measured
//!
//! Being written without `std` is a property of the source; being *buildable* for a board is
//! a property of the whole dependency graph, and the two are easy to confuse. When this
//! module was first written the crate would not build for `thumbv7em-none-eabihf` at all, and
//! the first thing in the way was not `rmpv` but `socket2`, arriving through retinue's TCP
//! interface. Outrider now carries the feature shape retinue already had:
//!
//! ```text
//! cargo build -p outrider --no-default-features --target thumbv7em-none-eabihf
//! ```
//!
//! which builds this module and nothing else. Its own `sha2` was held level with retinue's at
//! the same time; before that, a board linking both crates carried two SHA-256
//! implementations, two `digest`s and two block buffers, for one hash.
//!
//! [`crate::stamp`] came along the same way, and needed no second copy of itself: its whole
//! tie to `rmpv` was one integer encode, replaced by a narrow-width encoder held against
//! `rmpv`'s output at every boundary. So a board can now read a message, know its identity,
//! and weigh the work on it.

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

pub const DESTINATION_LEN: usize = 16;
pub const SOURCE_LEN: usize = 16;
pub const SIGNATURE_LEN: usize = 64;
pub const HEADER_LEN: usize = DESTINATION_LEN + SOURCE_LEN + SIGNATURE_LEN;

/// Every way an LXMF message can fail to be one.
///
/// Defined here rather than in [`crate::codec`] and re-exported from there, so that the
/// vocabulary survives that module's eventual deletion, and so a board can name these
/// failures without linking a MessagePack value tree.
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
    // The identity a message is known by is computed over its *unstamped* form, so a stamped
    // message is re-encoded without its stamp before hashing. A four-part message already is
    // that form, and reusing its bytes avoids re-encoding what arrived.
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

// --- MessagePack, only as much as this payload shape needs -------------------------------

fn byte(bytes: &[u8], at: usize) -> Result<u8, CodecError> {
    bytes
        .get(at)
        .copied()
        .ok_or(CodecError::MalformedMessagePack)
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, len: usize) -> Result<&'a [u8], CodecError> {
    let end = at
        .checked_add(len)
        .ok_or(CodecError::MalformedMessagePack)?;
    let slice = bytes
        .get(*at..end)
        .ok_or(CodecError::MalformedMessagePack)?;
    *at = end;
    Ok(slice)
}

fn be(bytes: &[u8]) -> usize {
    bytes
        .iter()
        .fold(0_usize, |value, b| (value << 8) | *b as usize)
}

fn at_map(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at) {
        Some(marker) => matches!(marker, 0x80..=0x8f | 0xde | 0xdf),
        None => false,
    }
}

fn read_array_len(bytes: &[u8], at: &mut usize) -> Result<usize, CodecError> {
    let marker = byte(bytes, *at)?;
    *at += 1;
    match marker {
        0x90..=0x9f => Ok((marker & 0x0f) as usize),
        0xdc => Ok(be(take(bytes, at, 2)?)),
        0xdd => Ok(be(take(bytes, at, 4)?)),
        _ => Err(CodecError::InvalidPayloadShape),
    }
}

fn read_f64(bytes: &[u8], at: &mut usize) -> Result<f64, CodecError> {
    let marker = byte(bytes, *at)?;
    *at += 1;
    match marker {
        0xcb => {
            let raw: [u8; 8] = take(bytes, at, 8)?.try_into().unwrap();
            Ok(f64::from_be_bytes(raw))
        }
        // Stock LXMF writes a double, and accepting a float32 here would let a re-encode
        // widen it and change every hash downstream. Refused rather than promoted.
        _ => Err(CodecError::InvalidTimestamp),
    }
}

fn read_bin<'a>(bytes: &'a [u8], at: &mut usize) -> Result<&'a [u8], CodecError> {
    let marker = byte(bytes, *at)?;
    *at += 1;
    let len = match marker {
        0xc4 => be(take(bytes, at, 1)?),
        0xc5 => be(take(bytes, at, 2)?),
        0xc6 => be(take(bytes, at, 4)?),
        _ => return Err(CodecError::InvalidTextParts),
    };
    take(bytes, at, len)
}

/// Advance past one complete MessagePack value, whatever it is.
///
/// The only structural knowledge this codec needs about `fields`: where it stops. Written as
/// a full skipper rather than a map-only one because a map's values may be anything, and a
/// skipper that handled only the shapes seen so far would fail on the first message carrying
/// something new — silently, by mis-slicing the rest of the payload.
fn skip(bytes: &[u8], at: &mut usize) -> Result<(), CodecError> {
    skip_nested(bytes, at, 0)
}

/// How deep a field map may nest before this refuses to follow it.
///
/// Every level of an array or map costs a stack frame here, and this parser's whole purpose
/// is to read bytes that arrived over the air from someone we have not met. A message of
/// nothing but `0x91` -- a one-element array, repeated -- is a handful of bytes per level,
/// so a 500-byte LXMF payload buys about five hundred frames. On a board whose whole stack
/// is measured in kilobytes that is not a parse failure, it is a reset, and on a host it is
/// an abort. Neither reports what happened.
///
/// Sixteen is far past anything LXMF puts in a field map, which is one level of key-value
/// pairs holding scalars, and shallow enough that the worst case costs nothing.
const MAX_NESTING: u32 = 16;

fn skip_nested(bytes: &[u8], at: &mut usize, depth: u32) -> Result<(), CodecError> {
    if depth > MAX_NESTING {
        return Err(CodecError::MalformedMessagePack);
    }
    let marker = byte(bytes, *at)?;
    *at += 1;
    match marker {
        // Fixed-width scalars, positive and negative fixint, nil, and booleans.
        0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => Ok(()),
        0xcc | 0xd0 => take(bytes, at, 1).map(|_| ()),
        0xcd | 0xd1 => take(bytes, at, 2).map(|_| ()),
        0xce | 0xd2 | 0xca => take(bytes, at, 4).map(|_| ()),
        0xcf | 0xd3 | 0xcb => take(bytes, at, 8).map(|_| ()),
        // Strings, binaries and extensions: a length, then that many bytes.
        0xa0..=0xbf => take(bytes, at, (marker & 0x1f) as usize).map(|_| ()),
        0xd9 | 0xc4 => {
            let len = be(take(bytes, at, 1)?);
            take(bytes, at, len).map(|_| ())
        }
        0xda | 0xc5 => {
            let len = be(take(bytes, at, 2)?);
            take(bytes, at, len).map(|_| ())
        }
        0xdb | 0xc6 => {
            let len = be(take(bytes, at, 4)?);
            take(bytes, at, len).map(|_| ())
        }
        0xd4..=0xd8 => {
            let len = 1_usize << (marker - 0xd4);
            take(bytes, at, len + 1).map(|_| ())
        }
        0xc7 => {
            let len = be(take(bytes, at, 1)?);
            take(bytes, at, len + 1).map(|_| ())
        }
        0xc8 => {
            let len = be(take(bytes, at, 2)?);
            take(bytes, at, len + 1).map(|_| ())
        }
        0xc9 => {
            let len = be(take(bytes, at, 4)?);
            take(bytes, at, len + 1).map(|_| ())
        }
        // Containers: skip each element in turn. Maps hold two values per entry.
        0x90..=0x9f => skip_many(bytes, at, (marker & 0x0f) as usize, depth),
        0xdc => {
            let len = be(take(bytes, at, 2)?);
            skip_many(bytes, at, len, depth)
        }
        0xdd => {
            let len = be(take(bytes, at, 4)?);
            skip_many(bytes, at, len, depth)
        }
        0x80..=0x8f => skip_many(bytes, at, (marker & 0x0f) as usize * 2, depth),
        0xde => {
            let len = be(take(bytes, at, 2)?);
            skip_many(
                bytes,
                at,
                len.checked_mul(2).ok_or(CodecError::MalformedMessagePack)?,
                depth,
            )
        }
        0xdf => {
            let len = be(take(bytes, at, 4)?);
            skip_many(
                bytes,
                at,
                len.checked_mul(2).ok_or(CodecError::MalformedMessagePack)?,
                depth,
            )
        }
        // 0xc1 is never a valid MessagePack value.
        0xc1 => Err(CodecError::MalformedMessagePack),
    }
}

fn skip_many(bytes: &[u8], at: &mut usize, count: usize, depth: u32) -> Result<(), CodecError> {
    for _ in 0..count {
        skip_nested(bytes, at, depth + 1)?;
    }
    Ok(())
}

fn write_f64(out: &mut Vec<u8>, value: f64) {
    out.push(0xcb);
    out.extend_from_slice(&value.to_be_bytes());
}

/// Write a binary in the shortest encoding that holds it, which is what stock MessagePack
/// writers do and therefore what byte-exactness requires.
fn write_bin(out: &mut Vec<u8>, bytes: &[u8]) {
    let len = bytes.len();
    if len <= u8::MAX as usize {
        out.push(0xc4);
        out.push(len as u8);
    } else if len <= u16::MAX as usize {
        out.push(0xc5);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(0xc6);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

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

    /// The bar, first half: the stock LXMF 0.9.6 capture decodes to the same facts the
    /// shipping codec reads from it, including the message id and the signing preimage that
    /// every signature downstream depends on.
    #[cfg(feature = "std")]
    #[test]
    fn the_oracle_capture_decodes_to_the_same_facts() {
        let oracle = oracle();
        let packed = hex::decode(&oracle.packed).unwrap();
        let mine = decode(&packed).unwrap();
        let theirs = crate::codec::decode(&packed).unwrap();

        assert_eq!(mine.destination, array(&oracle.destination));
        assert_eq!(mine.source, array(&oracle.source));
        assert_eq!(mine.signature, array(&oracle.signature));
        assert_eq!(mine.message_id, array::<32>(&oracle.message_id));
        assert_eq!(mine.payload.timestamp, 1_753_603_200.5);
        assert_eq!(mine.payload.title, b"TITLE");
        assert_eq!(mine.payload.content, b"BODY");
        // The whole design claim, made concrete: the fields map is carried as the bytes it
        // arrived as, and those bytes are a map holding key 7.
        assert_eq!(
            mine.payload.fields,
            vec![0x81, 0x07, 0xc4, 0x04, b'm', b'e', b't', b'a']
        );

        assert_eq!(mine.message_id, theirs.message_id);
        assert_eq!(mine.signing_bytes(), theirs.signing_bytes());
    }

    /// The bar, second half: re-encoding reproduces the captured bytes exactly. This is what
    /// says the codec can be swapped rather than merely read.
    #[test]
    fn re_encoding_the_oracle_payload_reproduces_its_exact_bytes() {
        let oracle = oracle();
        let packed = hex::decode(&oracle.packed).unwrap();
        let decoded = decode(&packed).unwrap();

        let re_encoded = encode_payload(&decoded.payload, false).unwrap();
        assert_eq!(
            re_encoded,
            &packed[HEADER_LEN..],
            "a decoded payload must re-encode to the bytes it came from",
        );
    }

    /// Every message the shipping codec can produce, this one reads identically. Swept over
    /// lengths that cross MessagePack's bin8/bin16 boundary, because the encoding of a
    /// binary changes there and a codec that agreed only on short strings would pass a
    /// single-vector test and fail on a real message.
    #[cfg(feature = "std")]
    #[test]
    fn the_two_codecs_agree_across_the_binary_length_boundary() {
        for len in [0_usize, 1, 31, 254, 255, 256, 257, 1000] {
            let content = vec![0xab_u8; len];
            let payload = crate::codec::LxmfPayload::text(1_753_603_200.5, b"t", content.clone());
            let prepared = crate::codec::prepare([9; 16], [8; 16], &payload).unwrap();
            let packed = prepared.finish([7; 64]);

            let mine = decode(&packed).unwrap();
            let theirs = crate::codec::decode(&packed).unwrap();
            assert_eq!(mine.message_id, theirs.message_id, "len {len}");
            assert_eq!(mine.signing_bytes(), theirs.signing_bytes(), "len {len}");
            assert_eq!(mine.payload.content, content, "len {len}");
            assert_eq!(
                encode_payload(&mine.payload, false).unwrap(),
                &packed[HEADER_LEN..],
                "len {len}: re-encode must be byte-exact",
            );
        }
    }

    /// A stamp does not change the identity of the message it rides on, which is the property
    /// the unstamped re-encode exists to preserve.
    #[test]
    fn a_stamp_does_not_change_the_message_id() {
        let mut payload = Payload::text(1_753_603_200.5, b"title", b"body");
        let unstamped = encode_payload(&payload, false).unwrap();
        let id = message_id([1; 16], [2; 16], &unstamped);

        payload.stamp = Some(vec![3; 16]);
        let stamped = encode_payload(&payload, true).unwrap();
        let mut packed = Vec::new();
        packed.extend_from_slice(&[1_u8; 16]);
        packed.extend_from_slice(&[2_u8; 16]);
        packed.extend_from_slice(&[4_u8; 64]);
        packed.extend_from_slice(&stamped);

        let decoded = decode(&packed).unwrap();
        assert_eq!(decoded.message_id, id);
        assert_eq!(decoded.payload.stamp, Some(vec![3; 16]));
    }

    /// The skipper is the load-bearing primitive: it decides where `fields` ends, so a value
    /// it mis-measures silently mis-slices everything after it. Swept over every container
    /// and width a field map may legally hold.
    #[test]
    fn the_skipper_measures_every_value_shape() {
        let cases: Vec<Vec<u8>> = vec![
            vec![0xc0],                                     // nil
            vec![0xc2],                                     // false
            vec![0x07],                                     // positive fixint
            vec![0xff],                                     // negative fixint
            vec![0xcc, 0x80],                               // uint8
            vec![0xcd, 0x01, 0x02],                         // uint16
            vec![0xce, 1, 2, 3, 4],                         // uint32
            vec![0xcf, 1, 2, 3, 4, 5, 6, 7, 8],             // uint64
            vec![0xca, 1, 2, 3, 4],                         // float32
            vec![0xcb, 1, 2, 3, 4, 5, 6, 7, 8],             // float64
            vec![0xa3, b'a', b'b', b'c'],                   // fixstr
            vec![0xd9, 2, b'h', b'i'],                      // str8
            vec![0xc4, 2, 1, 2],                            // bin8
            vec![0xc5, 0, 2, 1, 2],                         // bin16
            vec![0x80],                                     // empty map
            vec![0x81, 0x07, 0xc4, 0x01, 0x09],             // fixmap with a bin value
            vec![0x92, 0x01, 0x02],                         // array of two
            vec![0x81, 0x01, 0x92, 0x01, 0x81, 0x02, 0x03], // nesting
            vec![0xd4, 0x00, 0x01],                         // fixext1
            vec![0xc7, 0x02, 0x00, 0x01, 0x02],             // ext8
        ];
        for case in cases {
            let mut at = 0;
            skip(&case, &mut at).unwrap_or_else(|error| panic!("{case:02x?}: {error}"));
            assert_eq!(at, case.len(), "{case:02x?} was mis-measured");
        }

        // And a value that runs off the end is refused rather than measured optimistically.
        let mut at = 0;
        assert!(skip(&[0xc4, 40, 1, 2], &mut at).is_err());
    }

    /// This parser reads bytes that arrived over the air from someone we have not met, and
    /// every level of nesting costs a stack frame. A message of nothing but `0x91` -- a
    /// one-element array, repeated -- is one byte per level, so a 500-byte payload used to
    /// buy five hundred frames. On a board whose whole stack is kilobytes that is a reset,
    /// not a parse error, and it reports nothing.
    #[test]
    fn deep_nesting_is_refused_rather_than_recursed() {
        // Well inside the limit: still read normally.
        let mut shallow = alloc::vec![0x91_u8; MAX_NESTING as usize - 2];
        shallow.push(0xc0); // nil, to terminate the innermost array
        let mut at = 0;
        assert!(
            skip(&shallow, &mut at).is_ok(),
            "ordinary nesting still parses"
        );

        // Past it: refused, and the refusal is a value rather than a crash.
        let mut deep = alloc::vec![0x91_u8; 400];
        deep.push(0xc0);
        let mut at = 0;
        assert_eq!(
            skip(&deep, &mut at),
            Err(CodecError::MalformedMessagePack),
            "a nest deeper than the limit is refused, not followed",
        );
    }

    /// A map header names a count and each entry is two values, so the count is doubled
    /// before anything is read. On the 32-bit boards this targets, `u32::MAX * 2` wraps to a
    /// small number, and a header promising four billion entries would quietly become a
    /// header promising a handful -- the rest of the message then parsed as those entries.
    /// The multiplication is checked for that reason.
    ///
    /// This test cannot witness the wrap on a 64-bit host, where the product fits; what it
    /// does hold everywhere is that an impossible count is refused rather than acted on.
    #[test]
    fn an_absurd_map_count_is_refused() {
        let mut bytes = alloc::vec![0xdf_u8];
        bytes.extend_from_slice(&u32::MAX.to_be_bytes());
        let mut at = 0;
        assert_eq!(
            skip(&bytes, &mut at),
            Err(CodecError::MalformedMessagePack),
            "four billion entries in five bytes is refused, not attempted",
        );
    }

    #[test]
    fn malformed_messages_are_refused() {
        assert_eq!(decode(&[0; 95]), Err(CodecError::TruncatedHeader));
        let oracle = oracle();
        let mut trailing = hex::decode(&oracle.packed).unwrap();
        trailing.push(0);
        assert_eq!(decode(&trailing), Err(CodecError::MalformedMessagePack));
        // Fields that are not a map are refused, which is the one thing this codec asserts
        // about a structure it otherwise never looks inside.
        let mut payload = Payload::text(1.0, b"t", b"b");
        payload.fields = vec![0x92, 0x01, 0x02];
        assert_eq!(
            encode_payload(&payload, false),
            Err(CodecError::InvalidFields)
        );
    }
}
