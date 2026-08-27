//! Announces: building them, and validating them.
//!
//! # Layout
//!
//! The announce payload, in wire order:
//!
//! ```text
//! x25519_pub(32) || ed25519_pub(32) || name_hash(10) || rand_hash(10)
//!                || [ratchet(32)]   || signature(64) || app_data(*)
//! ```
//!
//! The ratchet is present exactly when the packet's context flag (header bit 5) is set.
//!
//! # The signed message is not the payload
//!
//! This is the trap in the whole protocol, and it has two halves. The Ed25519 signature
//! covers:
//!
//! ```text
//! dest_hash(16) || x25519_pub(32) || ed25519_pub(32) || name_hash(10) || rand_hash(10)
//!               || [ratchet(32)]  || app_data(*)
//! ```
//!
//! So: the destination hash is prepended, and it *is not in the payload at all* (it lives
//! in the packet header); and the signature itself is spliced out, which moves `app_data`.
//! Put the other way round, the signed message is the wire payload with the header's
//! destination hash on the front and the signature cut out of the middle.
//!
//! Verified against RNS 1.3.8 by independent Ed25519 verification across all four
//! combinations of {ratchet, no ratchet} x {app_data, no app_data}, and by six negative
//! fixtures in which a single flipped byte (including one in the *header*) must and does
//! fail validation.

use alloc::vec::Vec;
use core::result::Result as CoreResult;

use crate::destination::destination_hash;
use crate::hash::{AddressHash, NameHash};
use crate::identity::{IDENTITY_LEN, Identity, KEY_LEN, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use crate::{Error, Result};

/// Length of the random hash carried in an announce.
pub const RAND_HASH_LEN: usize = 10;

/// Bytes of per-announce unpredictable nonce material in an announce blob.
pub const ANNOUNCE_NONCE_LEN: usize = 5;

/// Largest value that fits in the announce blob's 40-bit timebase field.
pub const ANNOUNCE_TIMEBASE_MAX: u64 = (1_u64 << 40) - 1;

/// Why a timebase value or its ordinal state cannot be used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimebaseError {
    /// A value cannot be represented in the wire's 40-bit field.
    OutOfRange,
    /// The last emitted value is above the reserved ceiling.
    InvalidBounds,
    /// The next strictly newer value would exceed the reserved ceiling.
    Exhausted,
}

/// The ten-byte nonce-and-timebase field carried by an announce.
///
/// [`Self::from_wire`] preserves bytes decoded from an existing packet. It does not imply
/// that the sender minted a fresh value. [`Self::mint`] is for emission: it accepts the
/// nonce and checked whole-second timebase separately, then writes the exact wire layout
/// `nonce[5] || timebase_be[5]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AnnounceBlob([u8; RAND_HASH_LEN]);

impl AnnounceBlob {
    /// Preserve a blob decoded from the wire exactly as received.
    pub const fn from_wire(bytes: [u8; RAND_HASH_LEN]) -> Self {
        Self(bytes)
    }

    /// Mint a blob from a five-byte nonce and a checked 40-bit whole-second timebase.
    pub fn mint(nonce: [u8; ANNOUNCE_NONCE_LEN], timebase: u64) -> CoreResult<Self, TimebaseError> {
        if timebase > ANNOUNCE_TIMEBASE_MAX {
            return Err(TimebaseError::OutOfRange);
        }
        let mut bytes = [0_u8; RAND_HASH_LEN];
        bytes[..ANNOUNCE_NONCE_LEN].copy_from_slice(&nonce);
        bytes[ANNOUNCE_NONCE_LEN..].copy_from_slice(&timebase.to_be_bytes()[3..]);
        Ok(Self(bytes))
    }

    /// The nonce half, in its exact wire order.
    pub const fn nonce(&self) -> [u8; ANNOUNCE_NONCE_LEN] {
        [self.0[0], self.0[1], self.0[2], self.0[3], self.0[4]]
    }

    /// The big-endian, 40-bit whole-second timebase half.
    pub const fn timebase(&self) -> u64 {
        ((self.0[ANNOUNCE_NONCE_LEN] as u64) << 32)
            | ((self.0[ANNOUNCE_NONCE_LEN + 1] as u64) << 24)
            | ((self.0[ANNOUNCE_NONCE_LEN + 2] as u64) << 16)
            | ((self.0[ANNOUNCE_NONCE_LEN + 3] as u64) << 8)
            | self.0[ANNOUNCE_NONCE_LEN + 4] as u64
    }

