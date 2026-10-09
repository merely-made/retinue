//! Sealed cancels: matching them to a resource, and rejecting an offer.

use crate::link::{CTX_RESOURCE_RCL, Link};
use crate::packet::Packet;
use crate::resource::Advertisement;
use crate::token::IV_LEN;

/// A sealed cancel naming `resource_hash`: `RESOURCE_RCL` from a receiver, `RESOURCE_ICL`
/// from the initiator.
pub(super) fn cancel_packet(
    link: &Link,
    context: u8,
    resource_hash: &[u8],
    iv: &[u8; IV_LEN],
) -> Packet {
    link.sealed_packet(context, resource_hash, iv)
}

/// Whether `packet` decrypts on `link` and names `resource_hash`, as RNS matches a cancel
/// to its resource.
pub(super) fn names_resource(link: &Link, packet: &Packet, resource_hash: &[u8; 32]) -> bool {
    link.decrypt(packet)
        .is_ok_and(|plain| plain.get(..32) == Some(resource_hash.as_slice()))
}

/// Reject an advertised resource without receiving any of it: RNS's `Resource.reject`.
///
/// Returns the sealed receiver cancel naming the advertised resource, or `None` if
/// `advertisement` does not decrypt on `link` to a well-formed advertisement.
pub fn reject(link: &Link, advertisement: &Packet, iv: &[u8; IV_LEN]) -> Option<Packet> {
    let plain = link.decrypt(advertisement).ok()?;
    let adv = Advertisement::parse(&plain).ok()?;
    (adv.resource_hash.len() == 32)
        .then(|| cancel_packet(link, CTX_RESOURCE_RCL, &adv.resource_hash, iv))
}
