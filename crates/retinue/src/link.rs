//! Link primitives: the mode/MTU trailer, and the link id.
//!
//! This is not yet a link implementation (that is R3). It is the two facts that a link
//! implementation cannot be written without, both of which were settled by capturing a
//! real handshake, because a wrong guess on either means no link ever completes and there
//! is no useful error to debug.
//!
//! # The trailer
//!
//! An RNS 1.x link request carries **67** bytes, not 64: the two ephemeral public keys and
//! then a 3-byte trailer. A link proof carries **99**, not 96: signature, public key, and
//! the same trailer. The trailer is a 24-bit big-endian field:
//!
//! ```text
//! bits 23..21  the AES mode   (0 = AES-128-CBC, 1 = AES-256-CBC)
//! bits 20..0   the MTU
//! ```
//!
//! Observed: an initiator sends `20 20 00` = mode 1, MTU 8192. The responder answers
//! `20 01 f4` = mode 1, MTU 500, which is exactly `Reticulum.MTU`. So this is an MTU
//! negotiation, and the mode is AES-256 on both sides.
//!
//! Beechat sends a bare 64-byte request and does not participate in any of this.
//!
//! # The link id
//!
//! The link id is a truncated hash over the link request, and the details are unobvious:
//!
//! ```text
//! link_id = trunc16(SHA256( (flags & 0x0F) || destination(16) || context(1) || payload[..64] ))
//! ```
//!
//! Two things to note. `hops` is excluded, which makes sense: it mutates in transit. And
//! the payload is **truncated to the 64 bytes of keys**, so the trailer deliberately does
//! not affect the link id, which is also sensible because the trailer is negotiable.
//!
//! Derived by solving against two independently captured (request, link id) pairs; only
//! this formula satisfies both. See `oracle/capture_link.py`.

// Needed by the test build or the tokio shell; the bare no_std lib does not reach it.
#[allow(unused_imports)]
use alloc::string::ToString;

use alloc::vec;
use alloc::vec::Vec;

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use x25519_dalek::PublicKey as XPublicKey;

use crate::hash::{ADDRESS_HASH_LEN, AddressHash};
use crate::identity::{IDENTITY_LEN, Identity, KEY_LEN, PrivateIdentity, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
use crate::token::{DerivedKeys, IV_LEN};
use crate::{Error, Result};

/// Length of the mode/MTU trailer on link requests and proofs.
pub const TRAILER_LEN: usize = 3;

/// Bytes of key material in a link request: two 32-byte public keys.
pub const LINK_KEYS_LEN: usize = 64;

/// Bytes of a link request: the keys plus the trailer.
pub const LINK_REQUEST_LEN: usize = LINK_KEYS_LEN + TRAILER_LEN;

/// Bytes of a link proof: signature (64), public key (32), and the trailer.
pub const LINK_PROOF_LEN: usize = SIGNATURE_LEN + KEY_LEN + TRAILER_LEN;

/// Packet context byte for a link request proof.
pub const CTX_LRPROOF: u8 = 0xff;

/// Packet context byte for a `Channel` message (RNS `Packet.CHANNEL`). A reliable stream's
/// envelopes ride link data packets under this context; see [`crate::reliable`].
pub const CTX_CHANNEL: u8 = 0x0e;

/// Packet context byte for a link IDENTIFY (RNS `Packet.LINKIDENTIFY`). An initiator sends
/// one so the responder learns its identity; see [`Link::identify_packet`].
pub const CTX_LINKIDENTIFY: u8 = 0xfb;

/// Packet context byte for the link RTT packet.
pub const CTX_LRRTT: u8 = 0xfe;

/// Packet context byte for a keepalive.
pub const CTX_KEEPALIVE: u8 = 0xfa;

/// Packet context byte for a link close.
pub const CTX_LINKCLOSE: u8 = 0xfc;

/// Packet context byte for a request over a link.
pub const CTX_REQUEST: u8 = 0x09;

/// Packet context byte for a response over a link.
pub const CTX_RESPONSE: u8 = 0x0a;

/// Resource context bytes. See [`crate::resource`].
pub const CTX_RESOURCE: u8 = 0x01;
/// Resource advertisement.
pub const CTX_RESOURCE_ADV: u8 = 0x02;
/// Resource part request.
pub const CTX_RESOURCE_REQ: u8 = 0x03;
/// Resource hashmap update.
pub const CTX_RESOURCE_HMU: u8 = 0x04;
/// Resource proof.
pub const CTX_RESOURCE_PRF: u8 = 0x05;
/// Resource initiator cancel.
pub const CTX_RESOURCE_ICL: u8 = 0x06;
/// Resource receiver cancel.
pub const CTX_RESOURCE_RCL: u8 = 0x07;

/// Keepalive request/response sentinels, carried as the single plaintext byte of a
/// keepalive packet.
pub const KEEPALIVE_REQUEST: u8 = 0xff;
pub const KEEPALIVE_RESPONSE: u8 = 0xfe;

/// The symmetric cipher a link will use. Negotiated, not fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkMode {
    Aes128Cbc,
    Aes256Cbc,
}

