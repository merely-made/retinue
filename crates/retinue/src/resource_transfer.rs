//! Driving a resource transfer over a [`Link`](crate::link::Link): the sans-io
//! sender/receiver pair that runs the resource codec ([`crate::resource`]) over link packets,
//! as [`crate::reliable`] drives the `Channel`/`Buffer` codec. The codec carries RNS 1.3.8
//! wire fixtures; local transfer gates use the live-oracle pin in `oracle/requirements.txt`.
//!
//! # Wire, by link context byte
//!
//! ```text
//! 0x02 RESOURCE_ADV   the advertisement (msgpack), sealed; (re)sent until the receiver responds
//! 0x03 RESOURCE_REQ   the receiver's request for parts / solicitation for more hashmap, sealed
//! 0x01 RESOURCE       one part: a raw slice of the sealed token, framed (not re-sealed)
//! 0x04 RESOURCE_HMU   a hashmap update for a resource with more parts than one advert carries
//! 0x05 RESOURCE_PRF   the receiver's proof of receipt, a PROOF-type packet, unencrypted:
//!                     resource_hash(32) || proof(32)
//! 0x06 RESOURCE_ICL   the initiator cancels, sealed: resource_hash(32)
//! 0x07 RESOURCE_RCL   the receiver cancels or rejects, sealed: resource_hash(32)
//! 0x08 CACHE_REQUEST  a sender awaiting its proof asks for it again, unencrypted:
//!                     the proof packet's full hash(32)
//! ```
//!
//! The payload is sealed into the token **once**, then split, so a part is a slice of the
//! already-encrypted token and rides framed. Control packets are sealed; the proof, carrying
//! only public hashes, rides unencrypted in a PROOF-type packet, the only form RNS accepts. A
//! sender still accepts the DATA-type proof older retinue receivers sent.
//!
//! Either side ends a transfer it will not finish with a sealed cancel naming the resource
//! (`Resource.cancel`, `Resource.reject`). A cancel is honoured only if it decrypts on the
//! link and names the resource in progress.
//!
//! Both halves are sans-io: [`ResourceSender::on_packet`] / [`ResourceReceiver::on_packet`]
//! take a received packet and return packets to send, and the retransmit helpers re-emit on a
//! stall.

use alloc::boxed::Box;

use crate::resource::Advertisement;

mod cancel;
mod receiver;
mod sender;
#[cfg(test)]
mod tests;

pub use cancel::reject;
pub use receiver::ResourceReceiver;
pub use sender::ResourceSender;

/// How many times a sender that has sent every part asks for its missing proof with a
/// cache request. RNS resets `retries_left` to 3 on entering `AWAITING_PROOF`.
pub const PROOF_CACHE_REQUESTS: u8 = 3;

/// How many times a receiver re-sends one kept proof, to cache requests or to a
/// re-advertisement of the resource it proved: every request an honest sender makes, with
/// room for a lost answer. The request is unencrypted and anyone who heard the proof can
/// name its hash, so the cap bounds what a third party can make the receiver transmit.
pub const PROOF_CACHE_ANSWERS: u8 = PROOF_CACHE_REQUESTS + 2;

/// Decides whether to accept an advertised resource, as an RNS link's `ACCEPT_APP`
/// callback does: it sees the advertisement (sizes, part count, flags) before any part is
/// requested. A refused offer is answered with a receiver cancel.
pub type AcceptHook = Box<dyn Fn(&Advertisement) -> bool + Send + Sync>;
