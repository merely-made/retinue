//! Bounded transport tables and small helpers shared across the node.

use alloc::vec::Vec;

use heapless::Vec as BoundedVec;

use super::{InterfaceId, InterfaceMode};
use crate::hash::AddressHash;
use crate::link;

#[derive(Debug, Clone, Copy)]
pub(super) struct Route {
    pub(super) destination: AddressHash,
    pub(super) interface: InterfaceId,
    /// The next transport hop that announced this destination, if it is not direct.
    pub(super) transport: Option<AddressHash>,
    pub(super) hops: u8,
    /// When the route was learned, or last carried traffic.
    pub(super) learned: u64,
    /// The learning interface's mode when the route was learned.
    pub(super) mode: InterfaceMode,
}

impl Route {
    /// Whether the route is still usable at `now`, given the configured full-mode lifetime.
    pub(super) fn live(&self, now: u64, route_ttl: u64) -> bool {
        now.saturating_sub(self.learned) < self.mode.route_ttl(route_ttl)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct LinkBridge {
    pub(super) link_id: AddressHash,
    /// The destination the request named, whose identity signs the proof.
    pub(super) destination: AddressHash,
    pub(super) from: InterfaceId,
    pub(super) out: InterfaceId,
    pub(super) seen: u64,
    /// When an unproved link lapses; `None` once the destination's proof has passed.
    pub(super) proof_deadline: Option<u64>,
}

impl LinkBridge {
    pub(super) fn lapsed(&self, now: u64, ttl: u64) -> bool {
        self.proof_deadline
            .map_or(now.saturating_sub(self.seen) >= ttl, |deadline| {
                now >= deadline
            })
    }
}

/// Hashes remembered in two generations, as RNS keeps its packet hash list and the one before
/// it (`Transport.py` 832-834). A full generation becomes the previous one, so a burst forgets
/// the older half rather than everything. A generation also turns over after `max_age`, and
/// both are cleared after twice that, so a quiet node does not remember a hash indefinitely.
#[derive(Debug)]
pub(super) struct HashGenerations<const N: usize> {
    pub(super) current: BoundedVec<AddressHash, N>,
    previous: BoundedVec<AddressHash, N>,
    started: u64,
}

impl<const N: usize> HashGenerations<N> {
    pub(super) const fn new() -> Self {
        Self {
            current: BoundedVec::new(),
            previous: BoundedVec::new(),
            started: 0,
        }
    }

    /// Record `hash` at `now`; false if it was already remembered.
    pub(super) fn insert(&mut self, hash: AddressHash, now: u64, max_age: u64) -> bool {
        let age = now.saturating_sub(self.started);
        if age >= max_age.saturating_mul(2) {
            self.previous.clear();
            self.current.clear();
            self.started = now;
        } else if age >= max_age || self.current.is_full() {
            self.previous = core::mem::take(&mut self.current);
            self.started = now;
        }
        if self.current.contains(&hash) || self.previous.contains(&hash) {
            return false;
        }
        let _ = self.current.push(hash);
        true
    }
}

/// The way back for a carried packet's proof: RNS's reverse-table entry, keyed by the
/// truncated packet hash the proof is addressed to.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReverseEntry {
    pub(super) packet: AddressHash,
    pub(super) received: InterfaceId,
    pub(super) outbound: InterfaceId,
    pub(super) seen: u64,
}

/// One derived resource IV: `full_hash(tag || identity secret || link id || counter)`.
///
/// Deterministic on purpose — this layer holds no RNG — and unique by the counter, which
/// the node owns and never resets.
pub(super) fn derived_iv(
    seed: &[u8; 64],
    link_id: AddressHash,
    counter: &mut u32,
) -> [u8; crate::token::IV_LEN] {
    *counter = counter.wrapping_add(1);
    let mut input = Vec::with_capacity(48);
    input.extend_from_slice(b"retinue/node/resource-iv");
    input.extend_from_slice(seed);
    input.extend_from_slice(link_id.as_slice());
    input.extend_from_slice(&counter.to_le_bytes());
    let digest = crate::hash::full_hash(&input);
    let mut out = [0_u8; crate::token::IV_LEN];
    out.copy_from_slice(&digest[..crate::token::IV_LEN]);
    out
}

/// Whether a link-packet context belongs to a resource transfer.
pub(super) fn is_resource_context(context: u8) -> bool {
    matches!(
        context,
        link::CTX_RESOURCE
            | link::CTX_RESOURCE_ADV
            | link::CTX_RESOURCE_REQ
            | link::CTX_RESOURCE_HMU
            | link::CTX_RESOURCE_PRF
            | link::CTX_RESOURCE_ICL
            | link::CTX_RESOURCE_RCL
    )
}

/// Whether a link packet with this context is checked against the sent and received
/// windows. Resource contexts are not: a transfer has its own part and request bookkeeping,
/// and a re-sent part legitimately repeats its hash. Keepalives are not: every request on a
/// link is the same unencrypted byte, so each one repeats the last one's hash. Nor are cache
/// requests, which a sender repeats verbatim and RNS's packet filter also lets through.
pub(crate) fn is_deduplicated_link_context(context: u8) -> bool {
    !is_resource_context(context)
        && context != link::CTX_KEEPALIVE
        && context != link::CTX_CACHE_REQUEST
}

/// Remember a packet hash in a window, forgetting the oldest at capacity. A burst can outrun
/// the window; that only lets a late copy through, never drops a new packet.
pub(super) fn remember_hash<const N: usize>(
    window: &mut BoundedVec<AddressHash, N>,
    hash: AddressHash,
) {
    if window.is_full() {
        window.remove(0);
    }
    let _ = window.push(hash);
}
