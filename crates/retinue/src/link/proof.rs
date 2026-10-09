//! Explicit link-data proofs and the IDENTIFY signed message.

use alloc::vec::Vec;

use crate::hash::{ADDRESS_HASH_LEN, AddressHash};
use crate::identity::{IDENTITY_LEN, Identity, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};

/// Bytes of an explicit link-data proof payload: `full_hash(32) || signature(64)`. This is
/// RNS 1.3.8's `PacketReceipt.EXPL_LENGTH` (96); the implicit 64-byte form is not used for
/// link data, where the proof must carry the hash to name the packet it acknowledges.
pub const DATA_PROOF_LEN: usize = 32 + SIGNATURE_LEN;

/// Bytes of a link IDENTIFY payload (sealed): `public_key(64) || signature(64)`.
pub const LINK_IDENTIFY_LEN: usize = IDENTITY_LEN + SIGNATURE_LEN;

/// The message an IDENTIFY signs: `link_id(16) || public_key(64)`. Binding the identity to
/// the link id stops an identify from one link being replayed on another.
pub(super) fn identify_signed_message(
    link_id: AddressHash,
    public: &[u8; IDENTITY_LEN],
) -> Vec<u8> {
    let mut signed = Vec::with_capacity(ADDRESS_HASH_LEN + IDENTITY_LEN);
    signed.extend_from_slice(link_id.as_slice());
    signed.extend_from_slice(public);
    signed
}

/// Build the explicit link-data proof packet: a `Proof`-type packet addressed to
/// `link_id`, context `0x00`, payload `proven_full_hash(32) || Ed25519_sign(prover,
/// proven_full_hash)(64)`, sent unencrypted. This is RNS 1.3.8's link-data proof exactly
/// (captured in `rns_link_proof.json`): the ack that concludes a proof-requesting packet.
/// The proof is addressed to the link, not the packet hash, so it carries the hash inside
/// to say which packet it proves — the sender matches that to an outstanding sequence.
pub fn data_proof_packet(
    link_id: AddressHash,
    proven_full_hash: &[u8; 32],
    prover: &PrivateIdentity,
) -> Packet {
    explicit_proof_packet(link_id, proven_full_hash, &prover.sign(proven_full_hash))
}

/// The explicit proof wire, whichever key made `signature`.
pub(super) fn explicit_proof_packet(
    link_id: AddressHash,
    proven_full_hash: &[u8; 32],
    signature: &[u8; SIGNATURE_LEN],
) -> Packet {
    let mut payload = Vec::with_capacity(DATA_PROOF_LEN);
    payload.extend_from_slice(proven_full_hash);
    payload.extend_from_slice(signature);
    Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Proof,
        hops: 0,
        transport: None,
        destination: link_id,
        context: 0x00,
        payload,
    }
}

/// Validate an explicit link-data proof for `link_id` against `peer`'s identity, returning
/// the proven packet's full 32-byte hash if the proof is well-formed and correctly signed,
/// else `None`. The inverse of [`data_proof_packet`].
pub fn read_data_proof(link_id: AddressHash, proof: &Packet, peer: &Identity) -> Option<[u8; 32]> {
    let (full_hash, signature) = split_data_proof(link_id, proof)?;
    peer.verify(&full_hash, &signature).then_some(full_hash)
}

/// The proven hash and signature of a well-formed explicit proof for `link_id`, unverified.
pub(super) fn split_data_proof(
    link_id: AddressHash,
    proof: &Packet,
) -> Option<([u8; 32], [u8; SIGNATURE_LEN])> {
    if proof.packet_type != PacketType::Proof
        || proof.destination != link_id
        || proof.payload.len() != DATA_PROOF_LEN
    {
        return None;
    }
    let full_hash: [u8; 32] = proof.payload[..32].try_into().ok()?;
    let signature: [u8; SIGNATURE_LEN] = proof.payload[32..].try_into().ok()?;
    Some((full_hash, signature))
}
