//! The packet header and its codec.
//!
//! A packet is a two-byte header, one or two address fields, a context byte, and a
//! payload:
//!
//! ```text
//! byte 0   flags
//! byte 1   hops
//! 2..18    destination address hash (16)
//! [18..34] transport address hash (16), only when header_type == Type2]
//! next     context byte
//! rest     payload
//! ```
//!
//! Flag byte 0, most significant bit first:
//!
//! ```text
//! bit 7     IFAC flag
//! bit 6     header type   (0 = one address field, 1 = two)
//! bit 5     context flag  (announce: a ratchet key is present)
//! bit 4     propagation   (0 = broadcast, 1 = transport)
//! bits 3..2 destination type (single=0, group=1, plain=2, link=3)
//! bits 1..0 packet type      (data=0, announce=1, link request=2, proof=3)
//! ```
//!
//! Verified against RNS 1.3.8. A plain announce has flags `0x01`; the same announce with
//! ratchets enabled has `0x21`, differing only in bit 5. `HEADER_MINSIZE = 19` = 2 + 16 + 1
//! and `HEADER_MAXSIZE = 35` = 2 + 16 + 16 + 1 corroborate the address-field layout.

// Needed by the test build or the tokio shell; the bare no_std lib does not reach it.
#[allow(unused_imports)]
use alloc::vec;
// Needed by the test build or the tokio shell; the bare no_std lib does not reach it.
#[allow(unused_imports)]
use alloc::string::ToString;

use alloc::vec::Vec;

use sha2::{Digest, Sha256};

use crate::hash::{ADDRESS_HASH_LEN, AddressHash};
use crate::{Error, Result};

/// Smallest possible header: flags, hops, one address, context.
pub const HEADER_MIN_LEN: usize = 2 + ADDRESS_HASH_LEN + 1;

/// Largest possible header: as above, with a second address field.
pub const HEADER_MAX_LEN: usize = 2 + ADDRESS_HASH_LEN * 2 + 1;

/// Maximum size of a whole packet on the wire. `RNS.Reticulum.MTU`.
pub const MTU: usize = 500;

/// Maximum size of the data field: a plain (unencrypted) payload. `RNS.Reticulum.MDU` and
/// `RNS.Packet.PLAIN_MDU`. A packet whose data exceeds this is dropped by RNS.
pub const MDU: usize = 464;

/// Maximum plaintext when the data field carries an encrypted token: the token framing
/// (IV + HMAC + padding) eats into [`MDU`], so the plaintext limit is lower.
/// `RNS.Packet.ENCRYPTED_MDU`. Link data, resource parts, and single-packet encryption must
/// keep plaintext at or under this.
pub const ENCRYPTED_MDU: usize = 383;

// MDU is the protocol data cap and sits comfortably inside the raw MTU-minus-header room
// (500 - 19 = 481); RNS reserves the difference.
const _: () = assert!(MDU < MTU - HEADER_MIN_LEN);

/// What kind of packet this is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PacketType {
    Data,
    Announce,
    LinkRequest,
    Proof,
}

impl PacketType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0 => Self::Data,
            1 => Self::Announce,
            2 => Self::LinkRequest,
            _ => Self::Proof,
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Announce => 1,
            Self::LinkRequest => 2,
            Self::Proof => 3,
        }
    }
}

/// What kind of destination the address field names.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DestinationType {
    Single,
    Group,
    Plain,
    Link,
}

impl DestinationType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0 => Self::Single,
            1 => Self::Group,
            2 => Self::Plain,
            _ => Self::Link,
        }
    }

    fn to_bits(self) -> u8 {
        match self {
            Self::Single => 0,
            Self::Group => 1,
            Self::Plain => 2,
            Self::Link => 3,
        }
    }
}

/// Whether the packet carries one address field or two.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeaderType {
    /// One address field: the destination.
    Type1,
    /// Two address fields: a transport hop, then the destination.
    Type2,
}

/// How the packet propagates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Propagation {
    Broadcast,
    Transport,
}

/// A decoded packet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    /// Bit 7 as it arrived. A logical packet never carries it: IFAC is the interface's
    /// envelope, which [`crate::ifac::Ifac::seal`] adds and [`crate::ifac::Ifac::open`]
    /// removes, so a decoded packet with this set reached a decoder that skipped IFAC and
    /// must be refused. [`Packet::encode`] never writes it.
    pub ifac: bool,
    pub header_type: HeaderType,
    /// Bit 5. On an announce this means "a ratchet key is present in the payload".
    pub context_flag: bool,
    pub propagation: Propagation,
    pub destination_type: DestinationType,
    pub packet_type: PacketType,
    pub hops: u8,
    /// The second address field, present only when `header_type == Type2`.
    pub transport: Option<AddressHash>,
    pub destination: AddressHash,
    pub context: u8,
    pub payload: Vec<u8>,
}

