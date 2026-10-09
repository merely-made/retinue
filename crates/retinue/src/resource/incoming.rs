//! The receiving half of one resource segment.

use alloc::vec;
use alloc::vec::Vec;

use super::{
    Advertisement, DEFAULT_MAX_DECOMPRESSED_SIZE, DEFAULT_MAX_PARTS, FLAG_COMPRESSED,
    HASHMAP_MAX_PARTS, Hmu, MAPHASH_LEN, RANDOM_HASH_LEN, WINDOW_MAX, build_exhausted_request,
    build_request, data_from_content, map_hash, proof, resource_hash,
};
#[cfg(feature = "compression")]
use super::{BoundedDecompressError, decompress_bounded};
use crate::{Error, Result};

/// Receiver state for one incoming resource segment.
///
/// Requests parts, solicits more hashmap via [`Hmu`] when the known hashes run out, then
/// reassembles, decompresses, verifies, and proves. One segment is up to ~1 MB.
///
/// Parts are stored by index, as RNS stores them, and matched only within the window after
/// the first missing part, so a map hash repeated elsewhere cannot misplace one.
pub struct Incoming {
    hash: [u8; 32],
    random_hash: Vec<u8>,
    compressed: bool,
    has_metadata: bool,
    total_parts: usize,
    /// Map hashes by part index; `None` until the advertisement or an [`Hmu`] names them.
    hashmap: Vec<Option<[u8; MAPHASH_LEN]>>,
    /// How many entries of `hashmap` are known.
    hashmap_height: usize,
    /// Hashes per hashmap segment: an [`Hmu`] for segment `s` starts at `s * segment_len`.
    /// RNS fixes this at [`HASHMAP_MAX_PARTS`]; a sender on a narrow link advertises fewer,
    /// and the advertisement's own count is the segment length it then uses.
    segment_len: usize,
    /// Collected parts by index. Never longer than the advertised count, which `max_parts`
    /// caps.
    parts: Vec<Option<Vec<u8>>>,
    /// How many entries of `parts` are filled.
    received: usize,
    /// The index of the first missing part: every part before it has arrived.
    consecutive: usize,
    /// How many parts past `consecutive` are requested and matched.
    window: usize,
    /// The most parts this receiver will accept for one segment. A runtime cap, not a const
    /// generic: a structural bound would commit the worst case as static storage.
    max_parts: usize,
}

impl Incoming {
    /// Begin receiving from an advertisement. Accepts a partial hashmap (a large resource
    /// whose hashmap streams via [`Hmu`]); the missing hashes arrive later.
    pub fn new(adv: &Advertisement) -> Result<Self> {
        Self::new_with_max_parts(adv, DEFAULT_MAX_PARTS)
    }

    /// Begin receiving with an explicit part ceiling. A board sets this far lower than a
    /// desktop; see `design_docs/2026-07-31_retinue_small_plan.md`.
    pub fn new_with_max_parts(adv: &Advertisement, max_parts: usize) -> Result<Self> {
        if adv.resource_hash.len() != 32 {
            return Err(Error::BadRequest);
        }
        // `parts` is a peer-chosen wire u64; unbounded, it sizes our reassembly state.
        if adv.parts > max_parts as u64 {
            return Err(Error::CapacityExceeded);
        }
        let total_parts = adv.parts as usize;
        // The initial hashmap is peer input too: it may not exceed the advertised count, and
        // a partial map must name at least one part, or there is no segment length.
        let advertised = adv.hashmap.len() / MAPHASH_LEN;
        if !adv.hashmap.len().is_multiple_of(MAPHASH_LEN)
            || advertised > total_parts
            || (advertised == 0 && total_parts > 0)
            || adv.random_hash.len() != RANDOM_HASH_LEN
        {
            return Err(Error::BadRequest);
        }
        let mut hash = [0u8; 32];
        hash.copy_from_slice(&adv.resource_hash);
        let mut hashmap = vec![None; total_parts];
        for (slot, m) in hashmap
            .iter_mut()
            .zip(adv.hashmap.as_chunks::<MAPHASH_LEN>().0)
        {
            *slot = Some(*m);
        }
        Ok(Self {
            hash,
            random_hash: adv.random_hash.clone(),
            compressed: adv.flags & FLAG_COMPRESSED != 0,
            // Later segments carry the flag, but only the first carries the metadata.
            has_metadata: adv.has_metadata() && adv.i <= 1,
            total_parts,
            hashmap,
            hashmap_height: advertised,
            segment_len: if advertised < total_parts {
                advertised
            } else {
                HASHMAP_MAX_PARTS
            },
            parts: vec![None; total_parts],
            received: 0,
            consecutive: 0,
            window: HASHMAP_MAX_PARTS,
            max_parts,
        })
    }

