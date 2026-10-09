//! The sending half of one resource segment.

use alloc::vec::Vec;

use super::{
    Advertisement, FLAG_COMPRESSED, FLAG_ENCRYPTED, FLAG_METADATA, FLAG_RESPONSE, FLAG_SPLIT,
    HASHMAP_MAX_PARTS, MAPHASH_LEN, RANDOM_HASH_LEN, Request, SDU, build_hmu, map_hash, proof,
    resource_hash,
};
use crate::hash::full_hash;

/// How many times a sender re-draws its random hash to clear a map-hash collision before it
/// sends the map as it is. RNS needs uniqueness only within `COLLISION_GUARD_SIZE` (224
/// parts); this keeps it across the whole segment. The bound only keeps the loop finite.
const MAX_REROLLS: usize = 8;

/// Sender state for one outgoing resource segment.
///
/// Splits a sealed token into parts, advertises the first [`HASHMAP_MAX_PARTS`] map hashes,
/// serves part requests, and emits an [`Hmu`](super::Hmu) when the receiver's hashmap runs out. Pair the
/// advertisement and each served part / HMU with the link's framing in the shell.
pub struct Outgoing {
    hash: [u8; 32],
    /// The whole-resource identity, carried in every segment's advertisement `o` field. For
    /// a single-segment resource this equals `hash`; across a multi-segment resource it is
    /// the FIRST segment's hash, shared, so the receiver groups the segments.
    original_hash: [u8; 32],
    random_hash: [u8; RANDOM_HASH_LEN],
    compressed: bool,
    has_metadata: bool,
    /// 1-based segment index and total segment count. Single-segment resources are (1, 1).
    segment_index: i64,
    total_segments: i64,
    /// The full resource's data size, carried in every segment's advertisement `d` field.
    total_data_size: u64,
    request_id: Option<[u8; 16]>,
    /// The sealed token, held once; part `i` is its `i`th `part_size` slice.
    token: Vec<u8>,
    part_size: usize,
    /// All part map hashes, in transfer order.
    map_hashes: Vec<[u8; MAPHASH_LEN]>,
    /// `(map hash, part index)` for every part, sorted, to find a requested part.
    by_hash: Vec<([u8; MAPHASH_LEN], usize)>,
    expected_proof: [u8; 32],
}

impl Outgoing {
    /// Prepare to send `data`, already sealed into `token` (see [`content`](super::content) and the link's
    /// `seal`). `compressed` records whether `token`'s plaintext was bz2-compressed.
    pub fn new(
        data: &[u8],
        token: &[u8],
        random_hash: [u8; RANDOM_HASH_LEN],
        compressed: bool,
    ) -> Self {
        Self::new_with_part_size(data, token, random_hash, compressed, SDU)
    }

    /// Prepare a sender whose resource parts fit a negotiated link MTU.
    pub fn new_with_part_size(
        data: &[u8],
        token: &[u8],
        random_hash: [u8; RANDOM_HASH_LEN],
        compressed: bool,
        part_size: usize,
    ) -> Self {
        Self::from_token(data, token.to_vec(), random_hash, compressed, part_size)
    }

