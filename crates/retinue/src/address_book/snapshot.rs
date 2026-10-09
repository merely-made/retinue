//! The address book's persisted form.
//!
//! RNS keeps known destinations and received ratchets on disk across restarts
//! (`Identity.py` 177-240, 410-443, 484-508), so a restarted node can reach peers without
//! hearing them announce again. This is that state as one snapshot, signed by the owning
//! identity like [`RatchetStore`](crate::ratchet::RatchetStore)'s, so a host's storage cannot
//! plant a key for a destination. The host supplies the storage.

use alloc::vec::Vec;

use core::fmt;

use super::{AddressBook, Peer};
use crate::announce::RATCHET_LEN;
use crate::hash::{ADDRESS_HASH_LEN, AddressHash, NAME_HASH_LEN, NameHash};
use crate::identity::{IDENTITY_LEN, Identity, PrivateIdentity, SIGNATURE_LEN};

const MAGIC: &[u8; 4] = b"RTAB";
const VERSION: u8 = 1;
const HEADER_LEN: usize = MAGIC.len() + 1 + 4;
const FLAG_RETAINED: u8 = 0x01;
const FLAG_RATCHET: u8 = 0x02;

/// Why a snapshot was refused. A refused snapshot changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    Malformed,
    UnsupportedVersion(u8),
    /// Not signed by the expected identity: tampered with, or another node's.
    InvalidSignature,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed => f.write_str("malformed address book snapshot"),
            Self::UnsupportedVersion(v) => write!(f, "unsupported address book snapshot {v}"),
            Self::InvalidSignature => f.write_str("invalid address book snapshot signature"),
        }
    }
}

impl core::error::Error for SnapshotError {}

/// What [`AddressBook::restore`] did with a verified snapshot's peers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RestoreReceipt {
    pub loaded: usize,
    /// Already in the book, whose live entry was kept.
    pub known: usize,
    /// Left out because the book was full.
    pub refused: usize,
}

impl AddressBook {
    /// Encode every peer as a versioned snapshot signed by `owner`, the node holding the book.
    ///
    /// `magic | version | count u32le | entry* | signature`, each entry being
    /// `destination | public key | name hash | announces_seen, last_heard, last_used u64le |
    /// flags | [ratchet | ratchet_received u64le] | app_data len u16le | app_data`, in
    /// ascending destination order.
    pub fn encode_snapshot(&self, owner: &PrivateIdentity) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.push(VERSION);
        out.extend_from_slice(&(self.peers.len() as u32).to_le_bytes());
        for (destination, peer) in &self.peers {
            out.extend_from_slice(destination.as_slice());
            out.extend_from_slice(&peer.identity.to_public_bytes());
            out.extend_from_slice(peer.name_hash.as_slice());
            for tick in [peer.announces_seen, peer.last_heard, peer.last_used] {
                out.extend_from_slice(&tick.to_le_bytes());
            }
            let mut flags = 0;
            if peer.retained {
                flags |= FLAG_RETAINED;
            }
            if peer.ratchet.is_some() {
                flags |= FLAG_RATCHET;
            }
            out.push(flags);
            if let Some(ratchet) = &peer.ratchet {
                out.extend_from_slice(ratchet);
                out.extend_from_slice(&peer.ratchet_received.to_le_bytes());
            }
            // Announce app data fits one packet, so its length always fits.
            out.extend_from_slice(&(peer.app_data.len() as u16).to_le_bytes());
            out.extend_from_slice(&peer.app_data);
        }
        let signature = owner.sign(&out);
        out.extend_from_slice(&signature);
        out
    }

    /// Verify `owner`'s signature over `snapshot`, then add its peers to this book. A peer
    /// the book already holds keeps its live entry, and peers past capacity are left out.
    /// Nothing changes unless the whole snapshot verifies and parses.
    pub fn restore(
        &mut self,
        snapshot: &[u8],
        owner: &Identity,
    ) -> Result<RestoreReceipt, SnapshotError> {
        let peers = decode(snapshot, owner)?;
        let mut receipt = RestoreReceipt::default();
        for (destination, peer) in peers {
            if self.knows(destination) {
                receipt.known += 1;
            } else if self.is_full() {
                receipt.refused += 1;
            } else {
                self.peers.insert(destination, peer);
                receipt.loaded += 1;
            }
        }
        Ok(receipt)
    }
}