    /// Request and match at most `window` parts past the first missing one (clamped to
    /// `1..=`[`WINDOW_MAX`]). The default is [`HASHMAP_MAX_PARTS`].
    pub fn with_window(mut self, window: usize) -> Self {
        self.window = window.clamp(1, WINDOW_MAX);
        self
    }

    /// Whether the advertisement said the payload is bz2-compressed.
    pub fn is_compressed(&self) -> bool {
        self.compressed
    }

    /// Whether the advertisement said the data starts with metadata.
    pub fn has_metadata(&self) -> bool {
        self.has_metadata
    }

    /// Total parts in this segment.
    pub fn total_parts(&self) -> usize {
        self.total_parts
    }

    /// How many part hashes are known so far (advertisement + ingested HMUs).
    pub fn order_len(&self) -> usize {
        self.hashmap_height
    }

    /// The parts in the window after the first missing part, up to the first whose map hash
    /// is not yet known. These are what to ask for next.
    fn window_range(&self) -> core::ops::Range<usize> {
        self.consecutive..(self.consecutive + self.window).min(self.total_parts)
    }

    /// Known map hashes not yet collected in the current window. These are what to ask for
    /// next. The scan stops at the first part whose hash is not yet known.
    pub fn missing_known(&self) -> Vec<[u8; MAPHASH_LEN]> {
        self.window_range()
            .map_while(|i| self.hashmap[i].map(|m| (i, m)))
            .filter(|&(i, _)| self.parts[i].is_none())
            .map(|(_, m)| m)
            .collect()
    }

    /// Whether every known map hash has been collected (but more may remain via HMU).
    pub fn all_known_collected(&self) -> bool {
        // Parts are only ever placed where a hash is known.
        self.received == self.hashmap_height
    }

    /// Whether the full hashmap is known (all part hashes, via advertisement + HMUs).
    pub fn have_all_hashes(&self) -> bool {
        self.hashmap_height >= self.total_parts
    }

    /// Whether more hashmap is needed: the first missing part's hash is not yet known.
    pub fn needs_hmu(&self) -> bool {
        self.hashmap
            .get(self.consecutive)
            .is_some_and(|m| m.is_none())
    }

    /// A normal request for the given map hashes.
    pub fn request(&self, wanted: &[[u8; MAPHASH_LEN]]) -> Vec<u8> {
        build_request(&self.hash, wanted)
    }

    /// An exhausted request soliciting more hashmap, referencing the last known map hash.
    pub fn solicit_hmu(&self) -> Vec<u8> {
        let last = self
            .hashmap_height
            .checked_sub(1)
            .and_then(|i| self.hashmap[i])
            .unwrap_or([0u8; MAPHASH_LEN]);
        build_exhausted_request(&last, &self.hash, &[])
    }

    /// Ingest an HMU's hashes at their place in the map: segment `s` starts at part
    /// `s * segment_len`, as RNS places it. Returns how many hashes were newly learned.
    ///
    /// Nothing lands past the advertised part count, itself capped by `max_parts`, and a
    /// hash already known is kept, so a repeated HMU changes nothing.
    pub fn ingest_hmu(&mut self, hmu: &Hmu) -> usize {
        let Some(start) = usize::try_from(hmu.segment)
            .ok()
            .and_then(|segment| segment.checked_mul(self.segment_len))
        else {
            return 0;
        };
        let mut added = 0;
        for (slot, m) in self.hashmap.iter_mut().skip(start).zip(&hmu.hashes) {
            if slot.is_none() {
                *slot = Some(*m);
                added += 1;
            }
        }
        self.hashmap_height += added;
        added
    }

    /// The part ceiling this receiver was built with.
    pub fn max_parts(&self) -> usize {
        self.max_parts
    }