impl LinkMode {
    fn from_bits(bits: u8) -> Result<Self> {
        match bits {
            0 => Ok(Self::Aes128Cbc),
            1 => Ok(Self::Aes256Cbc),
            _ => Err(Error::BadLinkMode),
        }
    }

    fn to_bits(self) -> u32 {
        match self {
            Self::Aes128Cbc => 0,
            Self::Aes256Cbc => 1,
        }
    }
}

/// The 3-byte trailer on a link request or proof: a cipher mode and an MTU.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkTrailer {
    pub mode: LinkMode,
    pub mtu: u32,
}

/// The largest MTU the 21-bit field can carry.
pub const MAX_MTU: u32 = (1 << 21) - 1;

impl LinkTrailer {
    /// Decode the trailer. `mode` occupies the top 3 bits, `mtu` the low 21.
    pub fn decode(bytes: &[u8; TRAILER_LEN]) -> Result<Self> {
        let raw = u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2]);
        Ok(Self {
            mode: LinkMode::from_bits((raw >> 21) as u8)?,
            mtu: raw & MAX_MTU,
        })
    }

    /// Encode the trailer.
    pub fn encode(&self) -> [u8; TRAILER_LEN] {
        let raw = (self.mode.to_bits() << 21) | (self.mtu & MAX_MTU);
        [(raw >> 16) as u8, (raw >> 8) as u8, raw as u8]
    }
}

/// The link id implied by a link-request packet.
///
/// Returns [`Error::Truncated`] if the payload is too short to hold the key material.
pub fn link_id(request: &Packet) -> Result<AddressHash> {
    if request.payload.len() < LINK_KEYS_LEN {
        return Err(Error::Truncated);
    }

    let mut buf = Vec::with_capacity(1 + 16 + 1 + LINK_KEYS_LEN);
    // The flag byte is masked: the high nibble carries bits that change in transit (ifac,
    // header type, context flag, propagation), so they cannot be part of a stable id.
    //
    // Caveat, stated because it matters: both captured samples had flags == 0x02, where
    // masking is a no-op, so the capture does NOT prove the mask. It is taken on the
    // manual's and Beechat's authority, and only becomes observable for a link request
    // that arrives over a transport hop. Revisit if a two-hop link ever fails.
    buf.push(request.encode()[0] & 0x0F);
    buf.extend_from_slice(request.destination.as_slice());
    buf.push(request.context);
    buf.extend_from_slice(&request.payload[..LINK_KEYS_LEN]);

    Ok(AddressHash::of(&buf))
}

