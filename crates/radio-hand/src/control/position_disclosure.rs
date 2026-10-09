//! Position disclosure ACL: PD1 (the record) and PD2 (activation), per the position
//! disclosure plan and the PD0 ruling in the field-node security posture.
//!
//! The **wire** record, [`PositionAclV1`], carries plaintext address hashes with tiers; the
//! host compiles it from gazette and sends it under FS2. The **stored** table,
//! [`BlindedPositionAcl`], keeps only keyed hashes under a node-local secret, so a flash
//! dump yields a grant count and no identity. The host cannot blind because it does not
//! hold the secret; the node blinds at write time and never persists plaintext.
//!
//! The table governs the tier at which a directed position request is answered, never who
//! may command the node (the owner's FS2 key). That keeps PD2's no-rollback rule safe: a bad
//! table is correctable by the next command, and reverting would re-grant the revoked.
//!
//! An absent identity, or a node with no table, resolves to [`Resolved::Broadcast`]: the
//! public broadcast tier and never more. Neither is an error, since a silent node is
//! indistinguishable from a broken one. Capacity refuses rather than evicts, because a
//! silently dropped kin entry is a privacy failure; the refusal is counted.

use core::fmt;

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

/// Schema byte preceding every [`PositionAclV1`] wire encoding.
pub const POSITION_ACL_V1_VERSION: u8 = 1;
/// A plaintext address hash on the wire, matching `retinue::hash::ADDRESS_HASH_LEN`.
pub const POSITION_ACL_HASH_LEN: usize = 16;
/// A blinded tag in the stored table. Truncated HMAC-SHA256; sixteen bytes is far past
/// collision concern for a table bounded in the tens of entries.
pub const POSITION_ACL_TAG_LEN: usize = 16;
/// Bytes of node-local secret the blinding takes.
pub const POSITION_ACL_SECRET_LEN: usize = 32;
/// Wire bytes per entry: hash then tier.
pub const POSITION_ACL_ENTRY_LEN: usize = POSITION_ACL_HASH_LEN + 1;
/// Wire header: version, sequence (8, big-endian), absent policy, entry count.
pub const POSITION_ACL_HEADER_LEN: usize = 1 + 8 + 1 + 1;

/// Disclosure tier. A closed set: gazette's `trust` is a `String`, and a decision keyed on
/// a string defaults wrong on a typo.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum DisclosureTier {
    /// Report nothing to this identity.
    Off = 0,
    /// Report a position quantised at the source. Obscured against one observation and
    /// not against sustained multi-receiver observation; see PD0.
    Coarse = 1,
    /// Report the fix as held.
    Precise = 2,
}
impl DisclosureTier {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Off),
            1 => Some(Self::Coarse),
            2 => Some(Self::Precise),
            _ => None,
        }
    }
    const fn as_byte(self) -> u8 {
        self as u8
    }
}

/// What an identity absent from the table receives.
///
/// `Broadcast` is whitelist semantics and the PD0 default: absent askers get the
/// broadcast tier and never more. `Fixed` is the owner-settable alternative that makes
/// blacklist semantics expressible: list the denied identities at `Off` and set the
/// absent policy to `Fixed(Precise)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbsentPolicy {
    Broadcast,
    Fixed(DisclosureTier),
}
impl AbsentPolicy {
    const BROADCAST_BYTE: u8 = 0xFF;
    fn from_byte(byte: u8) -> Option<Self> {
        if byte == Self::BROADCAST_BYTE {
            Some(Self::Broadcast)
        } else {
            DisclosureTier::from_byte(byte).map(Self::Fixed)
        }
    }
    const fn as_byte(self) -> u8 {
        match self {
            Self::Broadcast => Self::BROADCAST_BYTE,
            Self::Fixed(tier) => tier.as_byte(),
        }
    }
}

/// One wire entry: a plaintext address hash and its tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PositionAclEntry {
    pub hash: [u8; POSITION_ACL_HASH_LEN],
    pub tier: DisclosureTier,
}

/// Why a wire record failed to decode or a table refused a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PositionAclError {
    Length,
    UnsupportedVersion(u8),
    InvalidAbsentPolicy(u8),
    InvalidTier(u8),
    /// More entries than this node's table holds. Refused, never evicted.
    Capacity {
        offered: usize,
        limit: usize,
    },
    /// Entries out of ascending hash order, or a hash listed twice. The canonical form
    /// is sorted and unique so that one table has exactly one encoding.
    NonCanonicalOrder,
    /// PD2: the record's sequence is not strictly greater than the accepted one.
    NotMonotonic {
        offered: u64,
        accepted: u64,
    },
}
impl fmt::Display for PositionAclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length => f.write_str("position ACL length is not canonical"),
            Self::UnsupportedVersion(v) => write!(f, "position ACL version {v} unsupported"),
            Self::InvalidAbsentPolicy(b) => {
                write!(f, "position ACL absent policy byte {b:#04x} invalid")
            }
            Self::InvalidTier(b) => write!(f, "position ACL tier byte {b:#04x} invalid"),
            Self::Capacity { offered, limit } => {
                write!(
                    f,
                    "position ACL offers {offered} entries, table holds {limit}"
                )
            }
            Self::NonCanonicalOrder => f.write_str("position ACL entries not sorted and unique"),
            Self::NotMonotonic { offered, accepted } => {
                write!(
                    f,
                    "position ACL sequence {offered} not above accepted {accepted}"
                )
            }
        }
    }
}

