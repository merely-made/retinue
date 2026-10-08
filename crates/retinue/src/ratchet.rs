//! Receive ratchets for link-less asymmetric packets.
//!
//! A destination advertises the public half of its current X25519 ratchet in announces.
//! Senders encrypt to that public key. The packet carries no ratchet id, so receivers try
//! retained private epochs until one authenticates.
//!
//! The lifecycle follows RNS (`Destination.py` 85-90, 206-288, 437-475): the next announce
//! rotates once the rotation interval has passed, retention is by count, and the persisted
//! snapshot is signed by the destination identity and verified on restore. This store owns
//! rotation, retention, and the snapshot format. It does not own a clock, entropy, or
//! durable storage; hosts (the tokio endpoint among them) supply all three. Snapshot bytes
//! contain private keys and must be protected like an identity secret.

// Needed by the test build or the tokio shell; the bare no_std lib does not reach it.
#[allow(unused_imports)]
use alloc::format;

use alloc::vec::Vec;

use core::fmt;
use core::time::Duration;

use x25519_dalek::{PublicKey as XPublicKey, StaticSecret};

use crate::hash::NameHash;
use crate::identity::{Identity, KEY_LEN, PrivateIdentity, SIGNATURE_LEN};

const SNAPSHOT_MAGIC: &[u8; 4] = b"RTRT";
/// Version 2 appends the identity's Ed25519 signature. Version 1 was unauthenticated and is
/// refused.
const SNAPSHOT_VERSION: u8 = 2;
const SNAPSHOT_HEADER_LEN: usize = SNAPSHOT_MAGIC.len() + 1 + 4;
const SNAPSHOT_ENTRY_LEN: usize = 8 + KEY_LEN;
const MAX_SNAPSHOT_EPOCHS: usize = 4_096;

/// RNS-compatible default rotation and retention policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RatchetPolicy {
    /// Maximum private epochs retained for trial decryption (`RATCHET_COUNT`, 512).
    pub max_count: usize,
    /// Age the current epoch must exceed before the next announce rotates it
    /// (`RATCHET_INTERVAL`, 30 minutes).
    pub rotation_interval: Duration,
    /// How long an epoch stays decryptable after a newer one superseded it. `None`, the RNS
    /// behaviour, retains by count only. The current epoch never expires by time: it is the
    /// one peers were last told to use.
    pub max_superseded_age: Option<Duration>,
}

impl Default for RatchetPolicy {
    fn default() -> Self {
        Self {
            max_count: 512,
            rotation_interval: Duration::from_secs(30 * 60),
            max_superseded_age: None,
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
    /// The snapshot is not signed by the expected identity.
    InvalidSignature,
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
            Self::InvalidSignature => f.write_str("invalid ratchet snapshot signature"),
            Self::Token(error) => write!(f, "ratchet token: {error}"),
        }
    }
}

impl core::error::Error for RatchetError {}

impl From<crate::Error> for RatchetError {
    fn from(value: crate::Error) -> Self {
        Self::Token(value)
    }
}

/// One private epoch. The public key and id are derived once, not per trial decryption.
#[derive(Clone)]
struct RatchetEpoch {
    secret: [u8; KEY_LEN],
    public: [u8; KEY_LEN],
    id: NameHash,
    created_at: f64,
}

impl RatchetEpoch {
    fn new(secret: [u8; KEY_LEN], created_at: f64) -> Self {
        let public = *XPublicKey::from(&StaticSecret::from(secret)).as_bytes();
        Self {
            secret,
            public,
            id: NameHash::of(&public),
            created_at,
        }
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
        self.epochs.first().map(|epoch| epoch.public)
    }

    pub fn current_id(&self) -> Option<NameHash> {
        self.epochs.first().map(|epoch| epoch.id)
    }

    /// Whether [`Self::rotate_if_due`] at `now` would install a new epoch: there is none
    /// yet, or the current one is older than the rotation interval (`Destination.py` 230).
    pub fn rotation_due(&self, now: f64) -> bool {
        self.epochs.first().is_none_or(|current| {
            now - current.created_at > self.policy.rotation_interval.as_secs_f64()
        })
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
        let due = self.rotation_due(now);
        let mut evicted = 0;
        if due {
            self.epochs.insert(0, RatchetEpoch::new(next_secret, now));
            if self.epochs.len() > self.policy.max_count {
                evicted = self.epochs.len() - self.policy.max_count;
                self.epochs.truncate(self.policy.max_count);
            }
        }
        let expired = self.prune_at(now);

        Ok(RatchetRotationReceipt {
            rotated: due,
            current: self
                .current_id()
                .expect("rotation installs an epoch when none exists"),
            expired,
            evicted,
        })
    }