    /// [`new_with_part_size`](Self::new_with_part_size), taking ownership of the token
    /// rather than copying it.
    ///
    /// If two different parts share a map hash, the random hash is re-drawn (derived from
    /// the previous one) until every map hash is unique, as RNS re-draws on a collision.
    /// The resource hash, map hashes and expected proof all follow the final random hash;
    /// the token, whose plaintext carries the caller's, is unchanged. The receiver strips
    /// that prefix without reading it, as it does for RNS, whose prefix is an independent
    /// random value.
    pub fn from_token(
        data: &[u8],
        token: Vec<u8>,
        mut random_hash: [u8; RANDOM_HASH_LEN],
        compressed: bool,
        part_size: usize,
    ) -> Self {
        let part_size = part_size.clamp(1, SDU);
        let mut rerolls = 0;
        let (map_hashes, by_hash) = loop {
            let map_hashes: Vec<_> = token
                .chunks(part_size)
                .map(|part| map_hash(part, &random_hash))
                .collect();
            let mut by_hash: Vec<_> = map_hashes.iter().copied().zip(0..).collect();
            by_hash.sort_unstable();
            // Byte-identical parts share a hash harmlessly: either copy serves either slot.
            let part = |i: usize| &token[i * part_size..((i + 1) * part_size).min(token.len())];
            let collides = by_hash
                .windows(2)
                .any(|pair| pair[0].0 == pair[1].0 && part(pair[0].1) != part(pair[1].1));
            if !collides || rerolls == MAX_REROLLS {
                break (map_hashes, by_hash);
            }
            rerolls += 1;
            let redrawn = full_hash(&random_hash);
            random_hash.copy_from_slice(&redrawn[..RANDOM_HASH_LEN]);
        };
        let hash = resource_hash(data, &random_hash);
        Self {
            hash,
            original_hash: hash,
            random_hash,
            compressed,
            has_metadata: false,
            segment_index: 1,
            total_segments: 1,
            total_data_size: data.len() as u64,
            request_id: None,
            expected_proof: proof(data, &hash),
            token,
            part_size,
            map_hashes,
            by_hash,
        }
    }

    /// Mark this as segment `index` of `total` in a larger resource of `total_data_size`
    /// bytes, identified by `original_hash` (carried in `i`/`l`/`d`/`o`). Verified against
    /// RNS 1.3.8.
    ///
    /// Segment 1 is identified by its own [`resource_hash`](Self::resource_hash) whatever
    /// `original_hash` says, since a re-drawn random hash changes it; take the later
    /// segments' `original_hash` from the first segment's `resource_hash()`.
    pub fn with_segment(
        mut self,
        index: i64,
        total: i64,
        total_data_size: u64,
        original_hash: [u8; 32],
    ) -> Self {
        self.segment_index = index;
        self.total_segments = total;
        self.total_data_size = total_data_size;
        self.original_hash = if index == 1 { self.hash } else { original_hash };
        self
    }

    /// Mark this Resource as the response to one request.
    pub fn with_request_id(mut self, request_id: [u8; 16]) -> Self {
        self.request_id = Some(request_id);
        self
    }

    /// Mark the data as starting with metadata framed by [`pack_metadata`](super::pack_metadata), which sets
    /// [`FLAG_METADATA`] on the advertisement.
    pub fn with_metadata(mut self) -> Self {
        self.has_metadata = true;
        self
    }

    /// The advertisement, carrying the first [`HASHMAP_MAX_PARTS`] map hashes.
    pub fn advertisement(&self) -> Advertisement {
        self.advertisement_with_hash_limit(HASHMAP_MAX_PARTS)
    }

    /// Build an advertisement with a caller-selected hashmap window.
    pub fn advertisement_with_hash_limit(&self, hash_limit: usize) -> Advertisement {
        let n = self
            .map_hashes
            .len()
            .min(hash_limit.clamp(1, HASHMAP_MAX_PARTS));
        let hashmap: Vec<u8> = self.map_hashes[..n]
            .iter()
            .flat_map(|h| h.iter().copied())
            .collect();
        let mut flags = FLAG_ENCRYPTED;
        if self.compressed {
            flags |= FLAG_COMPRESSED;
        }
        if self.request_id.is_some() {
            flags |= FLAG_RESPONSE;
        }
        if self.has_metadata {
            flags |= FLAG_METADATA;
        }
        if self.total_segments > 1 {
            flags |= FLAG_SPLIT;
        }
        Advertisement {
            transfer_size: self.token.len() as u64,
            data_size: self.total_data_size,
            parts: self.map_hashes.len() as u64,
            resource_hash: self.hash.to_vec(),
            original_hash: self.original_hash.to_vec(),
            random_hash: self.random_hash.to_vec(),
            flags,
            hashmap,
            i: self.segment_index,
            l: self.total_segments,
            q: self.request_id.map(|id| id.to_vec()),
        }
    }