/// The wire record the owner sends. Bounded by `N`, the same `N` as the node's table,
/// so an over-capacity record is refused at decode and never reaches storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionAclV1<const N: usize> {
    sequence: u64,
    absent: AbsentPolicy,
    entries: [Option<PositionAclEntry>; N],
    len: usize,
}
impl<const N: usize> PositionAclV1<N> {
    /// Constructs the canonical record. Entries are sorted by hash and must be unique.
    pub fn new(
        sequence: u64,
        absent: AbsentPolicy,
        entries: &[PositionAclEntry],
    ) -> Result<Self, PositionAclError> {
        if entries.len() > N {
            return Err(PositionAclError::Capacity {
                offered: entries.len(),
                limit: N,
            });
        }
        let mut table: [Option<PositionAclEntry>; N] = core::array::from_fn(|_| None);
        for (slot, entry) in table.iter_mut().zip(entries) {
            *slot = Some(*entry);
        }
        let len = entries.len();
        // Insertion sort: N is small and this stays no_std and allocation-free.
        for i in 1..len {
            let mut j = i;
            while j > 0 && table[j - 1].map(|e| e.hash) > table[j].map(|e| e.hash) {
                table.swap(j - 1, j);
                j -= 1;
            }
        }
        for pair in table[..len].windows(2) {
            if pair[0].map(|e| e.hash) == pair[1].map(|e| e.hash) {
                return Err(PositionAclError::NonCanonicalOrder);
            }
        }
        Ok(Self {
            sequence,
            absent,
            entries: table,
            len,
        })
    }

    pub const fn sequence(&self) -> u64 {
        self.sequence
    }
    pub const fn absent(&self) -> AbsentPolicy {
        self.absent
    }
    pub fn entries(&self) -> impl Iterator<Item = &PositionAclEntry> + '_ {
        self.entries[..self.len].iter().flatten()
    }
    pub const fn len(&self) -> usize {
        self.len
    }
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Exact bytes of this record's canonical encoding.
    pub const fn encoded_len(&self) -> usize {
        POSITION_ACL_HEADER_LEN + self.len * POSITION_ACL_ENTRY_LEN
    }

    /// Encodes the one and only canonical representation into `out`, returning the
    /// bytes written. `out` must hold at least [`Self::encoded_len`].
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, PositionAclError> {
        let needed = self.encoded_len();
        if out.len() < needed {
            return Err(PositionAclError::Length);
        }
        out[0] = POSITION_ACL_V1_VERSION;
        out[1..9].copy_from_slice(&self.sequence.to_be_bytes());
        out[9] = self.absent.as_byte();
        out[10] = self.len as u8;
        for (i, entry) in self.entries().enumerate() {
            let at = POSITION_ACL_HEADER_LEN + i * POSITION_ACL_ENTRY_LEN;
            out[at..at + POSITION_ACL_HASH_LEN].copy_from_slice(&entry.hash);
            out[at + POSITION_ACL_HASH_LEN] = entry.tier.as_byte();
        }
        Ok(needed)
    }

    /// Decodes exactly one canonical record. Unsorted or duplicated entries are
    /// rejected rather than repaired, so a byte string maps to at most one table.
    pub fn decode(bytes: &[u8]) -> Result<Self, PositionAclError> {
        if bytes.len() < POSITION_ACL_HEADER_LEN {
            return Err(PositionAclError::Length);
        }
        if bytes[0] != POSITION_ACL_V1_VERSION {
            return Err(PositionAclError::UnsupportedVersion(bytes[0]));
        }
        let mut seq = [0u8; 8];
        seq.copy_from_slice(&bytes[1..9]);
        let sequence = u64::from_be_bytes(seq);
        let absent = AbsentPolicy::from_byte(bytes[9])
            .ok_or(PositionAclError::InvalidAbsentPolicy(bytes[9]))?;
        let count = bytes[10] as usize;
        if count > N {
            return Err(PositionAclError::Capacity {
                offered: count,
                limit: N,
            });
        }
        if bytes.len() != POSITION_ACL_HEADER_LEN + count * POSITION_ACL_ENTRY_LEN {
            return Err(PositionAclError::Length);
        }
        let mut entries: [Option<PositionAclEntry>; N] = core::array::from_fn(|_| None);
        let mut previous: Option<[u8; POSITION_ACL_HASH_LEN]> = None;
        for (i, slot) in entries.iter_mut().enumerate().take(count) {
            let at = POSITION_ACL_HEADER_LEN + i * POSITION_ACL_ENTRY_LEN;
            let mut hash = [0u8; POSITION_ACL_HASH_LEN];
            hash.copy_from_slice(&bytes[at..at + POSITION_ACL_HASH_LEN]);
            let tier_byte = bytes[at + POSITION_ACL_HASH_LEN];
            let tier = DisclosureTier::from_byte(tier_byte)
                .ok_or(PositionAclError::InvalidTier(tier_byte))?;
            if previous.is_some_and(|prev| prev >= hash) {
                return Err(PositionAclError::NonCanonicalOrder);
            }
            previous = Some(hash);
            *slot = Some(PositionAclEntry { hash, tier });
        }
        Ok(Self {
            sequence,
            absent,
            entries,
            len: count,
        })
    }
}

