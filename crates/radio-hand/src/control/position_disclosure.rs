//! Position disclosure ACL: PD1 (the record) and PD2 (activation), per the
//! position disclosure plan and the PD0 ruling in the field-node security posture.
//!
//! Two encodings carry one truth. The **wire** record, [`PositionAclV1`], is what the
//! owner compiles on the host from gazette and sends under the FS2 envelope: plaintext
//! sixteen-byte address hashes, each with a tier. The **stored** table,
//! [`BlindedPositionAcl`], is what the node keeps: every hash replaced by a keyed hash
//! under a node-local secret, so a flash dump yields a grant count and no identity.
//! The split exists because the host does not hold the node's secret and therefore
//! cannot blind; the node blinds at write time and never persists the plaintext.
//!
//! What this table governs, and what it does not. It governs the tier at which a
//! directed position request from a given identity is answered. It never governs who
//! may command the node; that is the owner's FS2 key, independent of anything here.
//! That separation is what makes PD2's no-rollback rule safe: a bad table is always
//! correctable by the next command, so there is no path by which the node reverts to
//! an earlier table, because reverting would re-grant whoever was just revoked.
//!
//! The absent identity is a defined case, not a convention. A hash not in the table
//! resolves to [`Resolved::Broadcast`], meaning the asker gets whatever the broadcast
//! tier in the public configuration says, and never more. A node with no table at all
//! resolves everything to `Broadcast`. Neither case is an error, because a node that
//! goes silent on a lookup is indistinguishable from a broken one.
//!
//! Capacity refuses rather than evicts. Silently dropping a kin entry to make room is a
//! privacy failure that presents as a bug, so an over-capacity record fails to decode
//! and the refusal is counted; the owner prunes and resends.

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

