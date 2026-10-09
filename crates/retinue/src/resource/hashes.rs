//! Resource, map, and proof hashes.

use alloc::vec::Vec;

use super::MAPHASH_LEN;
use crate::hash::full_hash;

/// The resource hash: `SHA256(uncompressed_data || random_hash)`. It binds the resource to
/// its content and this transfer's random hash. Verified against RNS 1.3.8.
pub fn resource_hash(data: &[u8], random_hash: &[u8]) -> [u8; 32] {
    let mut m = Vec::with_capacity(data.len() + random_hash.len());
    m.extend_from_slice(data);
    m.extend_from_slice(random_hash);
    full_hash(&m)
}

/// A part's 4-byte map hash: `SHA256(part || random_hash)[..4]`. Verified against RNS 1.3.8.
pub fn map_hash(part: &[u8], random_hash: &[u8]) -> [u8; MAPHASH_LEN] {
    let mut m = Vec::with_capacity(part.len() + random_hash.len());
    m.extend_from_slice(part);
    m.extend_from_slice(random_hash);
    let h = full_hash(&m);
    let mut out = [0u8; MAPHASH_LEN];
    out.copy_from_slice(&h[..MAPHASH_LEN]);
    out
}

/// Parse a resource proof packet payload: `resource_hash(32) || proof(32)`, sent
/// unencrypted. Returns `(resource_hash, proof)`, or `None` if it is not 64 bytes.
///
/// A sender compares the returned proof against the value it precomputed with [`proof`]; a
/// match means the receiver reassembled the resource intact. Verified against RNS 1.3.8.
pub fn parse_proof(payload: &[u8]) -> Option<([u8; 32], [u8; 32])> {
    if payload.len() != 64 {
        return None;
    }
    let mut h = [0u8; 32];
    let mut p = [0u8; 32];
    h.copy_from_slice(&payload[..32]);
    p.copy_from_slice(&payload[32..]);
    Some((h, p))
}

/// The proof a receiver returns: `SHA256(uncompressed_data || resource_hash)`. The sender
/// checks it against the value it precomputed. Verified against RNS 1.3.8.
pub fn proof(data: &[u8], resource_hash: &[u8; 32]) -> [u8; 32] {
    let mut m = Vec::with_capacity(data.len() + 32);
    m.extend_from_slice(data);
    m.extend_from_slice(resource_hash);
    full_hash(&m)
}