/// Result of a directed lookup. `Broadcast` is the absent case and is never an error:
/// the caller answers at the public configuration's broadcast tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolved {
    Tier(DisclosureTier),
    Broadcast,
}

/// The stored table. Holds blinded tags only, the accepted sequence, and a refusal
/// counter. There is deliberately no method that restores an earlier table.
#[derive(Clone)]
pub struct BlindedPositionAcl<const N: usize> {
    accepted_sequence: Option<u64>,
    absent: AbsentPolicy,
    tags: [Option<([u8; POSITION_ACL_TAG_LEN], DisclosureTier)>; N],
    len: usize,
    refused: u32,
}
impl<const N: usize> fmt::Debug for BlindedPositionAcl<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BlindedPositionAcl")
            .field("accepted_sequence", &self.accepted_sequence)
            .field("absent", &self.absent)
            .field("grants", &self.len)
            .field("refused", &self.refused)
            .finish()
    }
}
impl<const N: usize> Default for BlindedPositionAcl<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> BlindedPositionAcl<N> {
    /// The no-table state. Every lookup resolves to `Broadcast`.
    pub fn new() -> Self {
        Self {
            accepted_sequence: None,
            absent: AbsentPolicy::Broadcast,
            tags: core::array::from_fn(|_| None),
            len: 0,
            refused: 0,
        }
    }

    /// Highest sequence accepted so far, or `None` for the no-table state.
    pub const fn accepted_sequence(&self) -> Option<u64> {
        self.accepted_sequence
    }
    /// Grants held. This is what a flash dump yields about the table.
    pub const fn grants(&self) -> usize {
        self.len
    }
    /// Writes refused, whether for sequence or capacity.
    pub const fn refused(&self) -> u32 {
        self.refused
    }

    /// PD2. Applies a wire record if and only if its sequence is strictly greater than
    /// the accepted one, blinding every hash under `secret` before it is retained. The
    /// previous table is gone the moment this returns `Ok`; there is no rollback and
    /// no method to reconstruct it.
    pub fn apply(
        &mut self,
        record: &PositionAclV1<N>,
        secret: &[u8; POSITION_ACL_SECRET_LEN],
    ) -> Result<(), PositionAclError> {
        if let Some(accepted) = self.accepted_sequence
            && record.sequence() <= accepted
        {
            self.refused = self.refused.saturating_add(1);
            return Err(PositionAclError::NotMonotonic {
                offered: record.sequence(),
                accepted,
            });
        }
        let mut tags: [Option<([u8; POSITION_ACL_TAG_LEN], DisclosureTier)>; N] =
            core::array::from_fn(|_| None);
        for (slot, entry) in tags.iter_mut().zip(record.entries()) {
            *slot = Some((blind(secret, &entry.hash), entry.tier));
        }
        self.tags = tags;
        self.len = record.len();
        self.absent = record.absent();
        self.accepted_sequence = Some(record.sequence());
        Ok(())
    }

    /// Resolves the tier for `asker`. Absent identities and the no-table state both
    /// return `Broadcast`; this never fails.
    pub fn resolve(
        &self,
        asker: &[u8; POSITION_ACL_HASH_LEN],
        secret: &[u8; POSITION_ACL_SECRET_LEN],
    ) -> Resolved {
        let tag = blind(secret, asker);
        for (stored, tier) in self.tags[..self.len].iter().flatten() {
            if constant_time_eq(stored, &tag) {
                return Resolved::Tier(*tier);
            }
        }
        match self.absent {
            AbsentPolicy::Broadcast => Resolved::Broadcast,
            AbsentPolicy::Fixed(tier) => Resolved::Tier(tier),
        }
    }
}

/// Keyed hash of an address hash under the node-local secret, truncated to the tag
/// length. A dump holding both secret and tags can test a candidate it already holds;
/// it cannot enumerate. That residual is stated in the seizure paragraph.
fn blind(
    secret: &[u8; POSITION_ACL_SECRET_LEN],
    hash: &[u8; POSITION_ACL_HASH_LEN],
) -> [u8; POSITION_ACL_TAG_LEN] {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC accepts any key length");
    mac.update(b"retinue.position-acl/v1");
    mac.update(hash);
    let full = mac.finalize().into_bytes();
    let mut tag = [0u8; POSITION_ACL_TAG_LEN];
    tag.copy_from_slice(&full[..POSITION_ACL_TAG_LEN]);
    tag
}

fn constant_time_eq(a: &[u8; POSITION_ACL_TAG_LEN], b: &[u8; POSITION_ACL_TAG_LEN]) -> bool {
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests;
