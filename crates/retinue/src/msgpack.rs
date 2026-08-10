//! The MessagePack subset Reticulum actually puts on the wire.
//!
//! RNS reaches for MessagePack in exactly one place this crate cares about: the signed
//! artifact envelope ([`crate::artifact`]). That envelope is a map of short string keys to
//! binary blobs, with an operator-supplied metadata map that may hold strings, integers,
//! booleans, and arrays of the same. This module encodes and decodes that much and no more.
//!
//! Two properties are load-bearing.
//!
//! **Encoding is canonical.** Every value has exactly one representation here: the shortest
//! header that fits. RNS emits the same shortest-form encoding, so an envelope built here is
//! byte-identical to one built by `rnid`, which is what makes signature comparison against
//! the oracle vectors meaningful rather than merely plausible.
//!
//! **Decoding is bounded.** Nesting depth, element counts, and every length are checked
//! against the bytes that remain before anything is allocated. A map header claiming four
//! billion pairs costs a lookup and an error, not four billion pairs of capacity. This is
//! the same discipline `outrider::portable` applies to LXMF, for the same reason: these
//! bytes arrive from strangers.
//!
//! # Provenance
//!
//! Written here, for [`crate::artifact`]. No MessagePack implementation was read while
//! writing it; the subset and the shortest-form rule come from the MessagePack
//! specification, and the choice of which types to support comes from what RNS emits. The
//! artifact envelope this serves *is* layout-derived from Prns, and the attribution for
//! that lives in `crates/retinue/NOTICE`.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

/// How deep a decoded structure may nest before it is treated as hostile.
///
/// The envelope RNS defines is two deep (envelope map, metadata map) and its metadata values
/// go one deeper for arrays. Sixteen leaves an order of magnitude of headroom while keeping
/// the recursive decoder's stack use bounded on a board.
pub const MAX_DEPTH: u32 = 16;

/// A decoded MessagePack value, in the subset RNS emits.
///
/// Floats and extension types are deliberately absent: nothing in the artifact envelope uses
/// them, and a type this code cannot produce is a type it need not be trusted to parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Nil,
    Bool(bool),
    /// A non-negative integer. RNS's metadata integers arrive here.
    Uint(u64),
    /// A negative integer. Kept separate so encoding stays canonical in both directions.
    Int(i64),
    Str(String),
    Bin(Vec<u8>),
    Array(Vec<Value>),
    /// Key order is preserved. The signature covers the encoded bytes, so a map that
    /// re-sorted itself on decode could not be re-encoded into something that still verifies.
    Map(Vec<(Value, Value)>),
}

impl Value {
    /// The string, if this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Str(value) => Some(value),
            _ => None,
        }
    }

    /// The bytes, if this is a binary blob.
    pub fn as_bin(&self) -> Option<&[u8]> {
        match self {
            Self::Bin(value) => Some(value),
            _ => None,
        }
    }

    /// The entries, if this is a map.
    pub fn as_map(&self) -> Option<&[(Value, Value)]> {
        match self {
            Self::Map(entries) => Some(entries),
            _ => None,
        }
    }

    /// Look a string key up in a map, preserving the "first wins" rule a duplicate key would
    /// otherwise leave ambiguous.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_map()?
            .iter()
            .find(|(name, _)| name.as_str() == Some(key))
            .map(|(_, value)| value)
    }
}

/// Why a byte string is not a MessagePack value this codec accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The value ended before its declared length did.
    Truncated,
    /// A type this codec does not implement: a float, an extension, or a reserved byte.
    Unsupported,
    /// A string claimed to be UTF-8 and was not.
    BadUtf8,
    /// Nesting exceeded [`MAX_DEPTH`].
    TooDeep,
    /// Bytes remained after a complete value. An envelope is exactly one value.
    Trailing,
}