    /// The exact ten bytes to pass to existing announce construction code.
    pub const fn into_bytes(self) -> [u8; RAND_HASH_LEN] {
        self.0
    }

    /// Borrow the exact ten wire bytes.
    pub const fn as_bytes(&self) -> &[u8; RAND_HASH_LEN] {
        &self.0
    }
}

/// A pure ordinal generator for an announcing identity's whole-second timebase.
///
/// The caller owns clock acquisition, durable storage, and reservation policy. A desktop
/// process can reserve through [`ANNOUNCE_TIMEBASE_MAX`] with [`Self::host`]. On firmware
/// reboot, `last_emitted` must be the prior durable reservation ceiling, not a checkpoint of
/// the last actual emission. Persist a higher ceiling first, then construct
/// [`Self::firmware_lease`] from that floor and new lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TimebaseGenerator {
    last_emitted: u64,
    reserved_through: u64,
}

impl TimebaseGenerator {
    /// Initialize from the last emitted value and the inclusive durable reservation ceiling.
    pub const fn new(last_emitted: u64, reserved_through: u64) -> CoreResult<Self, TimebaseError> {
        if last_emitted > ANNOUNCE_TIMEBASE_MAX || reserved_through > ANNOUNCE_TIMEBASE_MAX {
            return Err(TimebaseError::OutOfRange);
        }
        if last_emitted > reserved_through {
            return Err(TimebaseError::InvalidBounds);
        }
        Ok(Self {
            last_emitted,
            reserved_through,
        })
    }

    /// A host generator whose reservation extends to the wire's maximum value.
    pub const fn host(last_emitted: u64) -> CoreResult<Self, TimebaseError> {
        Self::new(last_emitted, ANNOUNCE_TIMEBASE_MAX)
    }

    /// A firmware generator constrained to an already durable inclusive lease.
    pub const fn firmware_lease(
        last_emitted: u64,
        reserved_through: u64,
    ) -> CoreResult<Self, TimebaseError> {
        Self::new(last_emitted, reserved_through)
    }

    /// The most recent emitted ordinal.
    pub const fn last_emitted(&self) -> u64 {
        self.last_emitted
    }

    /// The inclusive ceiling this generator may emit without another reservation.
    pub const fn reserved_through(&self) -> u64 {
        self.reserved_through
    }

    /// Produce a strictly newer ordinal from a whole-second source.
    ///
    /// The next value is `max(source_seconds, last_emitted + 1)`. It never silently wraps and
    /// never emits beyond `reserved_through`.
    pub fn next(&mut self, source_seconds: u64) -> CoreResult<u64, TimebaseError> {
        if source_seconds > ANNOUNCE_TIMEBASE_MAX {
            return Err(TimebaseError::OutOfRange);
        }
        let after_last = self
            .last_emitted
            .checked_add(1)
            .ok_or(TimebaseError::Exhausted)?;
        let next = source_seconds.max(after_last);
        if next > self.reserved_through {
            return Err(TimebaseError::Exhausted);
        }
        self.last_emitted = next;
        Ok(next)
    }
}

/// Length of a ratchet public key. An X25519 public key.
pub const RATCHET_LEN: usize = 32;

/// Smallest valid announce payload: both keys, both hashes, a signature, no app data.
pub const MIN_PAYLOAD_LEN: usize =
    IDENTITY_LEN + crate::hash::NAME_HASH_LEN + RAND_HASH_LEN + SIGNATURE_LEN;

/// A validated announce.
///
/// There is no way to construct one whose signature has not been checked: [`Announce::decode`]
/// verifies before it returns. An `Announce` in hand is an announce that verified.
#[derive(Clone, Debug)]
pub struct Announce {
    /// The announcing peer's identity, recovered from the payload.
    pub identity: Identity,
    /// The destination hash, taken from the packet header.
    pub destination: AddressHash,
    pub name_hash: NameHash,
    pub rand_hash: [u8; RAND_HASH_LEN],
    /// The peer's current ratchet public key, if it advertised one.
    pub ratchet: Option<[u8; RATCHET_LEN]>,
    pub app_data: Vec<u8>,
}

