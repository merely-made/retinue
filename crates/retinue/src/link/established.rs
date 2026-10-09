//! An established link and the classification of its inbound traffic.

use alloc::vec;
use alloc::vec::Vec;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};

use super::proof::{explicit_proof_packet, identify_signed_message, split_data_proof};
use super::wire::link_packet;
use super::{
    CTX_KEEPALIVE, CTX_LINKCLOSE, CTX_LINKIDENTIFY, CTX_LRRTT, CTX_REQUEST, CTX_RESOURCE_PRF,
    CTX_RESPONSE, KEEPALIVE_REQUEST, KEEPALIVE_RESPONSE, LINK_IDENTIFY_LEN, LinkMode,
    data_proof_packet, read_data_proof,
};
use crate::Result;
use crate::hash::AddressHash;
use crate::identity::{IDENTITY_LEN, Identity, KEY_LEN, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use crate::token::{DerivedKeys, IV_LEN};

/// What an inbound link-layer packet is, once matched to a link by its id.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    /// Application data, already decrypted.
    Data(Vec<u8>),
    /// The RTT packet that follows a proof. [`crate::link_liveness::read_rtt`] reads it.
    Rtt,
    /// A keepalive request. Answer it with [`Link::keepalive_packet`] carrying
    /// [`KEEPALIVE_RESPONSE`].
    KeepAliveRequest,
    /// A keepalive response to one we sent.
    KeepAliveResponse,
    /// The peer tore the link down.
    Close,
    /// A request, already decrypted. The payload is RNS's msgpack-packed request.
    Request(Vec<u8>),
    /// A response, already decrypted. The payload is RNS's msgpack-packed response.
    Response(Vec<u8>),
    /// Addressed to this link but not a shape we recognise.
    Unknown,
}

/// An established link: a shared key and the negotiated parameters.
///
/// Data rides the R0 token under the link's static key; forward secrecy lives in the
/// ephemeral exchange that formed the link, so links carry no ratchet. As in RNS, the
/// initiator proves link data with its request's ephemeral Ed25519 key and the responder with
/// its identity; both are kept as 32-byte seeds (64 bytes a link, not several hundred).
/// `Clone` is cheap and lets a stream hold a sealing handle to the link the router reads.
#[derive(Clone)]
pub struct Link {
    pub(super) id: AddressHash,
    pub(super) keys: DerivedKeys,
    pub(super) mode: LinkMode,
    pub(super) mtu: u32,
    /// Ed25519 seed this side proves link data with.
    pub(super) signer: [u8; KEY_LEN],
    /// Ed25519 public key the peer proves link data with. Not checked to be a valid point
    /// when the link forms: a bad one only means the peer's proofs never verify.
    pub(super) peer_signer: [u8; KEY_LEN],
}

impl Link {
    pub fn id(&self) -> AddressHash {
        self.id
    }

    pub fn mode(&self) -> LinkMode {
        self.mode
    }

    pub fn mtu(&self) -> u32 {
        self.mtu
    }

    /// Encrypt application bytes into a link data packet.
    ///
    /// `iv` is caller-supplied to keep this reproducible; it must be fresh and
    /// unpredictable per packet in production.
    pub fn data_packet(&self, plaintext: &[u8], iv: &[u8; IV_LEN]) -> Packet {
        link_packet(0x00, self.id, self.keys.encrypt(plaintext, iv))
    }

    /// Decrypt a link data packet's payload.
    pub fn decrypt(&self, packet: &Packet) -> Result<Vec<u8>> {
        self.keys.decrypt(&packet.payload)
    }

    /// Seal a whole blob with the link keys into one token (`IV || ciphertext || HMAC`).
    ///
    /// Resources encrypt the compressed payload as a single token, then split *that* into
    /// parts, so the resource layer needs blob crypto rather than per-packet crypto.
    pub fn seal(&self, plaintext: &[u8], iv: &[u8; IV_LEN]) -> Vec<u8> {
        self.keys.encrypt(plaintext, iv)
    }

    /// Open a whole blob sealed with [`seal`](Self::seal).
    pub fn open(&self, token: &[u8]) -> Result<Vec<u8>> {
        self.keys.decrypt(token)
    }

    /// A link packet with an arbitrary context and an already-encrypted payload.
    ///
    /// Resource parts carry raw slices of a pre-sealed token, so they are not encrypted
    /// again; the advertisement/request/proof, which are sealed, pass their token here too.
    pub fn framed_packet(&self, context: u8, payload: Vec<u8>) -> Packet {
        link_packet(context, self.id, payload)
    }