/// Encode one value in canonical shortest form.
pub fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Nil => out.push(0xc0),
        Value::Bool(false) => out.push(0xc2),
        Value::Bool(true) => out.push(0xc3),
        Value::Uint(n) => write_uint(*n, out),
        Value::Int(n) => write_int(*n, out),
        Value::Str(text) => {
            write_len(text.len(), 0xa0, 0xd9, 0xda, 0xdb, true, out);
            out.extend_from_slice(text.as_bytes());
        }
        Value::Bin(bytes) => {
            // Binary has no fixed-header form, so the fixed branch is never taken; the
            // sentinel below simply cannot match a length.
            write_len(bytes.len(), 0, 0xc4, 0xc5, 0xc6, false, out);
            out.extend_from_slice(bytes);
        }
        Value::Array(items) => {
            write_count(items.len(), 0x90, 0xdc, 0xdd, out);
            for item in items {
                write(item, out);
            }
        }
        Value::Map(entries) => {
            write_count(entries.len(), 0x80, 0xde, 0xdf, out);
            for (key, value) in entries {
                write(key, out);
                write(value, out);
            }
        }
    }
}

/// Ranges here are disjoint rather than cascading. Cascading arms would encode the same
/// bytes, since match arms are tried in order, but a reader has to know that to see it, and
/// this is a table where being wrong by one width is a silent wire bug.
fn write_uint(n: u64, out: &mut Vec<u8>) {
    match n {
        0..=0x7f => out.push(n as u8),
        0x80..=0xff => {
            out.push(0xcc);
            out.push(n as u8);
        }
        0x100..=0xffff => {
            out.push(0xcd);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        0x1_0000..=0xffff_ffff => {
            out.push(0xce);
            out.extend_from_slice(&(n as u32).to_be_bytes());
        }
        _ => {
            out.push(0xcf);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

fn write_int(n: i64, out: &mut Vec<u8>) {
    // A non-negative value has a shorter unsigned encoding, and canonical means shortest.
    // Delegating also stops the same number having two spellings, which would otherwise
    // make `Int(5)` encode to bytes that decode back as `Uint(5)`.
    if n >= 0 {
        write_uint(n as u64, out);
        return;
    }
    match n {
        -32..=-1 => out.push(n as i8 as u8),
        -128..=-33 => {
            out.push(0xd0);
            out.push(n as i8 as u8);
        }
        -32768..=-129 => {
            out.push(0xd1);
            out.extend_from_slice(&(n as i16).to_be_bytes());
        }
        -2_147_483_648..=-32769 => {
            out.push(0xd2);
            out.extend_from_slice(&(n as i32).to_be_bytes());
        }
        _ => {
            out.push(0xd3);
            out.extend_from_slice(&n.to_be_bytes());
        }
    }
}

/// String and binary share a header shape: an optional fixed form, then 8/16/32-bit lengths.
fn write_len(
    len: usize,
    fixed: u8,
    u8_tag: u8,
    u16_tag: u8,
    u32_tag: u8,
    has_fixed: bool,
    out: &mut Vec<u8>,
) {
    if has_fixed && len < 32 {
        out.push(fixed | len as u8);
    } else if len < 0x100 {
        out.push(u8_tag);
        out.push(len as u8);
    } else if len < 0x1_0000 {
        out.push(u16_tag);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(u32_tag);
        out.extend_from_slice(&(len as u32).to_be_bytes());
    }
}

/// Arrays and maps count elements rather than bytes, and their fixed form holds 16.
fn write_count(count: usize, fixed: u8, u16_tag: u8, u32_tag: u8, out: &mut Vec<u8>) {
    if count < 16 {
        out.push(fixed | count as u8);
    } else if count < 0x1_0000 {
        out.push(u16_tag);
        out.extend_from_slice(&(count as u16).to_be_bytes());
    } else {
        out.push(u32_tag);
        out.extend_from_slice(&(count as u32).to_be_bytes());
    }
}

/// Decode exactly one value, which must consume every byte.
///
/// Trailing bytes are an error rather than a shrug: an envelope with something appended is
/// not the envelope that was signed, and silently ignoring the tail is how a parser becomes
/// a place to hide a second message.
pub fn decode(bytes: &[u8]) -> Result<Value, Error> {
    let mut at = 0;
    let value = read(bytes, &mut at, 0)?;
    if at != bytes.len() {
        return Err(Error::Trailing);
    }
    Ok(value)
}

fn read(bytes: &[u8], at: &mut usize, depth: u32) -> Result<Value, Error> {
    if depth > MAX_DEPTH {
        return Err(Error::TooDeep);
    }
    let tag = *bytes.get(*at).ok_or(Error::Truncated)?;
    *at += 1;
    match tag {
        0x00..=0x7f => Ok(Value::Uint(u64::from(tag))),
        0xe0..=0xff => Ok(Value::Int(i64::from(tag as i8))),
        0x80..=0x8f => read_map(bytes, at, usize::from(tag & 0x0f), depth),
        0x90..=0x9f => read_array(bytes, at, usize::from(tag & 0x0f), depth),
        0xa0..=0xbf => read_str(bytes, at, usize::from(tag & 0x1f)),
        0xc0 => Ok(Value::Nil),
        0xc2 => Ok(Value::Bool(false)),
        0xc3 => Ok(Value::Bool(true)),
        0xc4 => {
            let len = read_len(bytes, at, 1)?;
            read_bin(bytes, at, len)
        }
        0xc5 => {
            let len = read_len(bytes, at, 2)?;
            read_bin(bytes, at, len)
        }
        0xc6 => {
            let len = read_len(bytes, at, 4)?;
            read_bin(bytes, at, len)
        }
        0xcc => Ok(Value::Uint(read_len(bytes, at, 1)? as u64)),
        0xcd => Ok(Value::Uint(read_len(bytes, at, 2)? as u64)),
        0xce => Ok(Value::Uint(read_len(bytes, at, 4)? as u64)),
        0xcf => {
            let raw = take(bytes, at, 8)?;
            let mut wide = [0u8; 8];
            wide.copy_from_slice(raw);
            Ok(Value::Uint(u64::from_be_bytes(wide)))
        }
        0xd0 => Ok(Value::Int(i64::from(take(bytes, at, 1)?[0] as i8))),
        0xd1 => {
            let raw = take(bytes, at, 2)?;
            Ok(Value::Int(i64::from(i16::from_be_bytes([raw[0], raw[1]]))))
        }
        0xd2 => {
            let raw = take(bytes, at, 4)?;
            let mut wide = [0u8; 4];
            wide.copy_from_slice(raw);
            Ok(Value::Int(i64::from(i32::from_be_bytes(wide))))
        }
        0xd3 => {
            let raw = take(bytes, at, 8)?;
            let mut wide = [0u8; 8];
            wide.copy_from_slice(raw);
            Ok(Value::Int(i64::from_be_bytes(wide)))
        }
        0xd9 => {
            let len = read_len(bytes, at, 1)?;
            read_str(bytes, at, len)
        }
        0xda => {
            let len = read_len(bytes, at, 2)?;
            read_str(bytes, at, len)
        }
        0xdb => {
            let len = read_len(bytes, at, 4)?;
            read_str(bytes, at, len)
        }
        0xdc => {
            let count = read_len(bytes, at, 2)?;
            read_array(bytes, at, count, depth)
        }
        0xdd => {
            let count = read_len(bytes, at, 4)?;
            read_array(bytes, at, count, depth)
        }
        0xde => {
            let count = read_len(bytes, at, 2)?;
            read_map(bytes, at, count, depth)
        }
        0xdf => {
            let count = read_len(bytes, at, 4)?;
            read_map(bytes, at, count, depth)
        }
        _ => Err(Error::Unsupported),
    }
}

/// Read a big-endian length of `width` bytes.
///
/// The `usize` cast is safe on every target this crate builds for because `width` is at most
/// 4; a 32-bit length that exceeds a 16-bit `usize` would be caught by [`take`] regardless,
/// since the buffer cannot be that long.
fn read_len(bytes: &[u8], at: &mut usize, width: usize) -> Result<usize, Error> {
    let raw = take(bytes, at, width)?;
    let mut len = 0usize;
    for byte in raw {
        len = len.checked_mul(256).ok_or(Error::Truncated)?;
        len = len
            .checked_add(usize::from(*byte))
            .ok_or(Error::Truncated)?;
    }
    Ok(len)
}

fn take<'a>(bytes: &'a [u8], at: &mut usize, len: usize) -> Result<&'a [u8], Error> {
    let end = at.checked_add(len).ok_or(Error::Truncated)?;
    let slice = bytes.get(*at..end).ok_or(Error::Truncated)?;
    *at = end;
    Ok(slice)
}

fn read_str(bytes: &[u8], at: &mut usize, len: usize) -> Result<Value, Error> {
    let raw = take(bytes, at, len)?;
    let text = core::str::from_utf8(raw).map_err(|_| Error::BadUtf8)?;
    Ok(Value::Str(String::from(text)))
}

fn read_bin(bytes: &[u8], at: &mut usize, len: usize) -> Result<Value, Error> {
    Ok(Value::Bin(take(bytes, at, len)?.to_vec()))
}

/// A container header claims a count. That claim is checked against the bytes that remain
/// before any capacity is reserved: the cheapest possible element is one byte, so a count
/// larger than the remaining buffer cannot be honest.
fn read_array(bytes: &[u8], at: &mut usize, count: usize, depth: u32) -> Result<Value, Error> {
    if count > bytes.len().saturating_sub(*at) {
        return Err(Error::Truncated);
    }
    let mut items = Vec::with_capacity(count);
    for _ in 0..count {
        items.push(read(bytes, at, depth + 1)?);
    }
    Ok(Value::Array(items))
}

fn read_map(bytes: &[u8], at: &mut usize, count: usize, depth: u32) -> Result<Value, Error> {
    // Two values per pair, so the remaining-bytes floor is twice the count.
    let pairs = count.checked_mul(2).ok_or(Error::Truncated)?;
    if pairs > bytes.len().saturating_sub(*at) {
        return Err(Error::Truncated);
    }
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let key = read(bytes, at, depth + 1)?;
        let value = read(bytes, at, depth + 1)?;
        entries.push((key, value));
    }
    Ok(Value::Map(entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn round_trip(value: Value) {
        let bytes = encode(&value);
        assert_eq!(decode(&bytes).unwrap(), value);
    }

    #[test]
    fn encodes_the_shortest_header_for_each_width() {
        assert_eq!(encode(&Value::Uint(0x7f)), vec![0x7f]);
        assert_eq!(encode(&Value::Uint(0x80)), vec![0xcc, 0x80]);
        assert_eq!(encode(&Value::Uint(0x100)), vec![0xcd, 0x01, 0x00]);
        assert_eq!(
            encode(&Value::Uint(0x1_0000)),
            vec![0xce, 0x00, 0x01, 0x00, 0x00]
        );
        assert_eq!(encode(&Value::Int(-1)), vec![0xff]);
        assert_eq!(encode(&Value::Int(-32)), vec![0xe0]);
        assert_eq!(encode(&Value::Int(-33)), vec![0xd0, 0xdf]);
        assert_eq!(encode(&Value::Int(-129)), vec![0xd1, 0xff, 0x7f]);
    }

    #[test]
    fn a_non_negative_int_takes_the_shorter_unsigned_encoding() {
        // Canonical means shortest, so there is exactly one encoding of 5 and it decodes as
        // an unsigned value. This is asymmetric on purpose: the alternative is two spellings
        // of the same number, and a signature over "the same number" that does not match.
        for n in [0_i64, 5, 127, 128, 300] {
            assert_eq!(encode(&Value::Int(n)), encode(&Value::Uint(n as u64)));
            assert_eq!(
                decode(&encode(&Value::Int(n))).unwrap(),
                Value::Uint(n as u64)
            );
        }
        assert_eq!(
            encode(&Value::Str("one".into())),
            vec![0xa3, b'o', b'n', b'e']
        );
        assert_eq!(encode(&Value::Bin(vec![1, 2])), vec![0xc4, 0x02, 1, 2]);
        assert_eq!(encode(&Value::Array(vec![Value::Nil])), vec![0x91, 0xc0]);
    }

    #[test]
    fn round_trips_the_shapes_the_envelope_uses() {
        round_trip(Value::Map(vec![
            (Value::Str("hashtype".into()), Value::Str("sha256".into())),
            (Value::Str("hash".into()), Value::Bin(vec![0xaa; 32])),
            (
                Value::Str("meta".into()),
                Value::Map(vec![
                    (Value::Str("version".into()), Value::Uint(3)),
                    (
                        Value::Str("tags".into()),
                        Value::Array(vec![Value::Str("one".into()), Value::Str("two".into())]),
                    ),
                    (Value::Str("stable".into()), Value::Bool(true)),
                ]),
            ),
        ]));
    }

    #[test]
    fn round_trips_lengths_that_cross_every_header_boundary() {
        for len in [0, 31, 32, 255, 256, 300] {
            round_trip(Value::Str("x".repeat(len)));
            round_trip(Value::Bin(vec![0x5a; len]));
        }
        for count in [0, 15, 16, 20] {
            round_trip(Value::Array(vec![Value::Uint(1); count]));
            round_trip(Value::Map(vec![(Value::Uint(1), Value::Nil); count]));
        }
    }

    #[test]
    fn a_container_cannot_claim_more_elements_than_bytes_remain() {
        // map32 claiming four billion pairs, with nothing behind it. The bound has to be
        // checked before the allocation, or this is an out-of-memory abort on a board.
        assert_eq!(
            decode(&[0xdf, 0xff, 0xff, 0xff, 0xff]),
            Err(Error::Truncated)
        );
        assert_eq!(
            decode(&[0xdd, 0xff, 0xff, 0xff, 0xff]),
            Err(Error::Truncated)
        );
        // Same for a declared byte length that outruns the buffer.
        assert_eq!(
            decode(&[0xc6, 0xff, 0xff, 0xff, 0xff]),
            Err(Error::Truncated)
        );
    }

    #[test]
    fn nesting_is_bounded() {
        // MAX_DEPTH + 2 nested one-element arrays: deep enough to trip the guard rather
        // than the recursion.
        let deep: Vec<u8> = core::iter::repeat_n(0x91, MAX_DEPTH as usize + 2)
            .chain(core::iter::once(0xc0))
            .collect();
        assert_eq!(decode(&deep), Err(Error::TooDeep));
    }

    #[test]
    fn trailing_bytes_are_refused() {
        assert_eq!(decode(&[0xc0, 0xc0]), Err(Error::Trailing));
    }

    #[test]
    fn unsupported_types_are_refused_rather_than_guessed() {
        // float64 and a fixext are both well-formed MessagePack this codec declines to read.
        assert_eq!(
            decode(&[0xcb, 0, 0, 0, 0, 0, 0, 0, 0]),
            Err(Error::Unsupported)
        );
        assert_eq!(decode(&[0xd4, 0x00, 0x00]), Err(Error::Unsupported));
    }

    #[test]
    fn a_string_that_is_not_utf8_is_refused() {
        assert_eq!(decode(&[0xa1, 0xff]), Err(Error::BadUtf8));
    }

    #[test]
    fn get_finds_string_keys_and_ignores_others() {
        let map = Value::Map(vec![
            (Value::Uint(1), Value::Str("not a key".into())),
            (Value::Str("name".into()), Value::Str("retinue".into())),
        ]);
        assert_eq!(map.get("name").and_then(Value::as_str), Some("retinue"));
        assert!(map.get("missing").is_none());
    }
}
