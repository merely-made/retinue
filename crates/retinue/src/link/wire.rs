//! Wire constants, the mode/MTU trailer, and the link id.

use alloc::vec::Vec;

use crate::hash::{ADDRESS_HASH_LEN, AddressHash};
use crate::identity::{Identity, KEY_LEN, SIGNATURE_LEN};
use crate::packet::{DestinationType, HeaderType, Packet, PacketType, Propagation};
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
/// one so the responder learns its identity; see [`super::Link::identify_packet`].
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
/// Cache request: the unencrypted full hash of a packet the sender wants re-sent from the
/// peer's cache. A resource sender awaiting its proof asks for it this way.
pub const CTX_CACHE_REQUEST: u8 = 0x08;

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
    // The high flag nibble changes in transit, so it is masked out. Both captures had
    // flags == 0x02, so the mask rests on the manual and Beechat, not on the capture.
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
pub(super) fn link_packet(context: u8, link_id: AddressHash, payload: Vec<u8>) -> Packet {
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