/// The mode/MTU trailer a link request signals, if it carries one. As in RNS, only a request
/// of exactly keys plus trailer signals; a bare 64-byte request does not.
pub fn request_trailer(request: &Packet) -> Result<Option<LinkTrailer>> {
    match request.payload.get(LINK_KEYS_LEN..) {
        Some(bytes) if bytes.len() == TRAILER_LEN => {
            LinkTrailer::decode(bytes.try_into().expect("checked length")).map(Some)
        }
        _ => Ok(None),
    }
}

/// Lower the MTU a link request signals to at most `limit`: what a transport hop can carry
/// between its two interfaces, or what a destination's receiving interface can. The link id
/// covers only the keys, so it is unchanged. A request without a trailer, or one that already
/// fits, passes untouched whatever mode it signals. As in RNS, only a request that must be
/// re-encoded under a mode this side does not know is an error, and is dropped.
pub fn clamp_request_mtu(request: &mut Packet, limit: u32) -> Result<()> {
    let Some(bytes) = request
        .payload
        .get_mut(LINK_KEYS_LEN..)
        .and_then(|bytes| <&mut [u8; TRAILER_LEN]>::try_from(bytes).ok())
    else {
        return Ok(());
    };
    let signalled =
        (u32::from(bytes[0]) << 16 | u32::from(bytes[1]) << 8 | u32::from(bytes[2])) & MAX_MTU;
    if signalled > limit {
        let trailer = LinkTrailer::decode(bytes)?;
        *bytes = LinkTrailer {
            mtu: limit,
            ..trailer
        }
        .encode();
    }
    Ok(())
}

/// Whether `proof` is a link-request proof that `destination` signed for the link it names.
///
/// A transport hop checks this before it carries the proof back and starts carrying the
/// link's traffic, so a forged proof can neither validate a bridge nor reach the initiator.
pub fn proof_is_signed_by(proof: &Packet, destination: &Identity) -> bool {
    let payload = &proof.payload;
    if proof.packet_type != PacketType::Proof
        || proof.context != CTX_LRPROOF
        || !matches!(payload.len(), n if n == LINK_PROOF_LEN || n == LINK_PROOF_LEN - TRAILER_LEN)
    {
        return false;
    }
    let (signature, rest) = payload.split_at(SIGNATURE_LEN);
    let (peer_eph, trailer) = rest.split_at(KEY_LEN);
    let mut signed = Vec::with_capacity(ADDRESS_HASH_LEN + 2 * KEY_LEN + TRAILER_LEN);
    signed.extend_from_slice(proof.destination.as_slice());
    signed.extend_from_slice(peer_eph);
    signed.extend_from_slice(destination.ed25519_bytes());
    signed.extend_from_slice(trailer);
    destination.verify(&signed, signature.try_into().expect("split length"))
}

