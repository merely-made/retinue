//! Framing of resource content: metadata prefix, random-hash prefix, and part splitting.

use alloc::vec::Vec;

use super::{METADATA_MAX_SIZE, RANDOM_HASH_LEN, SDU, map_hash};
use crate::{Error, Result};

/// Bytes of the big-endian length that prefixes a resource's metadata.
const METADATA_LEN_BYTES: usize = 3;

/// Frame already-packed (msgpack) metadata for the front of a resource's data:
/// `len(3, big-endian) || metadata`, as RNS does. Returns [`Error::CapacityExceeded`] past
/// [`METADATA_MAX_SIZE`].
pub fn pack_metadata(metadata: &[u8]) -> Result<Vec<u8>> {
    if metadata.len() > METADATA_MAX_SIZE {
        return Err(Error::CapacityExceeded);
    }
    let mut out = Vec::with_capacity(METADATA_LEN_BYTES + metadata.len());
    out.extend_from_slice(&(metadata.len() as u32).to_be_bytes()[1..]);
    out.extend_from_slice(metadata);
    Ok(out)
}

/// Split the metadata framed by [`pack_metadata`] off the front of a recovered resource's
/// data, in place: `data` keeps the payload and the packed metadata is returned.
pub fn split_metadata(data: &mut Vec<u8>) -> Result<Vec<u8>> {
    let len = data
        .get(..METADATA_LEN_BYTES)
        .ok_or(Error::Truncated)?
        .iter()
        .fold(0_usize, |len, &b| (len << 8) | usize::from(b));
    let end = METADATA_LEN_BYTES + len;
    let metadata = data
        .get(METADATA_LEN_BYTES..end)
        .ok_or(Error::Truncated)?
        .to_vec();
    data.drain(..end);
    Ok(metadata)
}

/// The content that is compressed, sealed, and split for transfer: `random_hash || data`.
///
/// RNS prepends the random hash to the payload before compression and encryption, so the
/// transferred blob decrypts to this, not to the bare payload. The resource hash is still
/// computed over `data || random_hash` (a different order); both were verified against RNS
/// 1.3.8.
pub fn content(data: &[u8], random_hash: &[u8]) -> Vec<u8> {
    let mut c = Vec::with_capacity(random_hash.len() + data.len());
    c.extend_from_slice(random_hash);
    c.extend_from_slice(data);
    c
}

/// Recover the payload from transferred content by stripping the `random_hash` prefix.
pub fn data_from_content(content: &[u8]) -> Result<&[u8]> {
    content.get(RANDOM_HASH_LEN..).ok_or(Error::Truncated)
}

/// Split a sealed transfer token into parts of at most [`SDU`] bytes, and compute the
/// hashmap over them.
pub fn split_parts(token: &[u8], random_hash: &[u8]) -> (Vec<Vec<u8>>, Vec<u8>) {
    split_parts_with_size(token, random_hash, SDU)
}

/// Split a sealed transfer token with an explicit part ceiling. The default
/// [`split_parts`] remains wire-compatible with RNS's 464-byte SDU; negotiated
/// smaller links can use shorter parts without changing resource semantics.
pub fn split_parts_with_size(
    token: &[u8],
    random_hash: &[u8],
    part_size: usize,
) -> (Vec<Vec<u8>>, Vec<u8>) {
    let mut parts = Vec::new();
    let mut hashmap = Vec::new();
    for chunk in token.chunks(part_size.clamp(1, SDU)) {
        hashmap.extend_from_slice(&map_hash(chunk, random_hash));
        parts.push(chunk.to_vec());
    }
    (parts, hashmap)
}