impl Packet {
    /// Decode a packet from the wire.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < HEADER_MIN_LEN {
            return Err(Error::Truncated);
        }
        // RNS drops over-MTU packets; reject them here so a peer cannot get us to hold an
        // arbitrarily large buffer as a valid packet.
        if bytes.len() > MTU {
            return Err(Error::Oversize);
        }

        let flags = bytes[0];
        let ifac = flags & 0b1000_0000 != 0;
        let header_type = if flags & 0b0100_0000 != 0 {
            HeaderType::Type2
        } else {
            HeaderType::Type1
        };
        let context_flag = flags & 0b0010_0000 != 0;
        let propagation = if flags & 0b0001_0000 != 0 {
            Propagation::Transport
        } else {
            Propagation::Broadcast
        };
        let destination_type = DestinationType::from_bits(flags >> 2);
        let packet_type = PacketType::from_bits(flags);
        let hops = bytes[1];

        let mut off = 2;
        let transport = match header_type {
            HeaderType::Type2 => {
                let h = AddressHash::from_slice(&bytes[off..]).ok_or(Error::Truncated)?;
                off += ADDRESS_HASH_LEN;
                Some(h)
            }
            HeaderType::Type1 => None,
        };

        let destination = AddressHash::from_slice(&bytes[off..]).ok_or(Error::Truncated)?;
        off += ADDRESS_HASH_LEN;

        let context = *bytes.get(off).ok_or(Error::Truncated)?;
        off += 1;

        Ok(Self {
            ifac,
            header_type,
            context_flag,
            propagation,
            destination_type,
            packet_type,
            hops,
            transport,
            destination,
            context,
            payload: bytes[off..].to_vec(),
        })
    }

    /// Address this packet to its first relay: header type 2 naming `transport`, the relay's
    /// identity hash. `None` (the destination is heard directly) leaves it unchanged.
    pub(crate) fn address_via(&mut self, transport: Option<AddressHash>) {
        if let Some(transport) = transport {
            self.header_type = HeaderType::Type2;
            self.transport = Some(transport);
        }
    }

    /// The RNS packet hash: `trunc16(SHA256(masked_flags || destination || context ||
    /// payload))`, where `masked_flags` is the low nibble of the flag byte (the high nibble
    /// changes in transit). This is what RNS calls a packet's truncated hash, and it is the
    /// `request_id` that ties a response to its request. Verified against RNS 1.3.8.
    ///
    /// Note the transport address of a header-type-2 packet is deliberately excluded, the
    /// same way RNS excludes it, so the hash is stable across a transport hop.
    pub fn hash(&self) -> AddressHash {
        AddressHash::from_bytes(
            self.full_hash()[..ADDRESS_HASH_LEN]
                .try_into()
                .expect("32 >= 16"),
        )
    }

    /// The full 32-byte SHA-256 packet hash: `SHA256(masked_flags || destination ||
    /// context || payload)`. [`hash`](Self::hash) is its 16-byte truncation. A link data
    /// **proof** carries this full hash to identify the packet it acknowledges (see
    /// [`crate::link::Link::data_proof`]); the truncation alone is what RNS calls the
    /// packet's truncated hash. Verified against RNS 1.3.8.
    pub fn full_hash(&self) -> [u8; 32] {
        #[cfg(test)]
        crate::probe::hit(crate::probe::Probe::PacketHash);
        let mut sha = Sha256::new();
        sha.update([self.flags() & 0x0F]);
        sha.update(self.destination.as_slice());
        sha.update([self.context]);
        sha.update(&self.payload);
        sha.finalize().into()
    }

    /// The flag byte as [`encode`](Self::encode) writes it, IFAC bit clear.
    fn flags(&self) -> u8 {
        let mut flags = self.destination_type.to_bits() << 2 | self.packet_type.to_bits();
        if matches!(self.header_type, HeaderType::Type2) {
            flags |= 0b0100_0000;
        }
        if self.context_flag {
            flags |= 0b0010_0000;
        }
        if matches!(self.propagation, Propagation::Transport) {
            flags |= 0b0001_0000;
        }
        flags
    }

    /// The encoded length of this packet on the wire.
    pub fn encoded_len(&self) -> usize {
        let header = match self.header_type {
            HeaderType::Type1 => HEADER_MIN_LEN,
            HeaderType::Type2 => HEADER_MAX_LEN,
        };
        header + self.payload.len()
    }

    /// Whether this packet fits the MTU. A packet that does not is dropped by RNS, so a
    /// caller building packets by hand should check this (the link, resource, and endpoint
    /// layers keep within it by construction).
    pub fn within_mtu(&self) -> bool {
        self.encoded_len() <= MTU
    }

    /// Encode a packet for the wire.
    ///
    /// The IFAC flag is left clear whatever [`Self::ifac`] says. Only an IFAC interface's
    /// [`crate::ifac::Ifac::seal`] sets it, so a forwarded copy of a received packet does
    /// not leave a plain interface carrying a flag every RNS peer drops it for.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER_MAX_LEN + self.payload.len());
        out.push(self.flags());
        out.push(self.hops);
        if let Some(t) = self.transport {
            out.extend_from_slice(t.as_slice());
        }
        out.extend_from_slice(self.destination.as_slice());
        out.push(self.context);
        out.extend_from_slice(&self.payload);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn announce_flags_round_trip() {
        // 0x01: announce, single, broadcast, one address field, no context flag.
        let p = Packet::decode(&{
            let mut v = vec![0x01, 0x00];
            v.extend_from_slice(&[0xAA; 16]);
            v.push(0x00);
            v.extend_from_slice(b"payload");
            v
        })
        .unwrap();
        assert_eq!(p.packet_type, PacketType::Announce);
        assert_eq!(p.destination_type, DestinationType::Single);
        assert_eq!(p.header_type, HeaderType::Type1);
        assert_eq!(p.propagation, Propagation::Broadcast);
        assert!(!p.context_flag);
        assert!(!p.ifac);
        assert_eq!(p.payload, b"payload");
        assert_eq!(p.encode()[0], 0x01);
    }

    #[test]
    fn context_flag_is_bit_five() {
        let mut v = vec![0x21, 0x00];
        v.extend_from_slice(&[0xAA; 16]);
        v.push(0x00);
        let p = Packet::decode(&v).unwrap();
        assert!(p.context_flag);
        assert_eq!(p.packet_type, PacketType::Announce);
        assert_eq!(p.encode()[0], 0x21);
    }

    #[test]
    fn encode_never_sets_the_ifac_flag() {
        // An IFAC-flagged frame decodes with the flag recorded, but its forwarded copy
        // leaves with bit 7 clear; only `Ifac::seal` sets it.
        let mut v = vec![0x80 | 0x21, 0x03];
        v.extend_from_slice(&[0xAA; 16]);
        v.push(0x00);
        let p = Packet::decode(&v).unwrap();
        assert!(p.ifac);
        let wire = p.encode();
        assert_eq!(wire[0] & 0x80, 0);
        assert_eq!(wire[0], 0x21);
        assert_eq!(wire[1..], v[1..]);
    }

    #[test]
    fn truncated_input_is_an_error() {
        assert!(Packet::decode(&[0x01, 0x00]).is_err());
    }

    #[test]
    fn oversize_input_is_rejected() {
        // A buffer larger than the wire MTU is not a valid packet; the decoder must reject it
        // rather than accept an arbitrarily large payload.
        assert!(matches!(
            Packet::decode(&vec![0u8; MTU + 1]),
            Err(Error::Oversize)
        ));
        // A packet exactly at the MTU still decodes.
        assert!(Packet::decode(&vec![0u8; MTU]).is_ok());
    }

    #[test]
    fn mtu_and_mdu_bounds() {
        let mut p = Packet::decode(&{
            let mut v = vec![0x00, 0x00];
            v.extend_from_slice(&[0xAA; 16]);
            v.push(0x00);
            v
        })
        .unwrap();
        p.payload = vec![0u8; MDU];
        assert!(p.within_mtu());
        // within_mtu is the hard wire bound: payload up to MTU - header fits, beyond fails.
        p.payload = vec![0u8; MTU - HEADER_MIN_LEN];
        assert!(p.within_mtu());
        p.payload = vec![0u8; MTU - HEADER_MIN_LEN + 1];
        assert!(!p.within_mtu());
    }

    /// The streamed hash matches the formula over the encoded bytes for every flag byte, and
    /// ignores what changes in transit: hops, the transport field and the high nibble.
    #[test]
    fn full_hash_streams_the_masked_header() {
        for flags in 0..=0x7Fu8 {
            let mut raw = vec![flags, 7];
            if flags & 0x40 != 0 {
                raw.extend_from_slice(&[0xBB; 16]);
            }
            raw.extend_from_slice(&[0xAA; 16]);
            raw.push(0x0E);
            raw.extend_from_slice(b"payload bytes");
            let p = Packet::decode(&raw).unwrap();
            let wire = p.encode();
            let mut hashable = vec![wire[0] & 0x0F];
            hashable.extend_from_slice(&wire[wire.len() - 16 - 1 - 13..]);
            assert_eq!(
                p.full_hash(),
                crate::hash::full_hash(&hashable),
                "{flags:#04x}"
            );

            let mut moved = p.clone();
            moved.hops = 0;
            moved.header_type = HeaderType::Type2;
            moved.transport = Some(AddressHash::from_bytes([0xCC; 16]));
            moved.propagation = Propagation::Transport;
            moved.context_flag = !moved.context_flag;
            assert_eq!(moved.full_hash(), p.full_hash());
        }
    }

    /// Known answer from `oracle/capture_reqresp_response.py`: this exact request packet
    /// hashes to the request_id RNS's handler reported. Pins the packet-hash formula.
    #[test]
    fn packet_hash_matches_rns_request_id() {
        let raw = hex::decode(
            "0c00e83cb1d07bf63284ec96514304f2968809d0572c505e36c218000000000000\
             0002d73c7bae548832cc812652952250be37815ddba83dc19dd524bd54d2582bf58\
             96f91bd15bc04e6488955bef5ecdad979431ec22ba1316d1ba01a9ed425e666c4be\
             c72382a3a3b44dc12821925b5ab4b1",
        )
        .unwrap();
        let packet = Packet::decode(&raw).unwrap();
        assert_eq!(
            packet.hash().to_string(),
            "9ab513b3bba3e87c5878bba6bf421119"
        );
    }
}