/// Build the flag/hops/dest/context prefix and payload of a link-layer packet.
fn link_packet(context: u8, link_id: AddressHash, payload: Vec<u8>) -> Packet {
    Packet {
        ifac: false,
        header_type: HeaderType::Type1,
        context_flag: false,
        propagation: Propagation::Broadcast,
        destination_type: DestinationType::Link,
        packet_type: PacketType::Data,
        hops: 0,
        transport: None,
        destination: link_id,
        context,
        payload,
    }
}

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
    /// `ephemeral` is a fresh 64-byte keypair seed (`x25519_secret(32) ||
    /// ed25519_seed(32)`), generated per attempt by the caller: R3 stays RNG-free the same
    /// way R0 does, so this is reproducible and the runtime supplies the randomness.
    /// Returns the pending link and the request packet to send.
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
        if proof.payload.len() < SIGNATURE_LEN + KEY_LEN {
            return Err(Error::Truncated);
        }

        let signature: [u8; SIGNATURE_LEN] = proof.payload[..SIGNATURE_LEN]
            .try_into()
            .expect("checked length");
        let peer_eph: [u8; KEY_LEN] = proof.payload[SIGNATURE_LEN..SIGNATURE_LEN + KEY_LEN]
            .try_into()
            .expect("checked length");
        let trailer_bytes = &proof.payload[SIGNATURE_LEN + KEY_LEN..];

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

        // The proof's trailer is authoritative for the negotiated mode and MTU; fall back
        // to what we requested if the peer sent none. The path only ever lowers the MTU, so
        // a proof echoing more than we asked for is held to our request.
        let agreed = if trailer_bytes.len() >= TRAILER_LEN {
            let echoed =
                LinkTrailer::decode(trailer_bytes[..TRAILER_LEN].try_into().expect("len"))?;
            LinkTrailer {
                mtu: echoed.mtu.min(self.requested.mtu),
                ..echoed
            }
        } else {
            self.requested
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
/// caller against the request if it wants to honour the initiator's proposal.
pub fn accept(
    request: &Packet,
    destination: &PrivateIdentity,
    ephemeral_seed: &[u8; 64],
    offered: LinkTrailer,
) -> Result<(Link, Packet)> {
    if request.packet_type != PacketType::LinkRequest {
        return Err(Error::NotALinkRequest);
    }
    if request.payload.len() < LINK_KEYS_LEN {
        return Err(Error::Truncated);
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
        },
        proof,
    ))
}

/// Bytes of an explicit link-data proof payload: `full_hash(32) || signature(64)`. This is
/// RNS 1.3.8's `PacketReceipt.EXPL_LENGTH` (96); the implicit 64-byte form is not used for
/// link data, where the proof must carry the hash to name the packet it acknowledges.
pub const DATA_PROOF_LEN: usize = 32 + SIGNATURE_LEN;

/// Bytes of a link IDENTIFY payload (sealed): `public_key(64) || signature(64)`.
pub const LINK_IDENTIFY_LEN: usize = IDENTITY_LEN + SIGNATURE_LEN;