    /// Remove superseded epochs older than [`RatchetPolicy::max_superseded_age`].
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
        let (plaintext, index, _) = crate::token::trial_decrypt(
            recipient,
            self.epochs.iter().map(|epoch| &epoch.secret),
            token,
        )?;
        Ok((plaintext, self.epochs[index].id))
    }

    /// Encode the complete logical state as a versioned binary snapshot signed by `identity`,
    /// the destination's owner (`Destination.py` 211-226).
    ///
    /// Derived public keys and ids are omitted and re-derived on restore. Entries are
    /// encoded newest first as `created_at(f64 LE) || private_key(32)`, and the Ed25519
    /// signature over everything before it closes the snapshot.
    pub fn encode_snapshot(&self, identity: &PrivateIdentity) -> Vec<u8> {
        let count = u32::try_from(self.epochs.len())
            .expect("validated policy bounds the epoch count to u32");
        let mut out = Vec::with_capacity(
            SNAPSHOT_HEADER_LEN + self.epochs.len() * SNAPSHOT_ENTRY_LEN + SIGNATURE_LEN,
        );
        out.extend_from_slice(SNAPSHOT_MAGIC);
        out.push(SNAPSHOT_VERSION);
        out.extend_from_slice(&count.to_le_bytes());
        for epoch in &self.epochs {
            out.extend_from_slice(&epoch.created_at.to_le_bytes());
            out.extend_from_slice(&epoch.secret);
        }
        let signature = identity.sign(&out);
        out.extend_from_slice(&signature);
        out
    }

    /// Verify `identity`'s signature, then restore atomically under `policy`
    /// (`Destination.py` 437-475).
    pub fn restore(
        policy: RatchetPolicy,
        snapshot: &[u8],
        identity: &Identity,
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
        let (body, signature) = snapshot
            .split_last_chunk::<SIGNATURE_LEN>()
            .filter(|(body, _)| body.len() >= SNAPSHOT_HEADER_LEN)
            .ok_or(RatchetError::InvalidSnapshot)?;
        if !identity.verify(body, signature) {
            return Err(RatchetError::InvalidSignature);
        }
        let count = u32::from_le_bytes(
            body[5..9]
                .try_into()
                .map_err(|_| RatchetError::InvalidSnapshot)?,
        ) as usize;
        if count > MAX_SNAPSHOT_EPOCHS
            || body.len() != SNAPSHOT_HEADER_LEN + count * SNAPSHOT_ENTRY_LEN
        {
            return Err(RatchetError::InvalidSnapshot);
        }

        let mut epochs = Vec::with_capacity(count);
        let mut previous_created_at = f64::INFINITY;
        for raw in body[SNAPSHOT_HEADER_LEN..]
            .as_chunks::<SNAPSHOT_ENTRY_LEN>()
            .0
        {
            let (created_at, secret) = raw.split_first_chunk::<8>().expect("entry length");
            let created_at = f64::from_le_bytes(*created_at);
            validate_timestamp(created_at).map_err(|_| RatchetError::InvalidSnapshot)?;
            if created_at > previous_created_at {
                return Err(RatchetError::InvalidSnapshot);
            }
            previous_created_at = created_at;
            let secret = secret.try_into().expect("entry length");
            epochs.push(RatchetEpoch::new(secret, created_at));
        }

        let mut store = Self { policy, epochs };
        let expired = store.prune_at(now);
        let evicted = store.len().saturating_sub(store.policy.max_count);
        store.epochs.truncate(store.policy.max_count);
        let receipt = RatchetRestoreReceipt {
            loaded: store.len(),
            expired,
            evicted,
        };
        Ok((store, receipt))
    }

    /// Drop the superseded suffix whose successor was created more than the policy's
    /// superseded age before `now`. Creation times only decrease along the list, so the
    /// expired epochs are always a suffix.
    fn prune_at(&mut self, now: f64) -> usize {
        let Some(max_age) = self.policy.max_superseded_age else {
            return 0;
        };
        let max_age = max_age.as_secs_f64();
        let keep = self
            .epochs
            .windows(2)
            .position(|pair| now - pair[0].created_at > max_age)
            .map_or(self.epochs.len(), |successor| successor + 1);
        let expired = self.epochs.len() - keep;
        self.epochs.truncate(keep);
        expired
    }
}

