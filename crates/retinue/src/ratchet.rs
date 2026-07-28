//! Caller-owned receive ratchets for link-less asymmetric packets.
//!
//! A destination advertises the public half of its current X25519 ratchet in announces.
//! Senders encrypt to that public key. The packet carries no ratchet id, so receivers try
//! retained private epochs until one authenticates.
//!
//! This store owns rotation, age/count retention, and a versioned snapshot. It does not
//! own a clock, entropy, or durable storage. Hosts supply all three. Snapshot bytes contain
//! private keys and must be protected like an identity secret.

use core::fmt;
use std::time::Duration;

use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};

use crate::hash::NameHash;
use crate::identity::{KEY_LEN, PrivateIdentity};

const SNAPSHOT_MAGIC: &[u8; 4] = b"RTRT";
const SNAPSHOT_VERSION: u8 = 1;
const SNAPSHOT_HEADER_LEN: usize = SNAPSHOT_MAGIC.len() + 1 + 4;
const SNAPSHOT_ENTRY_LEN: usize = 8 + KEY_LEN;
const MAX_SNAPSHOT_EPOCHS: usize = 4_096;

/// RNS-compatible default rotation and retention policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatchetPolicy {
    /// Maximum private epochs retained for trial decryption.
    pub max_count: usize,
    /// Minimum age of the current epoch before a supplied replacement is installed.
    pub rotation_interval: Duration,
    /// Maximum age of a private epoch.
    pub max_age: Duration,
}

impl Default for RatchetPolicy {
    fn default() -> Self {
        Self {
            max_count: 512,
            rotation_interval: Duration::from_secs(30 * 60),
            max_age: Duration::from_secs(30 * 24 * 60 * 60),
        }
    }
}

/// Ratchet state or snapshot validation failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RatchetError {
    InvalidPolicy,
    InvalidTimestamp,
    InvalidSnapshot,
    UnsupportedSnapshotVersion(u8),
    Token(crate::Error),
}

impl fmt::Display for RatchetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPolicy => f.write_str("invalid ratchet policy"),
            Self::InvalidTimestamp => f.write_str("invalid ratchet timestamp"),
            Self::InvalidSnapshot => f.write_str("invalid ratchet snapshot"),
            Self::UnsupportedSnapshotVersion(version) => {
                write!(f, "unsupported ratchet snapshot version {version}")
            }
            Self::Token(error) => write!(f, "ratchet token: {error}"),
        }
    }
}

impl std::error::Error for RatchetError {}

impl From<crate::Error> for RatchetError {
    fn from(value: crate::Error) -> Self {
        Self::Token(value)
    }
}

#[derive(Clone)]
struct RatchetEpoch {
    secret: [u8; KEY_LEN],
    created_at: f64,
}

impl RatchetEpoch {
    fn public(&self) -> [u8; KEY_LEN] {
        *XPublicKey::from(&StaticSecret::from(self.secret)).as_bytes()
    }

    fn id(&self) -> NameHash {
        NameHash::of(&self.public())
    }
}

/// Result of checking whether the receive ratchet should rotate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RatchetRotationReceipt {
    pub rotated: bool,
    pub current: NameHash,
    pub expired: usize,
    pub evicted: usize,
}

/// Result of restoring a snapshot under the current policy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RatchetRestoreReceipt {
    pub loaded: usize,
    pub expired: usize,
    pub evicted: usize,
}

/// Bounded receive-ratchet state with caller-managed persistence.
#[derive(Clone)]
pub struct RatchetStore {
    policy: RatchetPolicy,
    // Newest first. Order determines the receiver's trial-decryption order.
    epochs: Vec<RatchetEpoch>,
}

impl fmt::Debug for RatchetStore {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RatchetStore")
            .field("policy", &self.policy)
            .field("epochs", &self.epochs.len())
            .field("current", &self.current_id())
            .finish()
    }
}

impl RatchetStore {
    pub fn new(policy: RatchetPolicy) -> Result<Self, RatchetError> {
        validate_policy(&policy)?;
        Ok(Self {
            policy,
            epochs: Vec::new(),
        })
    }

    pub fn len(&self) -> usize {
        self.epochs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.epochs.is_empty()
    }

    pub fn policy(&self) -> &RatchetPolicy {
        &self.policy
    }

    pub fn current_public(&self) -> Option<[u8; KEY_LEN]> {
        self.epochs.first().map(RatchetEpoch::public)
    }

    pub fn current_id(&self) -> Option<NameHash> {
        self.epochs.first().map(RatchetEpoch::id)
    }