/// The message an IDENTIFY signs: `link_id(16) || public_key(64)`. Binding the identity to
/// the link id stops an identify from one link being replayed on another.
fn identify_signed_message(link_id: AddressHash, public: &[u8; IDENTITY_LEN]) -> Vec<u8> {
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
fn explicit_proof_packet(
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
fn split_data_proof(
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

/// The Ed25519 seed half of an identity's 64-byte secret.
fn signing_seed(identity: &PrivateIdentity) -> [u8; KEY_LEN] {
    identity.to_secret_bytes()[KEY_LEN..]
        .try_into()
        .expect("the Ed25519 half of 64 bytes is 32")
}

/// What an inbound link-layer packet is, once matched to a link by its id.
#[derive(Debug, PartialEq, Eq)]
pub enum Inbound {
    /// Application data, already decrypted.
    Data(Vec<u8>),
    /// The RTT packet that follows a proof. Its contents are not load-bearing.
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
/// The data channel is the R0 token with the link's static key. There is no per-packet
/// ECDH and no ephemeral prefix, because the forward secrecy already lives in the ephemeral
/// key exchange that established the link. This is why links carry no ratchet.
///
/// Each side also holds the two keys of link-data proofs, as RNS does: the initiator signs
/// with the ephemeral Ed25519 key from its request, the responder with its destination
/// identity. Both are kept as 32-byte seeds rather than expanded keys, so a link costs a
/// board 64 bytes for them, not several hundred.
///
/// `Clone` is cheap (an id and a handful of 32-byte keys) and lets a stream own a sealing
/// handle to the same link the router reads from.
#[derive(Clone)]
pub struct Link {
    id: AddressHash,
    keys: DerivedKeys,
    mode: LinkMode,
    mtu: u32,
    /// Ed25519 seed this side proves link data with.
    signer: [u8; KEY_LEN],
    /// Ed25519 public key the peer proves link data with. Not checked to be a valid point
    /// when the link forms: a bad one only means the peer's proofs never verify.
    peer_signer: [u8; KEY_LEN],
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::destination::DestinationName;

    fn request(dest_hex: &str, payload_hex: &str) -> Packet {
        let mut dest = [0u8; 16];
        hex::decode_to_slice(dest_hex, &mut dest).unwrap();
        Packet {
            ifac: false,
            header_type: HeaderType::Type1,
            context_flag: false,
            propagation: Propagation::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::LinkRequest,
            hops: 0,
            transport: None,
            destination: AddressHash::from_bytes(dest),
            context: 0,
            payload: hex::decode(payload_hex).unwrap(),
        }
    }

    /// Captured: RNS proved back to this link id for a 64-byte request retinue sent it.
    #[test]
    fn link_id_matches_the_captured_proof_address() {
        let p = request(
            "a8725a7e212dace39e9f99a8ac5da28c",
            "0faa684ed28867b97f4a6a2dee5df8ce974e76b7018e3f22a1c4cf2678570f20\
             a09aa5f47a6759802ff955f8dc2d2a14a5c99d23be97f864127ff9383455a4f0",
        );
        assert_eq!(
            link_id(&p).unwrap().to_string(),
            "7c88505173382e78aaaae5ecdf122eec",
        );
    }

    /// Captured: RNS reported this link id for the 67-byte request it sent retinue. The
    /// trailer must NOT feed the hash, and this is the case that proves it.
    #[test]
    fn link_id_ignores_the_trailer() {
        let p = request(
            "19208507854a8a0b871f881170d475aa",
            "f72075eeade493f3a3fd94d98cba8b628cf5cce2532b0903b48b1c2024676164\
             f7cf9beb5793e668eb8c589096d382c616db7a7ebdd0ec6407e8a7d89452dd4e\
             202000",
        );
        assert_eq!(p.payload.len(), LINK_REQUEST_LEN);
        assert_eq!(
            link_id(&p).unwrap().to_string(),
            "5452ae4c3251cffa6b080779f943dfa4",
        );
    }

    #[test]
    fn captured_trailers_decode() {
        // What an RNS initiator sends: AES-256, asking for 8192.
        let req = LinkTrailer::decode(&[0x20, 0x20, 0x00]).unwrap();
        assert_eq!(req.mode, LinkMode::Aes256Cbc);
        assert_eq!(req.mtu, 8192);

        // What the responder answers: AES-256, settling on 500 (= Reticulum.MTU).
        let proof = LinkTrailer::decode(&[0x20, 0x01, 0xf4]).unwrap();
        assert_eq!(proof.mode, LinkMode::Aes256Cbc);
        assert_eq!(proof.mtu, 500);
    }

    #[test]
    fn trailers_round_trip() {
        for t in [
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 8192,
            },
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
            LinkTrailer {
                mode: LinkMode::Aes128Cbc,
                mtu: 500,
            },
        ] {
            assert_eq!(LinkTrailer::decode(&t.encode()).unwrap(), t);
        }
        assert_eq!(
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 8192
            }
            .encode(),
            [0x20, 0x20, 0x00],
        );
    }

    #[test]
    fn a_short_payload_is_an_error() {
        let p = request("a8725a7e212dace39e9f99a8ac5da28c", "0faa");
        assert!(link_id(&p).is_err());
    }

    /// Initiator and responder, both retinue, must agree on the link id and the session
    /// key, and then talk. This checks the two sides are mutually consistent; the oracle
    /// gates check each side against RNS.
    #[test]
    fn initiator_and_responder_agree() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let peer = *dest_identity.public();
        let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);

        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };

        // Initiator opens.
        let (pending, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], trailer);

        // Responder accepts and proves.
        let (responder_link, proof) =
            accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();

        // Initiator verifies the proof and establishes.
        let initiator_link = pending.prove(&proof).unwrap();

        assert_eq!(initiator_link.id(), responder_link.id());
        assert_eq!(initiator_link.id(), pending.link_id());

        // The shared key round-trips: what one encrypts, the other decrypts.
        let msg = b"across the link";
        let packet = initiator_link.data_packet(msg, &[0x01; 16]);
        assert_eq!(
            responder_link.receive(&packet),
            Some(Inbound::Data(msg.to_vec()))
        );

        let back = responder_link.data_packet(b"and back", &[0x02; 16]);
        assert_eq!(
            initiator_link.receive(&back),
            Some(Inbound::Data(b"and back".to_vec())),
        );
    }

    #[test]
    fn receive_classifies_link_traffic() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let (_pending, request) = PendingLink::open(
            DestinationName::new("retinue", ["test"]).destination_hash(dest_identity.public()),
            *dest_identity.public(),
            &[0x33; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        let (link, _proof) = accept(
            &request,
            &dest_identity,
            &[0x99; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        )
        .unwrap();

        assert_eq!(
            link.receive(&link.keepalive_packet(KEEPALIVE_REQUEST)),
            Some(Inbound::KeepAliveRequest),
        );
        assert_eq!(
            link.receive(&link.keepalive_packet(KEEPALIVE_RESPONSE)),
            Some(Inbound::KeepAliveResponse),
        );
        let close = link.close_packet(&[0x44; IV_LEN]);
        assert_eq!(link.receive(&close), Some(Inbound::Close));
        let mut malformed_close = close;
        malformed_close.payload.truncate(16);
        assert_eq!(link.receive(&malformed_close), Some(Inbound::Unknown));

        // A packet for a different link id is not ours.
        let mut foreign = link.keepalive_packet(KEEPALIVE_REQUEST);
        foreign.destination = AddressHash::from_bytes([0xAB; 16]);
        assert_eq!(link.receive(&foreign), None);
    }

    /// Link-data proofs use RNS's keys in both directions: the initiator signs with the
    /// ephemeral key from its request, the responder with its destination identity, and
    /// each side validates the other's with no IDENTIFY.
    #[test]
    fn link_data_proofs_use_the_link_keys() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let (pending, request) = PendingLink::open(
            DestinationName::new("retinue", ["test"]).destination_hash(dest_identity.public()),
            *dest_identity.public(),
            &[0x33; 64],
            trailer,
        );
        let (responder, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
        let initiator = pending.prove(&proof).unwrap();
        let ephemeral = PrivateIdentity::from_secret_bytes(&[0x33; 64]);
        assert_eq!(
            &request.payload[KEY_LEN..LINK_KEYS_LEN],
            ephemeral.public().ed25519_bytes(),
            "request bytes 32..64 are the ephemeral Ed25519 key"
        );

        let to_responder = initiator.data_packet(b"up", &[0x01; IV_LEN]);
        let up = initiator.prove_packet(&to_responder);
        assert_eq!(
            up.encode(),
            data_proof_packet(initiator.id(), &to_responder.full_hash(), &ephemeral).encode(),
            "the initiator signs with its ephemeral key"
        );
        assert_eq!(
            responder.validate_proof(&up),
            Some(to_responder.full_hash())
        );

        let to_initiator = responder.data_packet(b"down", &[0x02; IV_LEN]);
        let down = responder.prove_packet(&to_initiator);
        assert_eq!(
            down.encode(),
            data_proof_packet(responder.id(), &to_initiator.full_hash(), &dest_identity).encode(),
            "the responder signs with its identity"
        );
        assert_eq!(
            initiator.validate_proof(&down),
            Some(to_initiator.full_hash())
        );

        // Neither side takes its own proof, or a stranger's, for the peer's.
        assert_eq!(initiator.validate_proof(&up), None);
        assert_eq!(responder.validate_proof(&down), None);
        let stranger = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
        assert_eq!(
            responder.validate_proof(&initiator.data_proof(&to_responder, &stranger)),
            None
        );
    }

    /// Gold test: retinue builds the exact link-data proof RNS 1.3.8 emitted for a packet
    /// we sent it (captured in `rns_link_proof.json`), and validates RNS's own proof back.
    /// Ed25519 is deterministic, so signing the same full hash with the same identity
    /// reproduces RNS's signature byte for byte — this pins the whole proof wire.
    #[test]
    fn data_proof_matches_rns_capture() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/rns_link_proof.json")).unwrap();
        let link_id =
            AddressHash::from_slice(&hex::decode(doc["link_id_hex"].as_str().unwrap()).unwrap())
                .unwrap();
        let full_hash: [u8; 32] =
            hex::decode(doc["our_sent_packet_hash_full_hex"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
        let seed: [u8; 64] = hex::decode(doc["prover_identity_secret_seed_hex"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let prover = PrivateIdentity::from_secret_bytes(&seed);

        // Generate: our proof packet is RNS's wire bytes exactly.
        let proof = data_proof_packet(link_id, &full_hash, &prover);
        assert_eq!(
            hex::encode(proof.encode()),
            doc["proofs"][0]["frame_hex"].as_str().unwrap(),
            "proof packet must equal RNS's wire bytes",
        );

        // Validate: retinue accepts RNS's proof against RNS's identity, recovering the hash.
        let pubbytes: [u8; 64] = hex::decode(doc["prover_identity_public_hex"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let peer = Identity::from_public_bytes(&pubbytes).unwrap();
        assert_eq!(
            read_data_proof(link_id, &proof, &peer),
            Some(full_hash),
            "validates and recovers the proven hash",
        );

        // A tampered signature is rejected, and a proof for a different link is not ours.
        let mut bad = proof.clone();
        *bad.payload.last_mut().unwrap() ^= 0x01;
        assert_eq!(
            read_data_proof(link_id, &bad, &peer),
            None,
            "tamper rejected"
        );
        assert_eq!(
            read_data_proof(AddressHash::from_bytes([0x00; 16]), &proof, &peer),
            None,
            "wrong link rejected",
        );
    }

    /// Gold test: retinue signs a link IDENTIFY to RNS 1.3.8's exact signature (captured in
    /// link_identify.json), which also confirms retinue's identity-pubkey derivation matches.
    #[test]
    fn identify_signature_matches_rns_capture() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/link_identify.json")).unwrap();
        let link_id =
            AddressHash::from_slice(&hex::decode(doc["link_id_hex"].as_str().unwrap()).unwrap())
                .unwrap();
        let public: [u8; IDENTITY_LEN] =
            hex::decode(doc["our_identity_public_hex"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
        let seed: [u8; IDENTITY_LEN] =
            hex::decode(doc["our_identity_secret_seed_hex"].as_str().unwrap())
                .unwrap()
                .try_into()
                .unwrap();
        let me = PrivateIdentity::from_secret_bytes(&seed);
        assert_eq!(
            me.public().to_public_bytes(),
            public,
            "pubkey derivation matches RNS"
        );
        let sig = me.sign(&identify_signed_message(link_id, &public));
        assert_eq!(
            hex::encode(sig),
            doc["signature_hex"].as_str().unwrap(),
            "identify signature must equal RNS's",
        );
    }

    /// An initiator's identify round-trips over a real link: the responder recovers the
    /// initiator's identity, and a tampered packet is rejected.
    #[test]
    fn identify_round_trips_over_a_link() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let (pending, request) = PendingLink::open(
            DestinationName::new("retinue", ["test"]).destination_hash(dest_identity.public()),
            *dest_identity.public(),
            &[0x33; 64],
            trailer,
        );
        let (responder, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
        let initiator = pending.prove(&proof).unwrap();

        let me = PrivateIdentity::from_secret_bytes(&[0x77; 64]);
        let pkt = initiator.identify_packet(&me, &[0x01; 16]);
        let learned = responder.read_identify(&pkt).expect("valid identify");
        assert_eq!(
            learned.hash(),
            me.public().hash(),
            "responder learns the initiator"
        );

        let mut bad = pkt.clone();
        *bad.payload.last_mut().unwrap() ^= 0x01;
        assert!(
            responder.read_identify(&bad).is_none(),
            "tampered identify rejected"
        );
    }

    /// A transport hop or destination lowers the signalled MTU without changing the link id,
    /// and leaves a bare request or a smaller request alone.
    #[test]
    fn clamping_a_request_keeps_its_link_id() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let peer = *dest_identity.public();
        let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
        let asked = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 8192,
        };
        let (pending, mut request) = PendingLink::open(dest_hash, peer, &[0x33; 64], asked);

        clamp_request_mtu(&mut request, 500).unwrap();
        assert_eq!(link_id(&request).unwrap(), pending.link_id());
        assert_eq!(
            request_trailer(&request).unwrap(),
            Some(LinkTrailer { mtu: 500, ..asked })
        );
        clamp_request_mtu(&mut request, 1024).unwrap();
        assert_eq!(request_trailer(&request).unwrap().unwrap().mtu, 500);

        let mut bare = request.clone();
        bare.payload.truncate(LINK_KEYS_LEN);
        clamp_request_mtu(&mut bare, 255).unwrap();
        assert_eq!(bare.payload.len(), LINK_KEYS_LEN);
        assert_eq!(request_trailer(&bare).unwrap(), None);

        let mut undecodable = request.clone();
        undecodable.payload[LINK_KEYS_LEN] = 0xe0;
        let before = undecodable.payload.clone();
        clamp_request_mtu(&mut undecodable, 500).expect("a fitting request passes unread");
        assert_eq!(undecodable.payload, before);
        assert!(clamp_request_mtu(&mut undecodable, 255).is_err());
    }

    /// The initiator holds the link to the MTU it asked for, whatever the proof echoes.
    #[test]
    fn an_initiator_never_adopts_more_than_it_requested() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let peer = *dest_identity.public();
        let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
        let asked = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 255,
        };
        let (pending, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], asked);
        let (_, proof) = accept(
            &request,
            &dest_identity,
            &[0x99; 64],
            LinkTrailer { mtu: 500, ..asked },
        )
        .unwrap();
        assert_eq!(pending.prove(&proof).unwrap().mtu(), 255);

        let (_, lower) = accept(
            &request,
            &dest_identity,
            &[0x99; 64],
            LinkTrailer { mtu: 247, ..asked },
        )
        .unwrap();
        assert_eq!(pending.prove(&lower).unwrap().mtu(), 247);
    }

    /// A relay accepts only the destination's own signature over the proof, trailer included.
    #[test]
    fn a_transport_hop_checks_the_proof_signature() {
        let dest_identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let impostor = PrivateIdentity::from_secret_bytes(&[0x12; 64]);
        let peer = *dest_identity.public();
        let dest_hash = DestinationName::new("retinue", ["test"]).destination_hash(&peer);
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let (_, request) = PendingLink::open(dest_hash, peer, &[0x33; 64], trailer);
        let (_, proof) = accept(&request, &dest_identity, &[0x99; 64], trailer).unwrap();
        assert!(proof_is_signed_by(&proof, &peer));
        assert!(!proof_is_signed_by(&proof, impostor.public()));

        let mut raised = proof.clone();
        raised.payload[LINK_PROOF_LEN - 1] ^= 1;
        assert!(!proof_is_signed_by(&raised, &peer), "the trailer is signed");

        let (_, forged) = accept(&request, &impostor, &[0x99; 64], trailer).unwrap();
        assert!(!proof_is_signed_by(&forged, &peer));

        let mut data = proof;
        data.context = 0;
        assert!(!proof_is_signed_by(&data, &peer));
    }
}
