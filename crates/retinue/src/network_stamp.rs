//! Network stamp wire primitives, adapted from stock RNS 1.6.1.
//!
//! RNS Stamps.py, Destination.py, Transport.py and Link.py, copyright 2016-2026
//! Mark Qvist, under the Reticulum License. See this crate's LICENSE and NOTICE.

use crate::hash::AddressHash;
use crate::identity::Identity;
use crate::stamp::{StampCost, detached_valid};
use alloc::vec::Vec;

/// Detached destination and path-request stamp length on the RNS 1.6 wire.
pub const STAMP_LEN: usize = 14;
/// RNS extension marker, reserved at MessagePack object boundaries.
pub const EXT_MARKER: u8 = 0xc1;
/// Destination extension: marker, flags, stamp, fractional/integer link cost.
pub const ANNOUNCE_EXT_LEN: usize = 18;
/// Path extension: marker, flags, stamp.
pub const PATH_EXT_LEN: usize = 16;

/// A recognized signed destination extension. The announce signature covers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkStamp {
    pub stamp: [u8; STAMP_LEN],
    pub link_cost: StampCost,
}

impl NetworkStamp {
    pub fn decode(app_data: &[u8]) -> Option<Self> {
        let ext = app_data.get(app_data.len().checked_sub(ANNOUNCE_EXT_LEN)?..)?;
        // The high bit is reserved for a future extended format.
        if ext[0] != EXT_MARKER || ext[17] & 0x80 != 0 {
            return None;
        }
        let fp = u16::from_be_bytes([ext[17], ext[16]]);
        if fp > 100 * 256 {
            return None;
        }
        Some(Self {
            stamp: ext[2..16].try_into().ok()?,
            link_cost: StampCost::from_fp(fp),
        })
    }
    pub fn encode(self) -> [u8; ANNOUNCE_EXT_LEN] {
        let mut ext = [0; ANNOUNCE_EXT_LEN];
        ext[0] = EXT_MARKER;
        ext[2..16].copy_from_slice(&self.stamp);
        let cost = self.link_cost.to_fp().to_be_bytes();
        ext[16] = cost[1];
        ext[17] = cost[0];
        ext
    }
}

pub fn destination_material(
    destination: AddressHash,
    identity: &Identity,
    name: crate::hash::NameHash,
) -> [u8; 90] {
    let mut material = [0; 90];
    material[..16].copy_from_slice(destination.as_slice());
    material[16..80].copy_from_slice(&identity.to_public_bytes());
    material[80..].copy_from_slice(name.as_slice());
    material
}

pub fn path_material(destination: AddressHash, tag: &[u8]) -> Vec<u8> {
    let mut material = Vec::with_capacity(16 + tag.len());
    material.extend_from_slice(destination.as_slice());
    material.extend_from_slice(tag);
    material
}

/// A bounded, cancellation-aware search. Every candidate must be fresh CSPRNG output
/// supplied by the caller. No candidate is returned when the budget or stop condition wins.
pub fn generate_detached(
    material: &[u8],
    cost: StampCost,
    max_attempts: u64,
    mut random: impl FnMut(&mut [u8]),
    mut stop: impl FnMut() -> bool,
) -> Option<[u8; STAMP_LEN]> {
    for _ in 0..max_attempts {
        if stop() {
            return None;
        }
        let mut stamp = [0; STAMP_LEN];
        random(&mut stamp);
        if detached_valid(material, &stamp, cost) {
            return Some(stamp);
        }
    }
    None
}

/// Mine the X25519 half of a link's fresh ephemeral keyset against its destination
/// and Ed25519 public half. The key bytes carry the stamp, with no extra wire field.
pub fn generate_link_seed(
    destination: AddressHash,
    mut seed: [u8; 64],
    cost: StampCost,
    max_attempts: u64,
    mut random: impl FnMut(&mut [u8]),
    mut stop: impl FnMut() -> bool,
) -> Option<[u8; 64]> {
    let signing = crate::identity::PrivateIdentity::from_secret_bytes(&seed);
    let mut material = [0; 48];
    material[..16].copy_from_slice(destination.as_slice());
    material[16..].copy_from_slice(signing.public().ed25519_bytes());
    for _ in 0..max_attempts {
        if stop() {
            return None;
        }
        random(&mut seed[..32]);
        let secret =
            x25519_dalek::StaticSecret::from(<[u8; 32]>::try_from(&seed[..32]).expect("seed"));
        let public = x25519_dalek::PublicKey::from(&secret);
        if detached_valid(&material, public.as_bytes(), cost) {
            return Some(seed);
        }
    }
    None
}

/// Operator-selected network stamp policy. Defaults match RNS 1.6.1: stamp emitted
/// announces, path requests and requested link requests, but do not enforce admission
/// until explicitly enabled.
///
/// `generation_timeout` bounds path and link request stamps, as RNS's `pr_stamp_timeout`
/// and `lr_stamp_timeout` do. A destination stamp is generated once and runs to
/// completion, as in RNS, stopped only by shutdown, a policy change or `max_attempts`;
/// the endpoint's [`DestinationStampStore`](crate::endpoint::DestinationStampStore)
/// keeps it across restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkStampPolicy {
    pub stamp_destinations: bool,
    pub stamp_path_requests: bool,
    pub stamp_link_requests: bool,
    pub always_stamp_links: bool,
    pub enforce: bool,
    pub destination_cost: StampCost,
    pub destination_value: StampCost,
    pub path_cost: StampCost,
    pub path_value: StampCost,
    pub link_cost: StampCost,
    pub generation_timeout: core::time::Duration,
    pub max_attempts: u64,
    /// Maximum cached local/peer destination stamp metadata rows.
    pub destination_capacity: usize,
    /// Maximum simultaneous destination, path and link generation jobs.
    pub generation_capacity: usize,
}
impl NetworkStampPolicy {
    /// RNS 1.6.0's transition defaults: link stamps when a peer asks for them, no
    /// destination or path-request stamps. Retinue's tests and oracle peers written for
    /// 1.6.0 keep these explicitly.
    pub fn rns_1_6_0() -> Self {
        Self {
            stamp_destinations: false,
            stamp_path_requests: false,
            ..Self::default()
        }
    }
}

impl Default for NetworkStampPolicy {
    fn default() -> Self {
        Self {
            stamp_destinations: true,
            stamp_path_requests: true,
            stamp_link_requests: true,
            always_stamp_links: false,
            enforce: false,
            destination_cost: StampCost::from_fp(6016),
            destination_value: StampCost::from_fp(6400),
            path_cost: StampCost::from_fp(4045),
            path_value: StampCost::from_fp(4045),
            link_cost: StampCost::from_fp(3200),
            generation_timeout: core::time::Duration::from_secs(15),
            max_attempts: 1 << 32,
            destination_capacity: 4096,
            generation_capacity: 8,
        }
    }
}