fn validate_policy(policy: &RatchetPolicy) -> Result<(), RatchetError> {
    if policy.max_count == 0
        || policy.max_count > MAX_SNAPSHOT_EPOCHS
        || policy.rotation_interval.is_zero()
        || policy.max_superseded_age.is_some_and(|age| age.is_zero())
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

    fn policy(max_count: usize, rotation: u64, superseded: Option<u64>) -> RatchetPolicy {
        RatchetPolicy {
            max_count,
            rotation_interval: Duration::from_secs(rotation),
            max_superseded_age: superseded.map(Duration::from_secs),
        }
    }

    fn owner() -> PrivateIdentity {
        PrivateIdentity::from_secret_bytes(&[0x5A; 64])
    }

    #[test]
    fn rotation_waits_out_the_interval_and_retains_by_count() {
        let mut store = RatchetStore::new(policy(2, 10, None)).unwrap();
        let first = store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        assert!(first.rotated);
        assert_eq!(store.len(), 1);

        // RNS rotates only once `now > latest + interval`.
        for now in [9.0, 10.0] {
            let retained = store.rotate_if_due([2; KEY_LEN], now).unwrap();
            assert!(!retained.rotated);
            assert_eq!(retained.current, first.current);
        }

        assert!(store.rotate_if_due([2; KEY_LEN], 10.5).unwrap().rotated);
        let third = store.rotate_if_due([3; KEY_LEN], 21.0).unwrap();
        assert!(third.rotated);
        assert_eq!(third.evicted, 1);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn a_long_lived_current_epoch_is_never_expired_by_age() {
        // The old policy dropped every epoch 30 days after creation, including the one the
        // destination was still advertising. Retention is by count only by default.
        let mut store = RatchetStore::new(RatchetPolicy::default()).unwrap();
        store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        let year = 365.0 * 24.0 * 3600.0;
        assert_eq!(store.prune(year).unwrap(), 0);
        assert_eq!(store.len(), 1);
        assert!(store.rotate_if_due([2; KEY_LEN], year).unwrap().rotated);
        assert_eq!(store.prune(10.0 * year).unwrap(), 0);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn superseded_age_counts_from_the_successor_not_from_creation() {
        let mut store = RatchetStore::new(policy(8, 10, Some(25))).unwrap();
        store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        store.rotate_if_due([2; KEY_LEN], 100.0).unwrap();
        store.rotate_if_due([3; KEY_LEN], 120.0).unwrap();
        // Epoch 1 was superseded at 100, epoch 2 at 120; epoch 3 is current.
        assert_eq!(store.prune(125.0).unwrap(), 0);
        assert_eq!(store.prune(126.0).unwrap(), 1);
        assert_eq!(store.len(), 2);
        assert_eq!(store.prune(1_000.0).unwrap(), 1);
        assert_eq!(store.len(), 1, "the current epoch survives any age");
    }

    #[test]
    fn decrypt_reports_the_cached_id_of_the_epoch_that_authenticated() {
        let recipient = owner();
        let mut store = RatchetStore::new(policy(4, 10, None)).unwrap();
        store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        let older = store.current_id().unwrap();
        let older_public = store.current_public().unwrap();
        store.rotate_if_due([2; KEY_LEN], 11.0).unwrap();
        assert_ne!(store.current_id(), Some(older));

        let token = crate::token::encrypt_to_ratchet(
            recipient.public(),
            &older_public,
            &[7; KEY_LEN],
            &[8; crate::token::IV_LEN],
            b"retained",
        );
        let (plaintext, id) = store.decrypt(&recipient, &token).unwrap();
        assert_eq!(plaintext, b"retained");
        assert_eq!(id, older);
        assert_eq!(id, NameHash::of(&older_public));
    }

    #[test]
    fn signed_snapshot_round_trips_and_reapplies_policy() {
        let owner = owner();
        let mut original = RatchetStore::new(policy(3, 10, None)).unwrap();
        original.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        original.rotate_if_due([2; KEY_LEN], 11.0).unwrap();
        original.rotate_if_due([3; KEY_LEN], 22.0).unwrap();
        let snapshot = original.encode_snapshot(&owner);

        let (restored, receipt) =
            RatchetStore::restore(policy(2, 10, None), &snapshot, owner.public(), 30.0).unwrap();
        assert_eq!(
            receipt,
            RatchetRestoreReceipt {
                loaded: 2,
                expired: 0,
                evicted: 1,
            }
        );
        assert_eq!(restored.current_id(), original.current_id());
        assert_eq!(restored.current_public(), original.current_public());

        let (aged, receipt) =
            RatchetStore::restore(policy(3, 10, Some(10)), &snapshot, owner.public(), 30.0)
                .unwrap();
        assert_eq!(
            receipt,
            RatchetRestoreReceipt {
                loaded: 2,
                expired: 1,
                evicted: 0,
            }
        );
        assert_eq!(aged.current_id(), original.current_id());
    }

    #[test]
    fn tampered_or_foreign_snapshots_are_rejected() {
        let owner = owner();
        let mut store = RatchetStore::new(policy(2, 10, None)).unwrap();
        store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        let snapshot = store.encode_snapshot(&owner);
        let restore = |bytes: &[u8], identity: &Identity| {
            RatchetStore::restore(policy(2, 10, None), bytes, identity, 10.0).map(|_| ())
        };
        assert_eq!(restore(&snapshot, owner.public()), Ok(()));

        // A substituted private key would let whoever planted it read our traffic.
        let mut swapped = snapshot.clone();
        swapped[SNAPSHOT_HEADER_LEN + 8] ^= 1;
        assert_eq!(
            restore(&swapped, owner.public()),
            Err(RatchetError::InvalidSignature)
        );

        let mut forged_signature = snapshot.clone();
        *forged_signature.last_mut().unwrap() ^= 1;
        assert_eq!(
            restore(&forged_signature, owner.public()),
            Err(RatchetError::InvalidSignature)
        );

        let stranger = PrivateIdentity::from_secret_bytes(&[0xA5; 64]);
        assert_eq!(
            restore(&snapshot, stranger.public()),
            Err(RatchetError::InvalidSignature)
        );
        assert_eq!(
            restore(&store.encode_snapshot(&stranger), owner.public()),
            Err(RatchetError::InvalidSignature)
        );
    }

    #[test]
    fn malformed_unsigned_or_reordered_snapshots_are_rejected_atomically() {
        let owner = owner();
        let mut store = RatchetStore::new(policy(2, 10, None)).unwrap();
        store.rotate_if_due([1; KEY_LEN], 0.0).unwrap();
        store.rotate_if_due([2; KEY_LEN], 11.0).unwrap();
        let snapshot = store.encode_snapshot(&owner);
        let restore = |bytes: &[u8]| {
            RatchetStore::restore(policy(2, 10, None), bytes, owner.public(), 11.0).map(|_| ())
        };

        assert_eq!(restore(&snapshot[..8]), Err(RatchetError::InvalidSnapshot));
        assert_eq!(
            restore(&snapshot[..SNAPSHOT_HEADER_LEN + SIGNATURE_LEN - 1]),
            Err(RatchetError::InvalidSnapshot)
        );

        // The unauthenticated version 1 layout is refused, not silently trusted.
        let mut v1 = snapshot[..snapshot.len() - SIGNATURE_LEN].to_vec();
        v1[4] = 1;
        assert_eq!(
            restore(&v1),
            Err(RatchetError::UnsupportedSnapshotVersion(1))
        );

        // A correctly signed but out-of-order body is still refused.
        let mut body = snapshot[..snapshot.len() - SIGNATURE_LEN].to_vec();
        body[SNAPSHOT_HEADER_LEN..].rotate_left(SNAPSHOT_ENTRY_LEN);
        let signature = owner.sign(&body);
        body.extend_from_slice(&signature);
        assert_eq!(restore(&body), Err(RatchetError::InvalidSnapshot));
    }

    #[test]
    fn debug_output_never_contains_private_key_bytes() {
        let mut store = RatchetStore::new(policy(2, 10, None)).unwrap();
        store.rotate_if_due([0xA5; KEY_LEN], 0.0).unwrap();
        let debug = format!("{store:?}");
        assert!(!debug.contains(&"a5".repeat(KEY_LEN)));
    }
}
