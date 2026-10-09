//! Part requests and hashmap updates.

use alloc::vec::Vec;

use super::MAPHASH_LEN;
use super::map_codec::{MapReader, MapWriter};
use crate::{Error, Result};

/// A parsed part request (context `RESOURCE_REQ`).
///
/// Normal: `0x00 || resource_hash(32) || wanted(4*N)`. Exhausted (soliciting more hashmap):
/// `0xff || last_map_hash(4) || resource_hash(32) || wanted(4*N)`. The leading map hash on the
/// exhausted form tells the sender where the [`Hmu`] resumes. Verified against RNS 1.3.8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    /// The receiver's hashmap is exhausted and it wants more via an [`Hmu`].
    pub exhausted: bool,
    /// On an exhausted request, the last map hash the receiver already holds.
    pub last_map_hash: Option<[u8; MAPHASH_LEN]>,
    pub resource_hash: [u8; 32],
    /// The map hashes of the parts being requested (may be empty on a pure HMU solicit).
    pub wanted: Vec<[u8; MAPHASH_LEN]>,
}

/// Build a normal part request: `0x00 || resource_hash || wanted*`.
pub fn build_request(resource_hash: &[u8; 32], wanted: &[[u8; MAPHASH_LEN]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 32 + wanted.len() * MAPHASH_LEN);
    out.push(0x00);
    out.extend_from_slice(resource_hash);
    for w in wanted {
        out.extend_from_slice(w);
    }
    out
}

/// Build an exhausted request soliciting more hashmap:
/// `0xff || last_map_hash || resource_hash || wanted*`.
pub fn build_exhausted_request(
    last_map_hash: &[u8; MAPHASH_LEN],
    resource_hash: &[u8; 32],
    wanted: &[[u8; MAPHASH_LEN]],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + MAPHASH_LEN + 32 + wanted.len() * MAPHASH_LEN);
    out.push(0xff);
    out.extend_from_slice(last_map_hash);
    out.extend_from_slice(resource_hash);
    for w in wanted {
        out.extend_from_slice(w);
    }
    out
}

/// Parse a part request. The sender uses this to learn which parts to send and whether to
/// emit an [`Hmu`]. The first byte is `0x00` or `0xff` (RNS `HASHMAP_IS_NOT_EXHAUSTED`,
/// `HASHMAP_IS_EXHAUSTED`); anything else is malformed.
pub fn parse_request(payload: &[u8]) -> Result<Request> {
    let exhausted = match payload.first() {
        Some(0x00) => false,
        Some(0xff) => true,
        _ => return Err(Error::BadRequest),
    };
    let mut off = 1;
    let last_map_hash = if exhausted {
        let m: [u8; MAPHASH_LEN] = payload
            .get(off..off + MAPHASH_LEN)
            .ok_or(Error::BadRequest)?
            .try_into()
            .expect("checked");
        off += MAPHASH_LEN;
        Some(m)
    } else {
        None
    };
    let resource_hash: [u8; 32] = payload
        .get(off..off + 32)
        .ok_or(Error::BadRequest)?
        .try_into()
        .expect("checked");
    off += 32;
    let wanted = payload[off..].as_chunks::<MAPHASH_LEN>().0.to_vec();
    Ok(Request {
        exhausted,
        last_map_hash,
        resource_hash,
        wanted,
    })
}

/// A parsed hashmap update (context `RESOURCE_HMU`): the next batch of part map hashes.
///
/// `resource_hash(32) || msgpack([segment, hashmap_bin(4*M)])`. Verified against RNS 1.3.8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hmu {
    pub resource_hash: [u8; 32],
    pub segment: i64,
    pub hashes: Vec<[u8; MAPHASH_LEN]>,
}

/// Build an HMU payload.
pub fn build_hmu(resource_hash: &[u8; 32], segment: i64, hashes: &[[u8; MAPHASH_LEN]]) -> Vec<u8> {
    let mut w = MapWriter::array(2);
    w.int(segment);
    w.bin(&hashes.concat());
    let body = w.finish();
    let mut out = Vec::with_capacity(32 + body.len());
    out.extend_from_slice(resource_hash);
    out.extend_from_slice(&body);
    out
}

/// Parse an HMU payload.
pub fn parse_hmu(payload: &[u8]) -> Result<Hmu> {
    let resource_hash: [u8; 32] = payload
        .get(..32)
        .ok_or(Error::BadRequest)?
        .try_into()
        .expect("32");
    let mut r = MapReader::new(&payload[32..]);
    if r.byte()? != 0x92 {
        return Err(Error::BadRequest);
    }
    let segment = r.int()?;
    let bin = r.bin()?;
    if bin.len() % MAPHASH_LEN != 0 {
        return Err(Error::BadRequest);
    }
    let hashes = bin.as_chunks::<MAPHASH_LEN>().0.to_vec();
    Ok(Hmu {
        resource_hash,
        segment,
        hashes,
    })
}
