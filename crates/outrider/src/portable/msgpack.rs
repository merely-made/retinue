//! MessagePack, only as much as the LXMF payload shape needs.

use alloc::vec::Vec;

use super::CodecError;

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

pub(super) fn at_map(bytes: &[u8], at: usize) -> bool {
    match bytes.get(at) {
        Some(marker) => matches!(marker, 0x80..=0x8f | 0xde | 0xdf),
        None => false,
    }
}

pub(super) fn read_array_len(bytes: &[u8], at: &mut usize) -> Result<usize, CodecError> {
    let marker = byte(bytes, *at)?;
    *at += 1;
    match marker {
        0x90..=0x9f => Ok((marker & 0x0f) as usize),
        0xdc => Ok(be(take(bytes, at, 2)?)),
        0xdd => Ok(be(take(bytes, at, 4)?)),
        _ => Err(CodecError::InvalidPayloadShape),
    }
}

pub(super) fn read_f64(bytes: &[u8], at: &mut usize) -> Result<f64, CodecError> {
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

pub(super) fn read_bin<'a>(bytes: &'a [u8], at: &mut usize) -> Result<&'a [u8], CodecError> {
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
/// A full skipper rather than a map-only one: a map's values may be anything, and a skipper
/// that missed a shape would silently mis-slice the rest of the payload.
pub(super) fn skip(bytes: &[u8], at: &mut usize) -> Result<(), CodecError> {
    skip_nested(bytes, at, 0)
}

/// How deep a field map may nest before this refuses to follow it.
///
/// Each level costs a stack frame, and a run of `0x91` buys one level per byte of untrusted
/// input: a reset on a board, an abort on a host. LXMF field maps are one level deep.
pub(super) const MAX_NESTING: u32 = 16;

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

pub(super) fn write_f64(out: &mut Vec<u8>, value: f64) {
    out.push(0xcb);
    out.extend_from_slice(&value.to_be_bytes());
}

/// Write a binary in the shortest encoding that holds it, which is what stock MessagePack
/// writers do and therefore what byte-exactness requires.
pub(super) fn write_bin(out: &mut Vec<u8>, bytes: &[u8]) {
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