fn decode(snapshot: &[u8], owner: &Identity) -> Result<Vec<(AddressHash, Peer)>, SnapshotError> {
    if snapshot.len() < HEADER_LEN || &snapshot[..MAGIC.len()] != MAGIC {
        return Err(SnapshotError::Malformed);
    }
    if snapshot[4] != VERSION {
        return Err(SnapshotError::UnsupportedVersion(snapshot[4]));
    }
    let (body, signature) = snapshot
        .split_last_chunk::<SIGNATURE_LEN>()
        .filter(|(body, _)| body.len() >= HEADER_LEN)
        .ok_or(SnapshotError::Malformed)?;
    if !owner.verify(body, signature) {
        return Err(SnapshotError::InvalidSignature);
    }
    let mut reader = Reader(&body[HEADER_LEN..]);
    let count = u32::from_le_bytes(*body[5..].first_chunk().ok_or(SnapshotError::Malformed)?);
    let mut peers = Vec::new();
    for _ in 0..count {
        let destination = AddressHash::from_bytes(reader.array::<ADDRESS_HASH_LEN>()?);
        // Ascending order, so no destination appears twice.
        if peers.last().is_some_and(|(last, _)| *last >= destination) {
            return Err(SnapshotError::Malformed);
        }
        let identity = Identity::from_public_bytes(&reader.array::<IDENTITY_LEN>()?)
            .map_err(|_| SnapshotError::Malformed)?;
        let name_hash = NameHash::from_bytes(reader.array::<NAME_HASH_LEN>()?);
        let [announces_seen, last_heard, last_used] = [reader.u64()?, reader.u64()?, reader.u64()?];
        let [flags] = reader.array::<1>()?;
        if flags & !(FLAG_RETAINED | FLAG_RATCHET) != 0 {
            return Err(SnapshotError::Malformed);
        }
        let (ratchet, ratchet_received) = if flags & FLAG_RATCHET != 0 {
            (Some(reader.array::<RATCHET_LEN>()?), reader.u64()?)
        } else {
            (None, 0)
        };
        let app_data_len = u16::from_le_bytes(reader.array::<2>()?) as usize;
        let app_data = reader.take(app_data_len)?.to_vec();
        peers.push((
            destination,
            Peer {
                identity,
                name_hash,
                app_data,
                ratchet,
                ratchet_received,
                announces_seen,
                last_heard,
                last_used,
                retained: flags & FLAG_RETAINED != 0,
            },
        ));
    }
    if !reader.0.is_empty() {
        return Err(SnapshotError::Malformed);
    }
    Ok(peers)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8], SnapshotError> {
        if self.0.len() < len {
            return Err(SnapshotError::Malformed);
        }
        let (head, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], SnapshotError> {
        Ok(self.take(N)?.try_into().expect("took N bytes"))
    }

    fn u64(&mut self) -> Result<u64, SnapshotError> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::announce_at;
    use super::*;

    fn owner() -> PrivateIdentity {
        PrivateIdentity::from_secret_bytes(&[0x3C; 64])
    }

    fn book() -> (AddressBook, [AddressHash; 2]) {
        let mut book = AddressBook::new();
        let plain = announce_at(&mut book, 1, None, 40);
        let ratcheted = announce_at(&mut book, 2, Some([9; RATCHET_LEN]), 50);
        book.mark_used(plain, 60);
        book.set_retained(ratcheted, true, 60);
        (book, [plain, ratcheted])
    }

    #[test]
    fn a_snapshot_restores_every_peer_and_its_ratchet() {
        let (book, destinations) = book();
        let snapshot = book.encode_snapshot(&owner());
        let mut restored = AddressBook::new();
        let receipt = restored.restore(&snapshot, owner().public()).unwrap();
        assert_eq!(receipt.loaded, 2);
        for destination in destinations {
            let (a, b) = (
                book.resolve(destination).unwrap(),
                restored.resolve(destination).unwrap(),
            );
            assert_eq!(a.identity, b.identity);
            assert_eq!(
                (a.name_hash, &a.app_data, a.ratchet, a.ratchet_received),
                (b.name_hash, &b.app_data, b.ratchet, b.ratchet_received)
            );
            assert_eq!(
                (a.announces_seen, a.last_heard, a.last_used, a.retained),
                (b.announces_seen, b.last_heard, b.last_used, b.retained)
            );
        }
        // Re-encoding the restored book gives the same bytes.
        assert_eq!(restored.encode_snapshot(&owner()), snapshot);
    }

    #[test]
    fn a_tampered_or_foreign_snapshot_changes_nothing() {
        let (book, _) = book();
        let snapshot = book.encode_snapshot(&owner());
        let mut target = AddressBook::new();
        for at in [9, snapshot.len() / 2, snapshot.len() - 1] {
            let mut tampered = snapshot.clone();
            tampered[at] ^= 0x01;
            assert_eq!(
                target.restore(&tampered, owner().public()),
                Err(SnapshotError::InvalidSignature)
            );
        }
        let stranger = PrivateIdentity::from_secret_bytes(&[0x3D; 64]);
        assert_eq!(
            target.restore(&snapshot, stranger.public()),
            Err(SnapshotError::InvalidSignature)
        );
        let mut future = snapshot.clone();
        future[4] = VERSION + 1;
        assert_eq!(
            target.restore(&future, owner().public()),
            Err(SnapshotError::UnsupportedVersion(VERSION + 1))
        );
        assert_eq!(
            target.restore(&snapshot[..8], owner().public()),
            Err(SnapshotError::Malformed)
        );
        assert!(target.is_empty());
    }

    #[test]
    fn a_signed_but_inconsistent_body_is_refused() {
        let (book, _) = book();
        let snapshot = book.encode_snapshot(&owner());
        let body = &snapshot[..snapshot.len() - SIGNATURE_LEN];
        let resign = |body: &[u8]| {
            let mut out = body.to_vec();
            out.extend_from_slice(&owner().sign(body));
            out
        };
        // A count past the entries present, and a trailing byte after them.
        let mut short = body.to_vec();
        short[5] = 3;
        let mut long = body.to_vec();
        long.push(0);
        let mut target = AddressBook::new();
        for body in [short, long] {
            assert_eq!(
                target.restore(&resign(&body), owner().public()),
                Err(SnapshotError::Malformed)
            );
        }
        assert!(target.is_empty());
    }

    #[test]
    fn a_restore_keeps_live_entries_and_respects_capacity() {
        let (book, [plain, ratcheted]) = book();
        let snapshot = book.encode_snapshot(&owner());
        let mut target = AddressBook::with_max_peers(1);
        announce_at(&mut target, 1, None, 900);
        let receipt = target.restore(&snapshot, owner().public()).unwrap();
        assert_eq!(
            receipt,
            RestoreReceipt {
                loaded: 0,
                known: 1,
                refused: 1
            }
        );
        assert_eq!(target.resolve(plain).unwrap().last_heard, 900);
        assert!(!target.knows(ratcheted));
    }
}