/// Disclosure tier. A closed set on purpose: gazette's `trust` is a `String`, and a
/// decision keyed on a string defaults wrong on a typo, so the projection maps to this
/// before anything reaches a node.
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
    Capacity { offered: usize, limit: usize },
    /// Entries out of ascending hash order, or a hash listed twice. The canonical form
    /// is sorted and unique so that one table has exactly one encoding.
    NonCanonicalOrder,
    /// PD2: the record's sequence is not strictly greater than the accepted one.
    NotMonotonic { offered: u64, accepted: u64 },
}
impl fmt::Display for PositionAclError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length => f.write_str("position ACL length is not canonical"),
            Self::UnsupportedVersion(v) => write!(f, "position ACL version {v} unsupported"),
            Self::InvalidAbsentPolicy(b) => write!(f, "position ACL absent policy byte {b:#04x} invalid"),
            Self::InvalidTier(b) => write!(f, "position ACL tier byte {b:#04x} invalid"),
            Self::Capacity { offered, limit } => {
                write!(f, "position ACL offers {offered} entries, table holds {limit}")
            }
            Self::NonCanonicalOrder => f.write_str("position ACL entries not sorted and unique"),
            Self::NotMonotonic { offered, accepted } => {
                write!(f, "position ACL sequence {offered} not above accepted {accepted}")
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
        let absent =
            AbsentPolicy::from_byte(bytes[9]).ok_or(PositionAclError::InvalidAbsentPolicy(bytes[9]))?;
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
        for i in 0..count {
            let at = POSITION_ACL_HEADER_LEN + i * POSITION_ACL_ENTRY_LEN;
            let mut hash = [0u8; POSITION_ACL_HASH_LEN];
            hash.copy_from_slice(&bytes[at..at + POSITION_ACL_HASH_LEN]);
            let tier_byte = bytes[at + POSITION_ACL_HASH_LEN];
            let tier = DisclosureTier::from_byte(tier_byte)
                .ok_or(PositionAclError::InvalidTier(tier_byte))?;
            if let Some(prev) = previous {
                if prev >= hash {
                    return Err(PositionAclError::NonCanonicalOrder);
                }
            }
            previous = Some(hash);
            entries[i] = Some(PositionAclEntry { hash, tier });
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
        if let Some(accepted) = self.accepted_sequence {
            if record.sequence() <= accepted {
                self.refused = self.refused.saturating_add(1);
                return Err(PositionAclError::NotMonotonic {
                    offered: record.sequence(),
                    accepted,
                });
            }
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
mod tests {
    use super::*;

    const SECRET: [u8; 32] = [0x5A; 32];

    fn hash(seed: u8) -> [u8; 16] {
        [seed; 16]
    }
    fn entry(seed: u8, tier: DisclosureTier) -> PositionAclEntry {
        PositionAclEntry {
            hash: hash(seed),
            tier,
        }
    }

    #[test]
    fn round_trips_canonically_and_sorts_on_construction() {
        let record = PositionAclV1::<8>::new(
            7,
            AbsentPolicy::Broadcast,
            &[
                entry(0x30, DisclosureTier::Precise),
                entry(0x10, DisclosureTier::Coarse),
                entry(0x20, DisclosureTier::Off),
            ],
        )
        .unwrap();
        let hashes: heapless::Vec<u8, 8> = record.entries().map(|e| e.hash[0]).collect();
        assert_eq!(&hashes[..], &[0x10, 0x20, 0x30]);
        let mut buf = [0u8; 128];
        let n = record.encode(&mut buf).unwrap();
        assert_eq!(n, POSITION_ACL_HEADER_LEN + 3 * POSITION_ACL_ENTRY_LEN);
        let decoded = PositionAclV1::<8>::decode(&buf[..n]).unwrap();
        assert_eq!(decoded, record);
        let mut again = [0u8; 128];
        let m = decoded.encode(&mut again).unwrap();
        assert_eq!(&buf[..n], &again[..m], "encoding is canonical");
    }

    #[test]
    fn decode_rejects_every_non_canonical_form() {
        let record = PositionAclV1::<8>::new(
            1,
            AbsentPolicy::Fixed(DisclosureTier::Coarse),
            &[entry(0x10, DisclosureTier::Precise), entry(0x20, DisclosureTier::Precise)],
        )
        .unwrap();
        let mut buf = [0u8; 128];
        let n = record.encode(&mut buf).unwrap();

        assert_eq!(PositionAclV1::<8>::decode(&buf[..n - 1]), Err(PositionAclError::Length));
        let mut v = buf;
        v[0] = 9;
        assert_eq!(PositionAclV1::<8>::decode(&v[..n]), Err(PositionAclError::UnsupportedVersion(9)));
        let mut a = buf;
        a[9] = 0x7E;
        assert_eq!(PositionAclV1::<8>::decode(&a[..n]), Err(PositionAclError::InvalidAbsentPolicy(0x7E)));
        let mut t = buf;
        t[POSITION_ACL_HEADER_LEN + POSITION_ACL_HASH_LEN] = 3;
        assert_eq!(PositionAclV1::<8>::decode(&t[..n]), Err(PositionAclError::InvalidTier(3)));
        let mut swapped = buf;
        swapped[POSITION_ACL_HEADER_LEN..POSITION_ACL_HEADER_LEN + 16].copy_from_slice(&hash(0x20));
        swapped[POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN..POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN + 16]
            .copy_from_slice(&hash(0x10));
        assert_eq!(PositionAclV1::<8>::decode(&swapped[..n]), Err(PositionAclError::NonCanonicalOrder));
        let mut dup = buf;
        dup[POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN..POSITION_ACL_HEADER_LEN + POSITION_ACL_ENTRY_LEN + 16]
            .copy_from_slice(&hash(0x10));
        assert_eq!(PositionAclV1::<8>::decode(&dup[..n]), Err(PositionAclError::NonCanonicalOrder));
    }

    #[test]
    fn duplicate_hashes_are_refused_at_construction() {
        let err = PositionAclV1::<8>::new(
            1,
            AbsentPolicy::Broadcast,
            &[entry(0x10, DisclosureTier::Off), entry(0x10, DisclosureTier::Precise)],
        )
        .unwrap_err();
        assert_eq!(err, PositionAclError::NonCanonicalOrder);
    }

    #[test]
    fn capacity_refuses_and_never_evicts() {
        let too_many: [PositionAclEntry; 3] = [
            entry(1, DisclosureTier::Precise),
            entry(2, DisclosureTier::Precise),
            entry(3, DisclosureTier::Precise),
        ];
        assert_eq!(
            PositionAclV1::<2>::new(1, AbsentPolicy::Broadcast, &too_many).unwrap_err(),
            PositionAclError::Capacity { offered: 3, limit: 2 }
        );
        let big = PositionAclV1::<8>::new(1, AbsentPolicy::Broadcast, &too_many).unwrap();
        let mut buf = [0u8; 128];
        let n = big.encode(&mut buf).unwrap();
        assert_eq!(
            PositionAclV1::<2>::decode(&buf[..n]).unwrap_err(),
            PositionAclError::Capacity { offered: 3, limit: 2 }
        );
    }

    #[test]
    fn no_table_resolves_everything_to_broadcast_and_never_fails() {
        let table = BlindedPositionAcl::<8>::new();
        assert_eq!(table.grants(), 0);
        assert_eq!(table.accepted_sequence(), None);
        assert_eq!(table.resolve(&hash(0x10), &SECRET), Resolved::Broadcast);
    }

    #[test]
    fn absent_identity_gets_broadcast_by_default_and_fixed_when_set() {
        let mut table = BlindedPositionAcl::<8>::new();
        let whitelist = PositionAclV1::<8>::new(
            1,
            AbsentPolicy::Broadcast,
            &[entry(0x10, DisclosureTier::Precise)],
        )
        .unwrap();
        table.apply(&whitelist, &SECRET).unwrap();
        assert_eq!(table.resolve(&hash(0x10), &SECRET), Resolved::Tier(DisclosureTier::Precise));
        assert_eq!(table.resolve(&hash(0x99), &SECRET), Resolved::Broadcast);

        let blacklist = PositionAclV1::<8>::new(
            2,
            AbsentPolicy::Fixed(DisclosureTier::Precise),
            &[entry(0x10, DisclosureTier::Off)],
        )
        .unwrap();
        table.apply(&blacklist, &SECRET).unwrap();
        assert_eq!(table.resolve(&hash(0x10), &SECRET), Resolved::Tier(DisclosureTier::Off));
        assert_eq!(table.resolve(&hash(0x99), &SECRET), Resolved::Tier(DisclosureTier::Precise));
    }

    #[test]
    fn stored_form_holds_no_plaintext_hash() {
        let mut table = BlindedPositionAcl::<8>::new();
        let record = PositionAclV1::<8>::new(
            1,
            AbsentPolicy::Broadcast,
            &[entry(0x42, DisclosureTier::Precise)],
        )
        .unwrap();
        table.apply(&record, &SECRET).unwrap();
        let (tag, _) = table.tags[0].unwrap();
        assert_ne!(tag, hash(0x42), "tag must not be the plaintext hash");
        assert_ne!(
            table.resolve(&hash(0x42), &[0x00; 32]),
            Resolved::Tier(DisclosureTier::Precise),
            "a different secret must not resolve the grant"
        );
    }

    #[test]
    fn pd2_sequence_is_monotonic_and_replay_is_refused_and_counted() {
        let mut table = BlindedPositionAcl::<8>::new();
        let five = PositionAclV1::<8>::new(5, AbsentPolicy::Broadcast, &[]).unwrap();
        let four = PositionAclV1::<8>::new(4, AbsentPolicy::Broadcast, &[]).unwrap();
        table.apply(&five, &SECRET).unwrap();
        assert_eq!(
            table.apply(&five, &SECRET).unwrap_err(),
            PositionAclError::NotMonotonic { offered: 5, accepted: 5 }
        );
        assert_eq!(
            table.apply(&four, &SECRET).unwrap_err(),
            PositionAclError::NotMonotonic { offered: 4, accepted: 5 }
        );
        assert_eq!(table.refused(), 2);
        assert_eq!(table.accepted_sequence(), Some(5));
    }

    #[test]
    fn pd2_revocation_holds_and_a_lockout_table_is_still_correctable() {
        // The owner's own hash is just another identity to this table; it governs
        // disclosure, never command authority, so a table that refuses everyone
        // (including the owner) is correctable by the next higher-sequence write.
        let owner = hash(0xAA);
        let kin = hash(0xBB);
        let mut table = BlindedPositionAcl::<8>::new();
        let grant = PositionAclV1::<8>::new(
            1,
            AbsentPolicy::Broadcast,
            &[
                PositionAclEntry { hash: owner, tier: DisclosureTier::Precise },
                PositionAclEntry { hash: kin, tier: DisclosureTier::Precise },
            ],
        )
        .unwrap();
        table.apply(&grant, &SECRET).unwrap();

        // Revoke kin. Kin must drop to Broadcast, and there is no method to restore it.
        let revoked = PositionAclV1::<8>::new(
            2,
            AbsentPolicy::Broadcast,
            &[PositionAclEntry { hash: owner, tier: DisclosureTier::Precise }],
        )
        .unwrap();
        table.apply(&revoked, &SECRET).unwrap();
        assert_eq!(table.resolve(&kin, &SECRET), Resolved::Broadcast);
        assert_eq!(table.apply(&grant, &SECRET).unwrap_err().is_not_monotonic(), true);
        assert_eq!(table.resolve(&kin, &SECRET), Resolved::Broadcast, "replay did not re-grant");

        // Lockout: refuse everyone, owner included.
        let lockout = PositionAclV1::<8>::new(
            3,
            AbsentPolicy::Fixed(DisclosureTier::Off),
            &[PositionAclEntry { hash: owner, tier: DisclosureTier::Off }],
        )
        .unwrap();
        table.apply(&lockout, &SECRET).unwrap();
        assert_eq!(table.resolve(&owner, &SECRET), Resolved::Tier(DisclosureTier::Off));

        // The owner's next command still lands: nothing here gated it.
        let corrected = PositionAclV1::<8>::new(
            4,
            AbsentPolicy::Broadcast,
            &[PositionAclEntry { hash: owner, tier: DisclosureTier::Precise }],
        )
        .unwrap();
        table.apply(&corrected, &SECRET).unwrap();
        assert_eq!(table.resolve(&owner, &SECRET), Resolved::Tier(DisclosureTier::Precise));
        assert_eq!(table.accepted_sequence(), Some(4));
    }

    impl PositionAclError {
        fn is_not_monotonic(self) -> bool {
            matches!(self, Self::NotMonotonic { .. })
        }
    }
}
