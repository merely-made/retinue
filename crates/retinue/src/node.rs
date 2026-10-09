//! The executor-neutral node: the shape a board runs.
//!
//! [`Endpoint`](crate::endpoint) is the desktop shell, with tokio tasks, sockets and a clock.
//! This is the same protocol work without the shell:
//!
//! ```text
//! node.ingest(interface, packet, now)       -> Actions
//! node.poll(now, interface, announce_blob?) -> Actions
//! ```
//!
//! Nothing here reads a clock, allocates without a bound, or performs I/O. Time arrives as
//! `now`, announce ordinals as caller-supplied [`AnnounceBlob`] values, and everything the
//! node wants done leaves as an [`Action`] for the shell.
//!
//! The node calls the same `announce`, `link`, `channel` and `resource` code the desktop does,
//! at the small capacity profile, so `Endpoint` stays an oracle for the board rather than a
//! second implementation (the plan's structural decision 1).

use alloc::vec::Vec;

use heapless::Vec as BoundedVec;

use crate::address_book::AddressBook;
use crate::announce::AnnounceBlob;
use crate::announce_freshness::AnnounceFreshness;
use crate::hash::{AddressHash, NameHash};
use crate::identity::PrivateIdentity;
use crate::link::{Link, PendingLink};
use crate::link_liveness::Liveness;
use crate::packet::Packet;
use crate::resource_transfer::{SegmentedReceiver, SegmentedSender};

mod action;
mod ingest;
mod links;
mod params;
mod poll;
mod report;
mod resources;
mod routes;
mod session;
mod setup;
mod tables;
#[cfg(test)]
mod tests;
mod transit;

pub use action::{Action, Actions, InterfaceId};
pub use params::*;
pub use report::*;
pub(crate) use tables::is_deduplicated_link_context;
use tables::{HashGenerations, LinkBridge, ReverseEntry, Route};

/// An executor-neutral Reticulum node.
///
/// `PEERS` bounds the address book. `ACTIONS` bounds what one call can ask of the shell.
/// `ROUTES` bounds learned paths, recent transit hashes, and transport bridges. All default to
/// the board profile, because the desktop has `Endpoint` and does not want this type.
pub struct Node<
    const PEERS: usize = 32,
    const ACTIONS: usize = 8,
    const LINKS: usize = 4,
    const ROUTES: usize = 16,
> {
    identity: PrivateIdentity,
    /// The destination this node announces. One for now: a board is one thing.
    name_hash: NameHash,
    book: AddressBook,
    /// Application data carried in our announces.
    app_data: Vec<u8>,
    payload_limits: PayloadLimits,
    logical_mtu: u32,
    refused_payloads: u64,
    /// The explicit policy for carrying traffic whose destination is not this node.
    transport: TransportConfig,
    /// Paths learned from verified announces. The book holds the keys to open a link; a route
    /// says where a transport packet goes.
    routes: BoundedVec<Route, ROUTES>,
    /// Receive-side announce freshness: the blobs of the announces behind each live route. A
    /// destination without a live route has none, so its next announce is a first sighting.
    freshness: AnnounceFreshness,
    freshness_policy: FreshnessPolicy,
    /// Link ids this node is carrying, with their ingress and egress interfaces. Proofs and
    /// link data name the link id, not the destination, so this is how return traffic finds
    /// its way back.
    bridges: BoundedVec<LinkBridge, ROUTES>,
    /// Return paths for the proofs of carried packets, consumed by the proof that uses them
    /// and forgotten after [`REVERSE_TIMEOUT`]. The oldest gives way at capacity.
    reverse: BoundedVec<ReverseEntry, ROUTES>,
    /// Recently relayed packet hashes, so a shared radio hearing its own relay does not loop.
    transit_filter: HashGenerations<TRANSPORT_DEDUP_HASHES>,
    /// Path requests already seen, by target and tag, so each is answered once.
    path_request_tags: HashGenerations<PATH_REQUEST_TAGS>,
    /// Hashes of the link data this node most recently sent, oldest first. A relay's copy of
    /// our packet has the same hash (it excludes hops and header type), so it is known as ours.
    sent_link_data: BoundedVec<AddressHash, { crate::capacity::small::OWN_ECHO_HASHES }>,
    /// Hashes of the link packets most recently received from far ends, oldest first, so a
    /// copy heard both directly and from a relay is dropped the second time.
    received_link_data: BoundedVec<AddressHash, { crate::capacity::small::DUPLICATE_HASHES }>,
    /// When we last announced, and how often to. `None` until the first poll, so a node
    /// announces promptly on boot rather than waiting a full interval.
    last_announce: Option<u64>,
    announce_interval: u64,
    /// The blob of the announce [`Node::poll`] last emitted. A path request for this node is
    /// answered with it, since this layer cannot mint a fresh one; the requester has no live
    /// route to us, so it is a first sighting there, as RNS's cached path responses are.
    announced_blob: Option<AnnounceBlob>,
    /// Established links, each with the proof that established it and its liveness timers.
    /// The proof is kept so a retransmitted request gets the *same* proof: answering afresh
    /// would leave the two sides with different keys for one link.
    links: BoundedVec<(Link, Packet, Liveness), LINKS>,
    /// The interface each link was established on, which its packets must arrive by
    /// (`Link.py` 938-941). Entries for links since dropped are pruned on the next bind.
    link_interfaces: BoundedVec<(AddressHash, InterfaceId), LINKS>,
    /// Links we opened, awaiting the peer's proof, each with the time it expires unanswered
    /// and the time it was sent, from which the proof's arrival measures the link RTT.
    pending: BoundedVec<(PendingLink, u64, u64), LINKS>,
    /// Per-interface first-hop airtime allowances, added to a request's deadline. An
    /// interface with no entry gets none.
    first_hop_airtime: BoundedVec<(InterfaceId, u64), FIRST_HOP_AIRTIME_INTERFACES>,
    /// Interfaces whose mode is not [`InterfaceMode::Full`].
    interface_modes: BoundedVec<(InterfaceId, InterfaceMode), INTERFACE_MODE_INTERFACES>,
    /// Inbound resource transfers, at most one per link.
    receivers: BoundedVec<(AddressHash, SegmentedReceiver, u64), LINKS>,
    /// The largest inbound resource accepted, in all; see [`Node::set_max_inbound_resource`].
    max_inbound_resource: usize,
    /// Outbound resource transfers, at most one per link.
    senders: BoundedVec<(AddressHash, SegmentedSender<Vec<u8>>, u64), LINKS>,
    /// The last resource proof sent on each link, with when and how many times it has been
    /// re-sent, kept for [`RESOURCE_PROOF_CACHE_TTL`] to answer the sender's cache request
    /// (or a re-advertisement) if it was lost.
    resource_proofs: BoundedVec<(AddressHash, Packet, u64, u8), LINKS>,
    /// Counter feeding derived resource IVs. Node state, never reset, because an IV must not
    /// repeat under a link key.
    iv_counter: u32,
    /// Link requests refused because the table was full.
    refused_links: u16,
    /// Slots reclaimed from peers that went silent.
    expired_links: u16,
    /// Link requests dropped unanswered at their deadline.
    expired_link_requests: u16,
    /// Announces whose identity a full address book turned away.
    refused_peers: u16,
    /// Resource offers refused: past the part ceiling, multi-segment, past the decompression
    /// limit, or with every receiver slot held.
    refused_offers: u16,
    transport_counters: TransportCounters,
}