    /// A link packet whose plaintext is sealed with the link keys under `context`.
    pub fn sealed_packet(&self, context: u8, plaintext: &[u8], iv: &[u8; IV_LEN]) -> Packet {
        link_packet(context, self.id, self.keys.encrypt(plaintext, iv))
    }

    /// A resource proof packet: a `Proof`-type packet on this link, context
    /// `RESOURCE_PRF`, payload `resource_hash(32) || proof(32)`, sent unencrypted. This is
    /// the shape RNS 1.3.8 accepts to conclude a resource transfer (verified live).
    pub fn resource_proof_packet(&self, resource_hash: &[u8; 32], proof: &[u8; 32]) -> Packet {
        let mut payload = Vec::with_capacity(64);
        payload.extend_from_slice(resource_hash);
        payload.extend_from_slice(proof);
        Packet {
            ifac: false,
            header_type: HeaderType::Type1,
            context_flag: false,
            propagation: Propagation::Broadcast,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Proof,
            hops: 0,
            transport: None,
            destination: self.id,
            context: CTX_RESOURCE_PRF,
            payload,
        }
    }

    /// Build a link-data **proof** for a received proof-requesting packet — the ack a
    /// [`Channel`](crate::channel::Channel) treats as delivery. Signs `proven`'s full
    /// 32-byte hash with this side's link signing key and wraps it in the explicit proof
    /// [`data_proof_packet`] addressed to this link: the ephemeral key from the request
    /// for an initiator, the destination identity for a responder. That is the key an RNS
    /// peer validates against; see [`validate_proof`](Self::validate_proof).
    pub fn prove_packet(&self, proven: &Packet) -> Packet {
        let hash = proven.full_hash();
        let signature = SigningKey::from_bytes(&self.signer).sign(&hash).to_bytes();
        explicit_proof_packet(self.id, &hash, &signature)
    }

    /// Validate the peer's link-data proof against its link signing key (the initiator's
    /// ephemeral key, or the destination's identity), returning the full hash of the packet
    /// it acknowledges, or `None` if it is not a well-formed, correctly-signed proof for this
    /// link. The inverse of [`prove_packet`](Self::prove_packet).
    pub fn validate_proof(&self, proof: &Packet) -> Option<[u8; 32]> {
        let (full_hash, signature) = split_data_proof(self.id, proof)?;
        let key = VerifyingKey::from_bytes(&self.peer_signer).ok()?;
        key.verify_strict(&full_hash, &Signature::from_bytes(&signature))
            .is_ok()
            .then_some(full_hash)
    }

    /// Build a link-data proof signed by an explicit `prover` rather than this side's link
    /// signing key. RNS never does this; [`prove_packet`](Self::prove_packet) is the
    /// interoperable form.
    pub fn data_proof(&self, proven: &Packet, prover: &PrivateIdentity) -> Packet {
        data_proof_packet(self.id, &proven.full_hash(), prover)
    }

    /// Validate an inbound link-data proof against an explicit `peer` identity rather than
    /// the peer's link signing key. Returns the proven hash as
    /// [`validate_proof`](Self::validate_proof) does. Kept for proofs from older retinue
    /// initiators, which signed with their IDENTIFY'd long-term identity.
    pub fn verify_data_proof(&self, proof: &Packet, peer: &Identity) -> Option<[u8; 32]> {
        read_data_proof(self.id, proof, peer)
    }

    /// Build a link **IDENTIFY** packet: sealed `public_key(64) || Ed25519_sign(link_id ||
    /// public_key)(64)` under context [`CTX_LINKIDENTIFY`]. An initiator sends this so the
    /// responder learns its identity. It does not change the link's proof keys. The
    /// signature binds the identity to *this* link, so an identify captured on one link cannot
    /// be replayed on another. RNS 1.3.8's exact wire (captured in `link_identify.json`).
    pub fn identify_packet(&self, me: &PrivateIdentity, iv: &[u8; IV_LEN]) -> Packet {
        let public = me.public().to_public_bytes();
        let signature = me.sign(&identify_signed_message(self.id, &public));
        let mut plaintext = Vec::with_capacity(LINK_IDENTIFY_LEN);
        plaintext.extend_from_slice(&public);
        plaintext.extend_from_slice(&signature);
        self.sealed_packet(CTX_LINKIDENTIFY, &plaintext, iv)
    }