impl Announce {
    /// Assemble the message the signature covers.
    ///
    /// Shared by build and validate so the two cannot drift apart. If they ever disagree,
    /// every announce we emit is rejected by everyone, silently.
    fn signed_message(
        destination: AddressHash,
        identity_public: &[u8; IDENTITY_LEN],
        name_hash: NameHash,
        rand_hash: &[u8; RAND_HASH_LEN],
        ratchet: Option<&[u8; RATCHET_LEN]>,
        app_data: &[u8],
    ) -> Vec<u8> {
        let mut msg = Vec::with_capacity(
            crate::hash::ADDRESS_HASH_LEN
                + IDENTITY_LEN
                + crate::hash::NAME_HASH_LEN
                + RAND_HASH_LEN
                + ratchet.map_or(0, |_| RATCHET_LEN)
                + app_data.len(),
        );
        msg.extend_from_slice(destination.as_slice());
        msg.extend_from_slice(identity_public);
        msg.extend_from_slice(name_hash.as_slice());
        msg.extend_from_slice(rand_hash);
        if let Some(r) = ratchet {
            msg.extend_from_slice(r);
        }
        msg.extend_from_slice(app_data);
        msg
    }

    /// Decode and validate an announce packet.
    ///
    /// Returns [`Error::BadSignature`] if the signature does not check out, which is the
    /// only thing standing between us and a peer that announces someone else's identity.
    pub fn decode(packet: &Packet) -> Result<Self> {
        if packet.packet_type != PacketType::Announce {
            return Err(Error::NotAnAnnounce);
        }

        let ratcheted = packet.context_flag;
        let want = MIN_PAYLOAD_LEN + if ratcheted { RATCHET_LEN } else { 0 };
        if packet.payload.len() < want {
            return Err(Error::Truncated);
        }
        let p = &packet.payload;

        let public: [u8; IDENTITY_LEN] = p[..IDENTITY_LEN].try_into().expect("checked length");
        let identity = Identity::from_public_bytes(&public)?;

        let mut off = IDENTITY_LEN;
        let name_hash = NameHash::from_slice(&p[off..]).ok_or(Error::Truncated)?;
        off += crate::hash::NAME_HASH_LEN;

        let rand_hash: [u8; RAND_HASH_LEN] = p[off..off + RAND_HASH_LEN]
            .try_into()
            .expect("checked length");
        off += RAND_HASH_LEN;

        let ratchet = if ratcheted {
            let r: [u8; RATCHET_LEN] = p[off..off + RATCHET_LEN]
                .try_into()
                .expect("checked length");
            off += RATCHET_LEN;
            Some(r)
        } else {
            None
        };

        let signature: [u8; SIGNATURE_LEN] = p[off..off + SIGNATURE_LEN]
            .try_into()
            .expect("checked length");
        off += SIGNATURE_LEN;

        let app_data = p[off..].to_vec();

        // The destination hash comes from the header, and it is part of the signed
        // message, so a peer cannot replay one destination's announce under another.
        let destination = packet.destination;

        let message = Self::signed_message(
            destination,
            &public,
            name_hash,
            &rand_hash,
            ratchet.as_ref(),
            &app_data,
        );
        if !identity.verify(&message, &signature) {
            return Err(Error::BadSignature);
        }

        // The destination hash must actually be the one this identity and name imply.
        // Without this a valid signature over an unrelated destination hash would pass.
        if destination_hash(name_hash, identity.hash()) != destination {
            return Err(Error::DestinationMismatch);
        }

        Ok(Self {
            identity,
            destination,
            name_hash,
            rand_hash,
            ratchet,
            app_data,
        })
    }

    /// The ratchet id a peer would use to refer to this announce's ratchet.
    ///
    /// `trunc10(SHA256(ratchet_public_key))`. Verified against RNS 1.3.8's
    /// `current_ratchet_id`.
    pub fn ratchet_id(&self) -> Option<NameHash> {
        self.ratchet.map(|r| NameHash::of(&r))
    }
}