    /// Install `next_secret` if there is no current epoch or its interval has elapsed.
    ///
    /// Supplying entropy does not force a rotation. Hosts can call this at every announce;
    /// the unused secret is discarded while the current epoch is still young.
    pub fn rotate_if_due(
        &mut self,
        next_secret: [u8; KEY_LEN],
        now: f64,
    ) -> Result<RatchetRotationReceipt, RatchetError> {
        validate_timestamp(now)?;
        let expired = self.prune_at(now);
        let due = self.epochs.first().is_none_or(|current| {
            now - current.created_at >= self.policy.rotation_interval.as_secs_f64()
        });

        let mut evicted = 0;
        if due {
            self.epochs.insert(
                0,
                RatchetEpoch {
                    secret: next_secret,
                    created_at: now,
                },
            );
            if self.epochs.len() > self.policy.max_count {
                evicted = self.epochs.len() - self.policy.max_count;
                self.epochs.truncate(self.policy.max_count);
            }
        }

        Ok(RatchetRotationReceipt {
            rotated: due,
            current: self
                .current_id()
                .expect("rotation installs an epoch when none exists"),
            expired,
            evicted,
        })
    }

    /// Remove expired receive epochs.
    pub fn prune(&mut self, now: f64) -> Result<usize, RatchetError> {
        validate_timestamp(now)?;
        Ok(self.prune_at(now))
    }

    /// Trial-decrypt using the retained epochs, newest first.
    pub fn decrypt(
        &self,
        recipient: &PrivateIdentity,
        token: &[u8],
    ) -> Result<(Vec<u8>, NameHash), RatchetError> {
        Ok(crate::token::decrypt_with_ratchets(
            recipient,
            self.epochs.iter().map(|epoch| &epoch.secret),
            token,
        )?)
    }

    /// Encode the complete logical state as a fixed, versioned binary snapshot.
    ///
    /// Derived public keys and ids are omitted and re-derived on restore. Entries are
    /// encoded newest first as `created_at(f64 LE) || private_key(32)`.
    pub fn encode_snapshot(&self) -> Vec<u8> {
        let count = u32::try_from(self.epochs.len())
            .expect("validated policy bounds the epoch count to u32");
        let mut out =
            Vec::with_capacity(SNAPSHOT_HEADER_LEN + self.epochs.len() * SNAPSHOT_ENTRY_LEN);
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.push(SNAPSHOT_VERSION);
        out.extend_from_slice(&count.to_le_bytes());
        for epoch in &self.epochs {
            out.extend_from_slice(&epoch.created_at.to_le_bytes());
            out.extend_from_slice(&epoch.secret);
        }
        out
    }

    /// Restore atomically, reapplying current age and count limits.
    pub fn restore(
        policy: RatchetPolicy,
        snapshot: &[u8],
        now: f64,
    ) -> Result<(Self, RatchetRestoreReceipt), RatchetError> {
        validate_policy(&policy)?;
        validate_timestamp(now)?;
        if snapshot.len() < SNAPSHOT_HEADER_LEN || &snapshot[..4] != SNAPSHOT_MAGIC {
            return Err(RatchetError::InvalidSnapshot);
        }
        let version = snapshot[4];
        if version != SNAPSHOT_VERSION {
            return Err(RatchetError::UnsupportedSnapshotVersion(version));
        }
        let count = u32::from_le_bytes(
            snapshot[5..9]
                .try_into()
                .map_err(|_| RatchetError::InvalidSnapshot)?,
        ) as usize;
        if count > MAX_SNAPSHOT_EPOCHS {
            return Err(RatchetError::InvalidSnapshot);
        }
        let expected = SNAPSHOT_HEADER_LEN
            .checked_add(
                count
                    .checked_mul(SNAPSHOT_ENTRY_LEN)
                    .ok_or(RatchetError::InvalidSnapshot)?,
            )
            .ok_or(RatchetError::InvalidSnapshot)?;
        if snapshot.len() != expected {
            return Err(RatchetError::InvalidSnapshot);
        }

        let mut epochs = Vec::with_capacity(count.min(policy.max_count));
        let mut receipt = RatchetRestoreReceipt::default();
        let mut previous_created_at = f64::INFINITY;
        for raw in snapshot[SNAPSHOT_HEADER_LEN..].chunks_exact(SNAPSHOT_ENTRY_LEN) {
            let created_at = f64::from_le_bytes(
                raw[..8]
                    .try_into()
                    .map_err(|_| RatchetError::InvalidSnapshot)?,
            );
            validate_timestamp(created_at).map_err(|_| RatchetError::InvalidSnapshot)?;
            if created_at > previous_created_at {
                return Err(RatchetError::InvalidSnapshot);
            }
            previous_created_at = created_at;

            if now - created_at > policy.max_age.as_secs_f64() {
                receipt.expired += 1;
                continue;
            }
            if epochs.len() == policy.max_count {
                receipt.evicted += 1;
                continue;
            }
            let secret = raw[8..]
                .try_into()
                .map_err(|_| RatchetError::InvalidSnapshot)?;
            epochs.push(RatchetEpoch { secret, created_at });
            receipt.loaded += 1;
        }

        Ok((Self { policy, epochs }, receipt))
    }