    /// Validate an inbound IDENTIFY packet on this link, returning the peer [`Identity`] it
    /// proves, or `None` if it does not decrypt, is malformed, or the signature does not
    /// verify. The inverse of [`identify_packet`](Self::identify_packet).
    pub fn read_identify(&self, packet: &Packet) -> Option<Identity> {
        let plaintext = self.decrypt(packet).ok()?;
        if plaintext.len() != LINK_IDENTIFY_LEN {
            return None;
        }
        let public: [u8; IDENTITY_LEN] = plaintext[..IDENTITY_LEN].try_into().ok()?;
        let signature: [u8; SIGNATURE_LEN] = plaintext[IDENTITY_LEN..].try_into().ok()?;
        let peer = Identity::from_public_bytes(&public).ok()?;
        peer.verify(&identify_signed_message(self.id, &public), &signature)
            .then_some(peer)
    }

    /// Classify an inbound packet addressed to this link.
    ///
    /// Returns `None` if the packet is not for this link at all. Otherwise it dispatches on
    /// the context byte: data and RTT are decrypted, keepalives and closes are recognised
    /// by their shape.
    pub fn receive(&self, packet: &Packet) -> Option<Inbound> {
        if packet.destination != self.id {
            return None;
        }
        Some(match packet.context {
            0x00 => match self.decrypt(packet) {
                Ok(data) => Inbound::Data(data),
                Err(_) => Inbound::Unknown,
            },
            CTX_LRRTT => Inbound::Rtt,
            CTX_REQUEST => match self.decrypt(packet) {
                Ok(data) => Inbound::Request(data),
                Err(_) => Inbound::Unknown,
            },
            CTX_RESPONSE => match self.decrypt(packet) {
                Ok(data) => Inbound::Response(data),
                Err(_) => Inbound::Unknown,
            },
            CTX_KEEPALIVE => match packet.payload.first().copied() {
                Some(KEEPALIVE_REQUEST) => Inbound::KeepAliveRequest,
                Some(KEEPALIVE_RESPONSE) => Inbound::KeepAliveResponse,
                _ => Inbound::Unknown,
            },
            CTX_LINKCLOSE => match self.decrypt(packet) {
                Ok(data) if data == self.id.as_slice() => Inbound::Close,
                _ => Inbound::Unknown,
            },
            _ => Inbound::Unknown,
        })
    }

    /// The RTT packet the initiator sends after a proof, which moves the link to active on
    /// the peer. Use a MessagePack float64: some peers do not activate on float32.
    pub fn rtt_packet(&self, rtt_seconds: f32, iv: &[u8; IV_LEN]) -> Packet {
        let mut plain = Vec::with_capacity(9);
        plain.push(0xcb);
        plain.extend_from_slice(&f64::from(rtt_seconds).to_be_bytes());
        link_packet(CTX_LRRTT, self.id, self.keys.encrypt(&plain, iv))
    }

    /// Encrypt an already-packed request into a link request packet (context `0x09`).
    ///
    /// The `packed` bytes are RNS's msgpack request structure; retinue does not impose one,
    /// so a caller can carry whatever the peer expects. See [`crate::request`].
    pub fn request_packet(&self, packed: &[u8], iv: &[u8; IV_LEN]) -> Packet {
        link_packet(CTX_REQUEST, self.id, self.keys.encrypt(packed, iv))
    }

    /// Encrypt an already-packed response into a link response packet (context `0x0a`).
    pub fn response_packet(&self, packed: &[u8], iv: &[u8; IV_LEN]) -> Packet {
        link_packet(CTX_RESPONSE, self.id, self.keys.encrypt(packed, iv))
    }

    /// A keepalive. The peer answers a [`KEEPALIVE_REQUEST`] with a [`KEEPALIVE_RESPONSE`].
    /// Keepalive bytes are not encrypted; they ride the link by its id alone.
    pub fn keepalive_packet(&self, sentinel: u8) -> Packet {
        link_packet(CTX_KEEPALIVE, self.id, vec![sentinel])
    }

    /// Tear the link down.
    ///
    /// The payload is the link id sealed under the link keys. RNS rejects an
    /// unsealed 16-byte id as a truncated token.
    pub fn close_packet(&self, iv: &[u8; IV_LEN]) -> Packet {
        link_packet(
            CTX_LINKCLOSE,
            self.id,
            self.keys.encrypt(self.id.as_slice(), iv),
        )
    }
}