    /// Take a received part (a raw token slice). Returns true only when it fills a missing
    /// part in the current window; unknown, out-of-window and duplicate parts return false.
    pub fn accept_part(&mut self, part: &[u8]) -> bool {
        let m = map_hash(part, &self.random_hash);
        let Some(i) = self
            .window_range()
            .find(|&i| self.parts[i].is_none() && self.hashmap[i] == Some(m))
        else {
            return false;
        };
        self.parts[i] = Some(part.to_vec());
        self.received += 1;
        while self
            .parts
            .get(self.consecutive)
            .is_some_and(Option::is_some)
        {
            self.consecutive += 1;
        }
        true
    }

    /// Whether every part of the segment has arrived.
    pub fn is_complete(&self) -> bool {
        self.received == self.total_parts
    }

    /// Reassemble the token in transfer order. Verifies nothing; call [`recover`](Self::recover).
    pub fn assemble_token(&self) -> Result<Vec<u8>> {
        let mut token = Vec::with_capacity(self.token_len()?);
        for part in self.parts.iter().flatten() {
            token.extend_from_slice(part);
        }
        Ok(token)
    }

    /// [`assemble_token`](Self::assemble_token), releasing each part as it is copied, so the
    /// parts and the token are not both held whole. The parts are gone afterwards.
    pub fn take_token(&mut self) -> Result<Vec<u8>> {
        let mut token = Vec::with_capacity(self.token_len()?);
        for part in self.parts.iter_mut().filter_map(Option::take) {
            token.extend_from_slice(&part);
        }
        Ok(token)
    }

    /// The reassembled token's length, or [`Error::Truncated`] while any part is missing.
    fn token_len(&self) -> Result<usize> {
        self.parts.iter().try_fold(0, |len, part| {
            Ok(len + part.as_ref().ok_or(Error::Truncated)?.len())
        })
    }

    /// Recover the payload from the decrypted transfer blob: decompress if the
    /// advertisement flagged it, strip the `random_hash` prefix, and verify against the
    /// resource hash. This is the whole receive tail in one call.
    ///
    /// Decompression is bounded by [`DEFAULT_MAX_DECOMPRESSED_SIZE`]; see
    /// [`recover_with_limit`](Self::recover_with_limit).
    pub fn recover(&self, decrypted: &[u8]) -> Result<Vec<u8>> {
        self.recover_with_limit(decrypted, DEFAULT_MAX_DECOMPRESSED_SIZE)
    }

    /// [`recover`](Self::recover) with an explicit ceiling on the decompressed size.
    ///
    /// Returns [`Error::DecompressionLimit`] if a compressed body inflates past
    /// `max_decompressed` bytes (the output buffer never grows past that bound),
    /// [`Error::ResourceCorrupt`] if the recovered data does not match the hash, and
    /// [`Error::Unsupported`] if the resource is compressed but the `compression` feature is
    /// off.
    pub fn recover_with_limit(&self, decrypted: &[u8], max_decompressed: usize) -> Result<Vec<u8>> {
        // The blob is `random_hash || body`; the prefix sits OUTSIDE the compression, so
        // strip it first, then decompress.
        let body = data_from_content(decrypted)?;
        let data = if self.compressed {
            #[cfg(feature = "compression")]
            {
                decompress_bounded(body, max_decompressed).map_err(|e| match e {
                    BoundedDecompressError::InvalidData => Error::BadPadding,
                    BoundedDecompressError::LimitExceeded => Error::DecompressionLimit,
                })?
            }
            #[cfg(not(feature = "compression"))]
            {
                let _ = max_decompressed;
                return Err(Error::Unsupported);
            }
        } else {
            body.to_vec()
        };
        if !self.verify(&data) {
            return Err(Error::ResourceCorrupt);
        }
        Ok(data)
    }

    /// Check that decrypted (and decompressed) `data` matches the advertised resource hash.
    pub fn verify(&self, data: &[u8]) -> bool {
        resource_hash(data, &self.random_hash) == self.hash
    }

    /// The proof to return for `data`: `SHA256(data || resource_hash)`.
    pub fn proof(&self, data: &[u8]) -> [u8; 32] {
        proof(data, &self.hash)
    }

    /// The resource hash from the advertisement.
    pub fn resource_hash(&self) -> [u8; 32] {
        self.hash
    }
}
