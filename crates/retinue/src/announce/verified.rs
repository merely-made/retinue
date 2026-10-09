//! Announce work that can be skipped: the freshness fields, read before any signature
//! check, and a small cache of announces that already verified.

use heapless::Vec as BoundedVec;

use super::{Announce, AnnounceBlob, MIN_PAYLOAD_LEN, RAND_HASH_LEN, RATCHET_LEN};
use crate::Result;
use crate::announce_freshness::AnnounceFreshnessCandidate;
use crate::hash::NAME_HASH_LEN;
use crate::identity::IDENTITY_LEN;
use crate::packet::{Packet, PacketType};

/// The freshness candidate of an announce packet, read without verifying it, or `None`
/// when [`Announce::decode`] would refuse its shape anyway.
///
/// A freshness rejection needs no signature: the verified announce carries the same
/// destination and blob and is rejected the same way. An acceptance still needs the decode.
pub(crate) fn unverified_candidate(packet: &Packet) -> Option<AnnounceFreshnessCandidate> {
    let want = MIN_PAYLOAD_LEN + if packet.context_flag { RATCHET_LEN } else { 0 };
    if packet.packet_type != PacketType::Announce || packet.payload.len() < want {
        return None;
    }
    let at = IDENTITY_LEN + NAME_HASH_LEN;
    let blob: [u8; RAND_HASH_LEN] = packet.payload[at..at + RAND_HASH_LEN]
        .try_into()
        .expect("checked length");
    Some(AnnounceFreshnessCandidate {
        destination: packet.destination,
        blob: AnnounceBlob::from_wire(blob),
    })
}

/// The full packet hash and the context flag of an announce that verified.
///
/// The packet hash masks the flag byte to its low nibble, which leaves out the context flag,
/// and the flag decides whether a ratchet is parsed: the same hash with the flag flipped is
/// a different announce, and must verify on its own.
type Key = ([u8; 32], bool);

/// The `N` announces that most recently verified, most recent first. A copy of one, relayed
/// by another neighbour or heard on another interface, decodes without a second signature
/// check.
pub(crate) struct VerifiedAnnounces<const N: usize> {
    keys: BoundedVec<Key, N>,
}

impl<const N: usize> VerifiedAnnounces<N> {
    pub(crate) const fn new() -> Self {
        Self {
            keys: BoundedVec::new(),
        }
    }

    /// [`Announce::decode`], skipping the signature for bytes that already verified.
    pub(crate) fn decode(&mut self, packet: &Packet) -> Result<Announce> {
        let key = (packet.full_hash(), packet.context_flag);
        if let Some(index) = self.keys.iter().position(|k| *k == key) {
            let key = self.keys.remove(index);
            let _ = self.keys.insert(0, key);
            return Announce::decode_checked(packet, false);
        }
        let announce = Announce::decode(packet)?;
        if self.keys.is_full() {
            self.keys.pop();
        }
        let _ = self.keys.insert(0, key);
        Ok(announce)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::announce::build;
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;
    use crate::probe::{Probe, take};

    fn announce(seed: u8, ratchet: Option<&[u8; RATCHET_LEN]>) -> Packet {
        let identity = PrivateIdentity::from_secret_bytes(&[seed; 64]);
        let name = DestinationName::new("retinue", ["cache"]).name_hash();
        build(
            &identity,
            name,
            &AnnounceBlob::from_wire([seed; RAND_HASH_LEN]),
            ratchet,
            b"app",
        )
    }

    #[test]
    fn a_copy_skips_the_signature_check() {
        let mut cache = VerifiedAnnounces::<2>::new();
        let original = announce(1, None);
        let mut relayed = original.clone();
        relayed.hops = 3;
        relayed.header_type = crate::packet::HeaderType::Type2;
        relayed.transport = Some(crate::AddressHash::from_bytes([9; 16]));

        take(Probe::AnnounceVerify);
        let first = cache.decode(&original).unwrap();
        let copy = cache.decode(&relayed).unwrap();
        assert_eq!(take(Probe::AnnounceVerify), 1);
        assert_eq!(copy.destination, first.destination);
        assert_eq!(copy.app_data, first.app_data);
    }

    #[test]
    fn a_flipped_context_flag_is_not_a_hit() {
        let mut cache = VerifiedAnnounces::<2>::new();
        let ratcheted = announce(1, Some(&[7; RATCHET_LEN]));
        cache.decode(&ratcheted).unwrap();
        let mut flipped = ratcheted.clone();
        flipped.context_flag = false;
        assert_eq!(flipped.full_hash(), ratcheted.full_hash());

        take(Probe::AnnounceVerify);
        assert!(cache.decode(&flipped).is_err());
        assert_eq!(
            take(Probe::AnnounceVerify),
            1,
            "the flipped copy is verified"
        );
    }

    #[test]
    fn a_failure_is_not_remembered_and_the_oldest_gives_way() {
        let mut cache = VerifiedAnnounces::<2>::new();
        let mut forged = announce(1, None);
        let last = forged.payload.len() - 1;
        forged.payload[last] ^= 1;
        assert!(cache.decode(&forged).is_err());
        assert!(cache.decode(&forged).is_err());

        let (a, b, c) = (announce(1, None), announce(2, None), announce(3, None));
        for packet in [&a, &b, &a, &c] {
            cache.decode(packet).unwrap();
        }
        take(Probe::AnnounceVerify);
        cache.decode(&a).unwrap();
        cache.decode(&c).unwrap();
        assert_eq!(
            take(Probe::AnnounceVerify),
            0,
            "a was used more recently than b"
        );
        cache.decode(&b).unwrap();
        assert_eq!(take(Probe::AnnounceVerify), 1);
    }

    #[test]
    fn the_candidate_is_read_from_the_raw_payload() {
        let packet = announce(4, Some(&[7; RATCHET_LEN]));
        let verified = Announce::decode(&packet).unwrap();
        let candidate = unverified_candidate(&packet).unwrap();
        assert_eq!(candidate.destination, verified.destination);
        assert_eq!(candidate.blob.into_bytes(), verified.rand_hash);

        let mut short = packet.clone();
        short.payload.truncate(MIN_PAYLOAD_LEN);
        assert!(unverified_candidate(&short).is_none());
    }
}
