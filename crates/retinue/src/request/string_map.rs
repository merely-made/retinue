//! Bounded string-map requests, independently observed from NomadNet 1.4.2.

use super::{Reader, write_bin, write_f64};
use crate::{Error, Result, hash::AddressHash};
use alloc::{collections::BTreeMap, string::String, vec::Vec};

/// Limits on the complete envelope, checked before allocating fields.
/// These do not enable segmented outgoing requests or replace the link packet limit.
#[derive(Clone, Copy, Debug)]
pub struct StringMapLimits {
    pub max_entries: usize,
    pub max_encoded_bytes: usize,
}

impl Default for StringMapLimits {
    fn default() -> Self {
        Self {
            max_entries: 64,
            max_encoded_bytes: 16 * 1024,
        }
    }
}

/// A request whose third array element is a MessagePack map of UTF-8 strings.
/// Keys are transport data: callers add `field_`/`var_` prefixes and decide
/// whether an endpoint may handle submissions. Decoding grants no authority.
#[derive(Clone, Debug, PartialEq)]
pub struct StringMapRequest {
    pub path_hash: AddressHash,
    pub data: BTreeMap<String, String>,
    pub time: f64,
}

impl StringMapRequest {
    pub fn new(path: &[u8], data: BTreeMap<String, String>, time: f64) -> Self {
        Self {
            path_hash: AddressHash::of(path),
            data,
            time,
        }
    }

    /// Pack a native map, never a binary blob containing a serialized map.
    pub fn pack(&self, limits: StringMapLimits) -> Result<Vec<u8>> {
        if !self.time.is_finite() || self.data.len() > limits.max_entries {
            return Err(Error::BadRequest);
        }
        let count = u32::try_from(self.data.len()).map_err(|_| Error::BadRequest)?;
        let map_header = match count {
            0..=15 => 1,
            16..=65535 => 3,
            _ => 5,
        };
        let mut size = 1usize + 9 + 18 + map_header;
        for value in self.data.iter().flat_map(|(key, value)| [key, value]) {
            let len = u32::try_from(value.len()).map_err(|_| Error::BadRequest)?;
            let header = match len {
                0..=31 => 1,
                32..=255 => 2,
                256..=65535 => 3,
                _ => 5,
            };
            size = size
                .checked_add(header)
                .and_then(|n| n.checked_add(value.len()))
                .ok_or(Error::BadRequest)?;
        }
        if size > limits.max_encoded_bytes {
            return Err(Error::BadRequest);
        }
        let mut out = Vec::with_capacity(size);
        out.push(0x93);
        write_f64(&mut out, self.time);
        write_bin(&mut out, self.path_hash.as_slice());
        match count {
            0..=15 => out.push(0x80 | count as u8),
            16..=65535 => {
                out.push(0xde);
                out.extend_from_slice(&(count as u16).to_be_bytes());
            }
            _ => {
                out.push(0xdf);
                out.extend_from_slice(&count.to_be_bytes());
            }
        }
        for (key, value) in &self.data {
            write_text(&mut out, key);
            write_text(&mut out, value);
        }
        Ok(out)
    }

    /// Reject duplicate keys, non-string fields, invalid UTF-8, trailing data and
    /// oversized envelopes. Check the map count before allocating any entries.
    pub fn unpack(bytes: &[u8], limits: StringMapLimits) -> Result<Self> {
        if bytes.len() > limits.max_encoded_bytes {
            return Err(Error::BadRequest);
        }
        let mut r = Reader::new(bytes);
        if r.array_header()? != 3 {
            return Err(Error::BadRequest);
        }
        let time = r.f64()?;
        if !time.is_finite() {
            return Err(Error::BadRequest);
        }
        let path_hash = AddressHash::from_slice(r.bin()?).ok_or(Error::BadRequest)?;
        let count = match r.byte()? {
            tag @ 0x80..=0x8f => (tag & 0x0f) as usize,
            0xde => u16::from_be_bytes(r.take(2)?.try_into().unwrap()) as usize,
            0xdf => usize::try_from(u32::from_be_bytes(r.take(4)?.try_into().unwrap()))
                .map_err(|_| Error::BadRequest)?,
            _ => return Err(Error::BadRequest),
        };
        if count > limits.max_entries {
            return Err(Error::BadRequest);
        }
        let mut data = BTreeMap::new();
        for _ in 0..count {
            let key = read_text(&mut r)?;
            if data.contains_key(key) {
                return Err(Error::BadRequest);
            }
            let value = read_text(&mut r)?;
            data.insert(String::from(key), String::from(value));
        }
        if r.i != bytes.len() {
            return Err(Error::BadRequest);
        }
        Ok(Self {
            path_hash,
            data,
            time,
        })
    }
}

fn write_text(out: &mut Vec<u8>, value: &str) {
    let len = value.len(); // pack checked u32 length and total budget
    match len {
        0..=31 => out.push(0xa0 | len as u8),
        32..=255 => {
            out.push(0xd9);
            out.push(len as u8);
        }
        256..=65535 => {
            out.push(0xda);
            out.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            out.push(0xdb);
            out.extend_from_slice(&(len as u32).to_be_bytes());
        }
    }
    out.extend_from_slice(value.as_bytes());
}

fn read_text<'a>(r: &mut Reader<'a>) -> Result<&'a str> {
    let len = match r.byte()? {
        tag @ 0xa0..=0xbf => (tag & 0x1f) as usize,
        0xd9 => r.byte()? as usize,
        0xda => u16::from_be_bytes(r.take(2)?.try_into().unwrap()) as usize,
        0xdb => usize::try_from(u32::from_be_bytes(r.take(4)?.try_into().unwrap()))
            .map_err(|_| Error::BadRequest)?,
        _ => return Err(Error::BadRequest),
    };
    core::str::from_utf8(r.take(len)?).map_err(|_| Error::BadRequest)
}
