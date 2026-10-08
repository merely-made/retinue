//! Delivery proofs for link-less single packets.
//!
//! A destination proves a packet by signing its full 32-byte packet hash with its identity
//! key, and sends the signature back as a PROOF packet addressed to the packet's truncated
//! hash (RNS `Identity.prove`, `Packet.ProofDestination`). The proof is implicit (the
//! signature alone) unless the prover chooses explicit (`hash || signature`). A sender
//! validates both forms against the destination's identity (`PacketReceipt.validate_proof`).
//! A transport node carries the proof back the way the packet came, by the same truncated
//! hash.

use alloc::vec::Vec;

use crate::hash::{AddressHash, HASH_LEN};
use crate::identity::{Identity, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};

/// Payload length of an implicit proof: the signature alone (RNS `IMPL_LENGTH`).
pub const IMPLICIT_PROOF_LEN: usize = SIGNATURE_LEN;

/// Payload length of an explicit proof: `packet_hash || signature` (RNS `EXPL_LENGTH`).
pub const EXPLICIT_PROOF_LEN: usize = HASH_LEN + SIGNATURE_LEN;

/// Build the proof that `prover` received the packet whose full hash is `packet_hash`.
pub fn proof_packet(
    prover: &PrivateIdentity,
    packet_hash: &[u8; HASH_LEN],
    implicit: bool,
) -> Packet {
    let signature = prover.sign(packet_hash);
    let mut payload = Vec::with_capacity(EXPLICIT_PROOF_LEN);
    if !implicit {
        payload.extend_from_slice(packet_hash);
    }
    payload.extend_from_slice(&signature);
    Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Single,
        packet_type: PacketType::Proof,
        hops: 0,
        transport: None,
        destination: truncated(packet_hash),
        context: 0,
        payload,
    }
}

/// Whether `proof` (a proof packet's payload, in either form) proves `packet_hash` was
/// received by `identity`.
pub fn validate(proof: &[u8], packet_hash: &[u8; HASH_LEN], identity: &Identity) -> bool {
    let signature = match proof.len() {
        EXPLICIT_PROOF_LEN if proof[..HASH_LEN] == packet_hash[..] => &proof[HASH_LEN..],
        IMPLICIT_PROOF_LEN => proof,
        _ => return false,
    };
    <&[u8; SIGNATURE_LEN]>::try_from(signature)
        .is_ok_and(|signature| identity.verify(packet_hash, signature))
}

/// The truncated packet hash a proof is addressed to.
pub fn truncated(packet_hash: &[u8; HASH_LEN]) -> AddressHash {
    AddressHash::from_slice(&packet_hash[..crate::hash::ADDRESS_HASH_LEN])
        .expect("a full hash is longer than an address hash")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn received() -> (PrivateIdentity, Packet) {
        let prover = PrivateIdentity::from_secret_bytes(&[0x21; 64]);
        let packet = Packet {
            ifac: false,
            header_type: HeaderType::Type1,
            context_flag: false,
            propagation: Propagation::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 3,
            transport: None,
            destination: AddressHash::from_bytes([0x42; 16]),
            context: 0,
            payload: alloc::vec![7; 80],
        };
        (prover, packet)
    }

    #[test]
    fn both_forms_are_addressed_to_the_truncated_hash_and_validate() {
        let (prover, packet) = received();
        let hash = packet.full_hash();
        for implicit in [true, false] {
            let proof = proof_packet(&prover, &hash, implicit);
            assert_eq!(proof.destination, packet.hash());
            assert_eq!(proof.packet_type, PacketType::Proof);
            assert_eq!(
                proof.payload.len(),
                if implicit {
                    IMPLICIT_PROOF_LEN
                } else {
                    EXPLICIT_PROOF_LEN
                }
            );
            assert!(validate(&proof.payload, &hash, prover.public()));
        }
    }

    #[test]
    fn a_proof_from_another_identity_or_for_another_packet_fails() {
        let (prover, packet) = received();
        let hash = packet.full_hash();
        let stranger = PrivateIdentity::from_secret_bytes(&[0x22; 64]);
        let mut other = hash;
        other[0] ^= 1;
        for implicit in [true, false] {
            let proof = proof_packet(&prover, &hash, implicit);
            assert!(!validate(&proof.payload, &hash, stranger.public()));
            assert!(!validate(&proof.payload, &other, prover.public()));
            assert!(!validate(&proof.payload[1..], &hash, prover.public()));
        }
    }
}