/// Build a signed announce packet.
///
/// `rand_hash` is supplied by the caller rather than generated here, which keeps this
/// module free of any RNG and lets fixtures be reproduced byte for byte. The runtime layer
/// is responsible for producing a fresh random one per announce.
pub fn build(
    identity: &PrivateIdentity,
    name_hash: NameHash,
    rand_hash: &[u8; RAND_HASH_LEN],
    ratchet: Option<&[u8; RATCHET_LEN]>,
    app_data: &[u8],
) -> Packet {
    let public = identity.public().to_public_bytes();
    let destination = destination_hash(name_hash, identity.hash());

    let message = Announce::signed_message(
        destination,
        &public,
        name_hash,
        rand_hash,
        ratchet,
        app_data,
    );
    let signature = identity.sign(&message);

    let mut payload = Vec::with_capacity(
        IDENTITY_LEN
            + crate::hash::NAME_HASH_LEN
            + RAND_HASH_LEN
            + ratchet.map_or(0, |_| RATCHET_LEN)
            + SIGNATURE_LEN
            + app_data.len(),
    );
    payload.extend_from_slice(&public);
    payload.extend_from_slice(name_hash.as_slice());
    payload.extend_from_slice(rand_hash);
    if let Some(r) = ratchet {
        payload.extend_from_slice(r);
    }
    payload.extend_from_slice(&signature);
    payload.extend_from_slice(app_data);

    Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: ratchet.is_some(),
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Announce,
        hops: 0,
        transport: None,
        destination,
        context: 0,
        payload,
    }
}

// Silence an unused-import warning when the crate is built without the token module's
// consumers; KEY_LEN documents that a ratchet is an X25519 public key.
const _: () = assert!(RATCHET_LEN == KEY_LEN);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minted_blob_writes_nonce_then_big_endian_timebase() {
        let blob = AnnounceBlob::mint([0xa1, 0xb2, 0xc3, 0xd4, 0xe5], 0x01_02_03_04_05)
            .expect("40-bit timebase");

        assert_eq!(
            blob.into_bytes(),
            [0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0x01, 0x02, 0x03, 0x04, 0x05]
        );
    }

    #[test]
    fn ordinal_advances_within_one_source_second() {
        let mut generator = TimebaseGenerator::host(41).expect("valid host ceiling");

        assert_eq!(generator.next(42), Ok(42));
        assert_eq!(generator.next(42), Ok(43));
        assert_eq!(generator.last_emitted(), 43);
    }

    #[test]
    fn ordinal_does_not_regress_when_source_clock_moves_back() {
        let mut generator = TimebaseGenerator::host(100).expect("valid host ceiling");

        assert_eq!(generator.next(3), Ok(101));
        assert_eq!(generator.last_emitted(), 101);
    }

    #[test]
    fn firmware_lease_refuses_an_unreserved_ordinal() {
        let mut generator = TimebaseGenerator::firmware_lease(10, 11).expect("valid lease");

        assert_eq!(generator.next(0), Ok(11));
        assert_eq!(generator.next(0), Err(TimebaseError::Exhausted));
        assert_eq!(generator.last_emitted(), 11);
    }

    #[test]
    fn forty_bit_maximum_is_the_last_mintable_and_emittable_value() {
        let blob = AnnounceBlob::mint([0; ANNOUNCE_NONCE_LEN], ANNOUNCE_TIMEBASE_MAX)
            .expect("40-bit maximum fits");
        assert_eq!(blob.timebase(), ANNOUNCE_TIMEBASE_MAX);
        assert_eq!(
            AnnounceBlob::mint([0; ANNOUNCE_NONCE_LEN], ANNOUNCE_TIMEBASE_MAX + 1),
            Err(TimebaseError::OutOfRange)
        );

        let mut generator = TimebaseGenerator::host(ANNOUNCE_TIMEBASE_MAX)
            .expect("maximum is a valid restored value");
        assert_eq!(
            generator.next(ANNOUNCE_TIMEBASE_MAX),
            Err(TimebaseError::Exhausted)
        );
    }

    #[test]
    fn decoded_fixture_blob_round_trips_without_reminting() {
        let wire = [0xf2, 0xd0, 0x91, 0xf8, 0x87, 0x00, 0x6a, 0x55, 0xa7, 0x8a];
        let decoded = AnnounceBlob::from_wire(wire);

        assert_eq!(decoded.nonce(), [0xf2, 0xd0, 0x91, 0xf8, 0x87]);
        assert_eq!(decoded.timebase(), 0x00_6a_55_a7_8a);
        assert_eq!(decoded.into_bytes(), wire);
    }
}