    /// Part `index`, a slice of the sealed token.
    pub fn part(&self, index: usize) -> Option<&[u8]> {
        let start = index.checked_mul(self.part_size)?;
        self.token
            .get(start..(start + self.part_size).min(self.token.len()))
            .filter(|part| !part.is_empty())
    }

    /// The indices of the parts with map hash `m`, in transfer order.
    fn indices_of(&self, m: [u8; MAPHASH_LEN]) -> impl Iterator<Item = usize> + '_ {
        let first = self.by_hash.partition_point(|(h, _)| *h < m);
        self.by_hash[first..]
            .iter()
            .take_while(move |(h, _)| *h == m)
            .map(|&(_, i)| i)
    }

    /// The indices of every part sharing part `index`'s map hash, `index` among them. Map
    /// hashes are unique but for byte-identical parts, so serving any one of them serves
    /// them all.
    pub fn copies_of(&self, index: usize) -> impl Iterator<Item = usize> + '_ {
        let m = self.map_hashes.get(index).copied();
        m.into_iter().flat_map(|m| self.indices_of(m))
    }

    /// The indices of the parts a request names, in request order. A hash this sender does
    /// not hold names nothing.
    pub fn requested_indices(&self, request: &Request) -> Vec<usize> {
        request
            .wanted
            .iter()
            .filter_map(|m| self.indices_of(*m).next())
            .collect()
    }

    /// The parts to send in response to a request (those whose map hashes we hold).
    pub fn serve(&self, request: &Request) -> Vec<Vec<u8>> {
        self.requested_indices(request)
            .into_iter()
            .filter_map(|i| self.part(i).map(<[u8]>::to_vec))
            .collect()
    }

    /// Build the next hashmap update after `last_map_hash`: the batch of map hashes that
    /// follow it in transfer order. Empty if `last_map_hash` is the final part.
    pub fn hmu_after(&mut self, last_map_hash: &[u8; MAPHASH_LEN]) -> Vec<u8> {
        self.hmu_after_with_hash_limit(last_map_hash, HASHMAP_MAX_PARTS)
    }

    /// Build the next hashmap update with a caller-selected hash window.
    pub fn hmu_after_with_hash_limit(
        &mut self,
        last_map_hash: &[u8; MAPHASH_LEN],
        hash_limit: usize,
    ) -> Vec<u8> {
        // Derive the segment from the requested position, not mutable state, so a repeated
        // solicitation reproduces the same HMU after loss. A receiver solicits after a
        // segment's last hash, so prefer that position among identical parts.
        let hash_limit = hash_limit.clamp(1, HASHMAP_MAX_PARTS);
        let start = self
            .indices_of(*last_map_hash)
            .map(|i| i + 1)
            .reduce(|found, next| if found % hash_limit == 0 { found } else { next })
            .unwrap_or(self.map_hashes.len());
        let end = (start + hash_limit).min(self.map_hashes.len());
        let seg = (start / hash_limit) as i64;
        build_hmu(&self.hash, seg, &self.map_hashes[start..end])
    }

    /// The resource hash.
    pub fn resource_hash(&self) -> [u8; 32] {
        self.hash
    }

    /// The random hash the map hashes and resource hash are salted with: the caller's, or
    /// a re-drawn one if the caller's produced a map-hash collision.
    pub fn random_hash(&self) -> [u8; RANDOM_HASH_LEN] {
        self.random_hash
    }

    /// The proof the receiver must return for a correct transfer.
    pub fn expected_proof(&self) -> [u8; 32] {
        self.expected_proof
    }

    /// Total parts.
    pub fn total_parts(&self) -> usize {
        self.map_hashes.len()
    }
}