    fn prune_at(&mut self, now: f64) -> usize {
        let before = self.epochs.len();
        let max_age = self.policy.max_age.as_secs_f64();
        self.epochs
            .retain(|epoch| now - epoch.created_at <= max_age);
        before - self.epochs.len()
    }
}

fn validate_policy(policy: &RatchetPolicy) -> Result<(), RatchetError> {
    if policy.max_count == 0
        || policy.max_count > MAX_SNAPSHOT_EPOCHS
        || policy.rotation_interval.is_zero()
        || policy.max_age.is_zero()
    {
        return Err(RatchetError::InvalidPolicy);
    }
    Ok(())
}

fn validate_timestamp(timestamp: f64) -> Result<(), RatchetError> {
    if timestamp.is_finite() {
        Ok(())
    } else {
        Err(RatchetError::InvalidTimestamp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy(max_count: usize, rotation: u64, max_age: u64) -> RatchetPolicy {
        RatchetPolicy {
            max_count,
            rotation_interval: Duration::from_secs(rotation),
            max_age: Duration::from_secs(max_age),
        }
    }

    #[test]
    fn rotation_respects_interval_count_and_age() {
        let mut store = RatchetStore::new(policy(2, 10, 25)).unwrap();
        let first = store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        assert!(first.rotated);
        assert_eq!(store.len(), 1);

        let retained = store.rotate_if_due([2; KEY_LEN], 9.0).unwrap();
        assert!(!retained.rotated);
        assert_eq!(retained.current, first.current);

        assert!(store.rotate_if_due([2; KEY_LEN], 10.0).unwrap().rotated);
        let third = store.rotate_if_due([3; KEY_LEN], 20.0).unwrap();
        assert!(third.rotated);
        assert_eq!(third.evicted, 1);
        assert_eq!(store.len(), 2);

        assert_eq!(store.prune(36.0).unwrap(), 1);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn snapshot_round_trip_rederives_state_and_reapplies_policy() {
        let mut original = RatchetStore::new(policy(3, 10, 100)).unwrap();
        original.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        original.rotate_if_due([2; KEY_LEN], 10.0).unwrap();
        original.rotate_if_due([3; KEY_LEN], 20.0).unwrap();
        let snapshot = original.encode_snapshot();

        let (restored, receipt) =
            RatchetStore::restore(policy(2, 10, 25), &snapshot, 30.0).unwrap();
        assert_eq!(
            receipt,
            RatchetRestoreReceipt {
                loaded: 2,
                expired: 1,
                evicted: 0,
            }
        );
        assert_eq!(restored.len(), 2);
        assert_eq!(restored.current_id(), original.current_id());
    }

    #[test]
    fn malformed_or_reordered_snapshots_are_rejected_atomically() {
        let mut store = RatchetStore::new(policy(2, 10, 100)).unwrap();
        store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        store.rotate_if_due([2; KEY_LEN], 10.0).unwrap();
        let snapshot = store.encode_snapshot();

        assert_eq!(
            RatchetStore::restore(policy(2, 10, 100), &snapshot[..8], 10.0).unwrap_err(),
            RatchetError::InvalidSnapshot,
        );

        let mut reordered = snapshot;
        reordered[SNAPSHOT_HEADER_LEN..].rotate_left(SNAPSHOT_ENTRY_LEN);
        assert_eq!(
            RatchetStore::restore(policy(2, 10, 100), &reordered, 10.0).unwrap_err(),
            RatchetError::InvalidSnapshot,
        );
    }

    #[test]
    fn debug_output_never_contains_private_key_bytes() {
        let mut store = RatchetStore::new(policy(2, 10, 100)).unwrap();
        store.rotate_if_due([0xA5; KEY_LEN], 0.0).unwrap();
        let debug = format!("{store:?}");
        assert!(!debug.contains(&"a5".repeat(KEY_LEN)));
    }
}
