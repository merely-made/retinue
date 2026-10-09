//! Opening a link (initiator) and accepting one (responder).

use alloc::vec::Vec;

use x25519_dalek::PublicKey as XPublicKey;

use super::{
    CTX_LRPROOF, LINK_KEYS_LEN, LINK_PROOF_LEN, LINK_REQUEST_LEN, Link, LinkMode, LinkTrailer,
    TRAILER_LEN, link_id, request_trailer,
};
use crate::hash::AddressHash;
use crate::identity::{Identity, KEY_LEN, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use crate::token::DerivedKeys;
use crate::{Error, Result};

/// Bytes of a link proof before its trailer: signature and public key.
const LINK_PROOF_KEYS_LEN: usize = LINK_PROOF_LEN - TRAILER_LEN;

/// An outbound link the initiator has requested but the peer has not yet proved.
///
/// Holds the ephemeral secret so the shared key can be derived once the proof arrives. A
/// `PendingLink` becomes a [`Link`] only through [`prove`](PendingLink::prove), which
/// verifies the peer's signature first.
pub struct PendingLink {
    ephemeral: PrivateIdentity,
    peer: Identity,
    link_id: AddressHash,
    requested: LinkTrailer,
}

impl PendingLink {
    /// Start a link to `peer` at `destination`.
    ///
    /// `ephemeral_seed` is a fresh `x25519_secret(32) || ed25519_seed(32)` per attempt; the
    /// caller supplies the randomness so the core stays RNG-free. Returns the pending link
    /// and the request packet to send.
    pub fn open(
        destination: AddressHash,
        peer: Identity,
        ephemeral_seed: &[u8; 64],
        requested: LinkTrailer,
    ) -> (Self, Packet) {
        let ephemeral = PrivateIdentity::from_secret_bytes(ephemeral_seed);

        let mut payload = Vec::with_capacity(LINK_REQUEST_LEN);
        payload.extend_from_slice(&ephemeral.public().to_public_bytes());
        payload.extend_from_slice(&requested.encode());

        let request = Packet {
            ifac: false,
            header_type: HeaderType::Type1,
            context_flag: false,
            propagation: Propagation::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::LinkRequest,
            hops: 0,
            transport: None,
            destination,
            context: 0,
            payload,
        };

        let link_id = link_id(&request).expect("request we just built has 64+ payload bytes");
        (
            Self {
                ephemeral,
                peer,
                link_id,
                requested,
            },
            request,
        )
    }

    /// The link id this attempt will have. Inbound proofs are addressed to it.
    pub fn link_id(&self) -> AddressHash {
        self.link_id
    }

    /// Validate a proof and, if it checks out, produce the established [`Link`].
    ///
    /// The proof is `signature(64) || peer_ephemeral_x25519(32) || trailer(3)`. The
    /// signature covers `link_id || peer_ephemeral_x25519 || peer_identity_ed25519 ||
    /// trailer`, which binds the ephemeral key to the destination's long-term identity, so
    /// a third party cannot substitute its own ephemeral key. Verified against RNS 1.3.8.
    pub fn prove(&self, proof: &Packet) -> Result<Link> {
        if proof.packet_type != PacketType::Proof {
            return Err(Error::NotAProof);
        }
        if proof.destination != self.link_id {
            return Err(Error::LinkMismatch);
        }
        // Exactly 96 or 99 bytes, signalling the mode we asked for (`Link.py` 396-405).
        let mode = match proof.payload.len() {
            LINK_PROOF_KEYS_LEN => LinkMode::DEFAULT,
            LINK_PROOF_LEN => {
                LinkTrailer::decode(
                    proof.payload[LINK_PROOF_KEYS_LEN..]
                        .try_into()
                        .expect("len"),
                )?
                .mode
            }
            n if n < LINK_PROOF_KEYS_LEN => return Err(Error::Truncated),
            _ => return Err(Error::NotAProof),
        };
        if mode != self.requested.mode {
            return Err(Error::BadLinkMode);
        }
        #[cfg(test)]
        crate::probe::hit(crate::probe::Probe::LinkProve);

        let signature: [u8; SIGNATURE_LEN] = proof.payload[..SIGNATURE_LEN]
            .try_into()
            .expect("checked length");
        let peer_eph: [u8; KEY_LEN] = proof.payload[SIGNATURE_LEN..SIGNATURE_LEN + KEY_LEN]
            .try_into()
            .expect("checked length");
        let trailer_bytes = &proof.payload[LINK_PROOF_KEYS_LEN..];

        // The signed message ends with the trailer when the proof carries one.
        let mut signed = Vec::with_capacity(
            crate::hash::ADDRESS_HASH_LEN + KEY_LEN + KEY_LEN + trailer_bytes.len(),
        );
        signed.extend_from_slice(self.link_id.as_slice());
        signed.extend_from_slice(&peer_eph);
        signed.extend_from_slice(self.peer.ed25519_bytes());
        signed.extend_from_slice(trailer_bytes);

        if !self.peer.verify(&signed, &signature) {
            return Err(Error::BadSignature);
        }

        // The proof's trailer is authoritative for the negotiated mode and MTU. Without one
        // RNS takes `Reticulum.MTU` (`Link.py` 422). The path only ever lowers the MTU, so
        // either is held to our request.
        let agreed = if trailer_bytes.len() == TRAILER_LEN {
            LinkTrailer::decode(trailer_bytes.try_into().expect("len"))?
        } else {
            LinkTrailer {
                mtu: crate::packet::MTU as u32,
                ..self.requested
            }
        };
        let agreed = LinkTrailer {
            mtu: agreed.mtu.min(self.requested.mtu),
            ..agreed
        };

        let shared = self.ephemeral.diffie_hellman(&XPublicKey::from(peer_eph));
        let keys = DerivedKeys::derive(&shared, self.link_id);

        // The initiator proves link data with the ephemeral Ed25519 key it put in the
        // request, and the destination proves with its identity key, which is what the
        // proof above was just verified against.
        Ok(Link {
            id: self.link_id,
            keys,
            mode: agreed.mode,
            mtu: agreed.mtu,
            signer: signing_seed(&self.ephemeral),
            peer_signer: *self.peer.ed25519_bytes(),
            resource_carry: None,
        })
    }
}

/// Accept an inbound link request and produce the proof.
///
/// This is the responder mirror of [`PendingLink`]. The destination signs the proof with
/// its long-term identity (so the initiator, which learned that identity from an announce,
/// can bind the ephemeral key to it), and contributes a fresh ephemeral X25519 key for the
/// exchange. Returns the established [`Link`] and the proof packet to send back.
///
/// `ephemeral_seed` is a fresh 64-byte keypair seed supplied by the caller; only its
/// X25519 half is used here. `offered` is the mode and MTU to advertise, capped by the
/// caller against the request if it wants to honour the initiator's proposal. Its mode must
/// be the one the request signals, which must be enabled ([`LinkMode::is_enabled`]); a
/// request that is not exactly 64 or 67 bytes is refused.
pub fn accept(
    request: &Packet,
    destination: &PrivateIdentity,
    ephemeral_seed: &[u8; 64],
    offered: LinkTrailer,
) -> Result<(Link, Packet)> {
    if request.packet_type != PacketType::LinkRequest {
        return Err(Error::NotALinkRequest);
    }
    // Exactly 64 or 67 bytes (`Link.py` 187), in an enabled mode that the proof will echo
    // (`Link.py` 149, 225-227, 367).
    let requested = match request.payload.len() {
        LINK_KEYS_LEN => LinkMode::DEFAULT,
        LINK_REQUEST_LEN => {
            request_trailer(request)?
                .expect("67 bytes carry a trailer")
                .mode
        }
        n if n < LINK_KEYS_LEN => return Err(Error::Truncated),
        _ => return Err(Error::NotALinkRequest),
    };
    if !requested.is_enabled() || offered.mode != requested {
        return Err(Error::BadLinkMode);
    }

    let id = link_id(request)?;
    let peer_eph_x: [u8; KEY_LEN] = request.payload[..KEY_LEN]
        .try_into()
        .expect("checked length");
    // The initiator's ephemeral Ed25519 key, which signs its link-data proofs. Taking it
    // from the request is what lets us validate those proofs without an IDENTIFY.
    let peer_signer: [u8; KEY_LEN] = request.payload[KEY_LEN..LINK_KEYS_LEN]
        .try_into()
        .expect("checked length");

    let ephemeral = PrivateIdentity::from_secret_bytes(ephemeral_seed);
    let our_eph_x = *ephemeral.public().x25519_bytes();
    let trailer = offered.encode();

    // Sign link_id || our_eph_x || our_long_term_ed25519 || trailer with the destination's
    // identity, exactly as the initiator will reconstruct and verify it.
    let mut signed =
        Vec::with_capacity(crate::hash::ADDRESS_HASH_LEN + KEY_LEN + KEY_LEN + TRAILER_LEN);
    signed.extend_from_slice(id.as_slice());
    signed.extend_from_slice(&our_eph_x);
    signed.extend_from_slice(destination.public().ed25519_bytes());
    signed.extend_from_slice(&trailer);
    let signature = destination.sign(&signed);

    let mut payload = Vec::with_capacity(LINK_PROOF_LEN);
    payload.extend_from_slice(&signature);
    payload.extend_from_slice(&our_eph_x);
    payload.extend_from_slice(&trailer);

    let proof = Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Proof,
        hops: 0,
        transport: None,
        destination: id,
        context: CTX_LRPROOF,
        payload,
    };

    let shared = ephemeral.diffie_hellman(&XPublicKey::from(peer_eph_x));
    let keys = DerivedKeys::derive(&shared, id);

    Ok((
        Link {
            id,
            keys,
            mode: offered.mode,
            mtu: offered.mtu,
            signer: signing_seed(destination),
            peer_signer,
            resource_carry: None,
        },
        proof,
    ))
}

/// The Ed25519 seed half of an identity's 64-byte secret.
pub(super) fn signing_seed(identity: &PrivateIdentity) -> [u8; KEY_LEN] {
    identity.to_secret_bytes()[KEY_LEN..]
        .try_into()
        .expect("the Ed25519 half of 64 bytes is 32")
}
