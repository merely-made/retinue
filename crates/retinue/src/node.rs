//! The executor-neutral node: the shape a board runs.
//!
//! [`Endpoint`](crate::endpoint) is the desktop shell. It owns tokio tasks, unbounded
//! channels, sockets and a clock, none of which a 256 KB board has. This is the same
//! protocol work with the shell removed:
//!
//! ```text
//! node.ingest(interface, packet, now)       -> Actions
//! node.poll(now, interface, announce_blob?) -> Actions
//! ```
//!
//! Nothing here reads a clock, allocates without a bound, or performs I/O. Time arrives as
//! a `now` argument, and announce ordinals arrive as caller-supplied [`AnnounceBlob`] values.
//! Everything the node wants to happen leaves as an [`Action`] for a shell
//! to carry out. That is what makes it testable at a desk and runnable under embassy
//! without either knowing about the other.
//!
//! # Why this is not a second implementation
//!
//! The node calls the same `announce`, `link`, `channel` and `resource` code the desktop
//! calls, at the small capacity profile instead of the large one. If it re-implemented any
//! of that, `Endpoint` would stop being an oracle for the board and become a different
//! program that merely interoperates. See the plan's structural decision 1.

use alloc::vec::Vec;

use heapless::Vec as BoundedVec;

use crate::address_book::{AddressBook, Ingested};
use crate::announce::{self, Announce, AnnounceBlob, RATCHET_LEN};
use crate::announce_freshness::{
    AnnounceFreshness, AnnounceFreshnessCandidate, AnnounceFreshnessConfig,
    AnnounceFreshnessDecision, AnnounceFreshnessReject,
};
use crate::hash::{AddressHash, NameHash};
use crate::identity::PrivateIdentity;
use crate::link::{self, Inbound, Link, LinkMode, LinkTrailer, PendingLink};
use crate::packet::{HeaderType, Packet, PacketType};
use crate::resource_transfer::{ResourceReceiver, ResourceSender};

/// Which interface a packet arrived on or should leave by.
///
/// A plain integer chosen by the shell, matching the desktop's `InterfaceId`, so a board
/// with one radio and one host link can simply number them.
pub type InterfaceId = u32;

/// Something the node wants the shell to do.
///
/// The node never acts; it decides. A shell reads these and performs them with whatever
/// radio, timer and link it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Put this packet on the wire, by this interface.
    Send {
        interface: InterfaceId,
        packet: Packet,
    },
    /// A destination was learned or refreshed from a valid announce. The shell may show it
    /// on a face or hand it to an application; the node has already recorded it.
    Learned { destination: AddressHash },
    /// A link is established, in either direction. The shell may now carry data on it.
    LinkUp { link_id: AddressHash },
    /// An established link ended, because the peer closed it or the node dropped it.
    LinkDown { link_id: AddressHash },
    /// A link request this node opened got no proof by its deadline (see
    /// [`link_request_timeout`]) and was dropped. That link never came up, so this is not a
    /// [`Action::LinkDown`]. The id is the one `link::link_id` reads from the request.
    ///
    /// Stricter than RNS 1.5.4, which reports the same TIMEOUT reason for a request that was
    /// never answered and for an established link later lost
    /// (`testing/receipts/rns-1.5.4-link-echo-corroboration`, Q1).
    LinkRequestTimedOut { link_id: AddressHash },
    /// Application bytes arrived on a link, already decrypted.
    Data {
        link_id: AddressHash,
        payload: Vec<u8>,
    },
    /// A resource arrived whole, reassembled and verified against its advertised hash.
    Resource { link_id: AddressHash, data: Vec<u8> },
}

/// What one `ingest` or `poll` produced.
///
/// Bounded, because a single call must never be able to demand unbounded work of a shell
/// that has 256 KB. `overflowed` reports honestly when the bound was reached rather than
/// silently dropping, per the plan's rule that a full table stays operational and says so.
#[derive(Debug)]
pub struct Actions<const N: usize> {
    items: BoundedVec<Action, N>,
    overflowed: u16,
}

impl<const N: usize> Actions<N> {
    fn new() -> Self {
        Self {
            items: BoundedVec::new(),
            overflowed: 0,
        }
    }

    fn push(&mut self, action: Action) -> bool {
        if self.items.push(action).is_err() {
            self.overflowed = self.overflowed.saturating_add(1);
            false
        } else {
            true
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Action> {
        self.items.iter()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Actions that did not fit. Nonzero means the shell is not draining fast enough, or
    /// `ACTIONS` is too small for this traffic.
    pub fn overflowed(&self) -> u16 {
        self.overflowed
    }
}

impl<const N: usize> IntoIterator for Actions<N> {
    type Item = Action;
    type IntoIter = <BoundedVec<Action, N> as IntoIterator>::IntoIter;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}

/// How often this node re-announces itself, in the caller's tick unit.
///
/// A board on a shared band should not announce often; this is a starting cadence a shell
/// can override, not a protocol constant.
pub const DEFAULT_ANNOUNCE_INTERVAL: u64 = 600_000;

/// The link MTU this node offers.
///
/// 255, the SX1262's frame size, because the trunk is retinue-to-retinue over direct PHY.
/// Carrying stock RNS's 500 over the air needs the long-packet fragmentation lane, which
/// belongs to the RNode personality; see the plan's pressure point 4.
pub const LINK_MTU: u32 = 255;

/// Minimum logical budget, including a plain announce without application data.
pub const MIN_LOGICAL_MTU: u32 = (crate::packet::HEADER_MIN_LEN
    + crate::identity::IDENTITY_LEN
    + crate::hash::NAME_HASH_LEN
    + announce::RAND_HASH_LEN
    + crate::identity::SIGNATURE_LEN) as u32;

/// Why a logical packet budget cannot be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalMtuError {
    OutOfRange,
    SessionsActive,
    AppDataTooLarge,
}

impl core::fmt::Display for LogicalMtuError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            Self::OutOfRange => "logical MTU outside supported radio range",
            Self::SessionsActive => "logical MTU cannot change while sessions are active",
            Self::AppDataTooLarge => "announce application data exceeds logical MTU",
        })
    }
}
impl core::error::Error for LogicalMtuError {}

/// The most parts this node will accept for one inbound resource.
///
/// A sender chooses the advertised part count, so this is where a peer's ambition stops
/// being the board's problem. Thirty-two parts is roughly 13 KB of reassembly at the
/// default part size, which a 256 KB board can hold while a desktop's 4096-part ceiling
/// (about 1.7 MB) it plainly cannot.
pub const MAX_RESOURCE_PARTS: usize = 32;

/// Payload budgets for one Node. Table counts remain its const parameters.
///
/// The default preserves existing host callers. Embedded callers should choose
/// finite values with [`Node::new_with_payload_limits`]. The caller must also
/// bound raw input before decoding a [`Packet`] and bound retained action queues.
/// Inbound uncompressed resources are bounded by `max_resource_parts` times
/// `max_ingress_bytes`. With `compression` on, a compressed resource is also refused once
/// it inflates past [`DEFAULT_MAX_DECOMPRESSED_SIZE`](crate::resource::DEFAULT_MAX_DECOMPRESSED_SIZE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PayloadLimits {
    pub max_ingress_bytes: usize,
    pub max_app_data: usize,
    pub max_link_payload: usize,
    pub max_outbound_resource: usize,
    pub max_resource_parts: usize,
}

impl Default for PayloadLimits {
    fn default() -> Self {
        Self {
            max_ingress_bytes: usize::MAX,
            max_app_data: usize::MAX,
            max_link_payload: usize::MAX,
            max_outbound_resource: usize::MAX,
            max_resource_parts: MAX_RESOURCE_PARTS,
        }
    }
}

/// Caller-supplied application data exceeds this Node's configured budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AppDataTooLarge;

impl core::fmt::Display for AppDataTooLarge {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("node application data exceeds configured limit")
    }
}

impl core::error::Error for AppDataTooLarge {}

/// Parts requested per turn. Small, because a half-duplex radio should not be asked for a
/// burst it cannot answer before the next request arrives.
pub const RESOURCE_REQUEST_WINDOW: usize = 4;

/// How long a transfer may sit silent before [`Node::poll`] redrives it, in the caller's
/// tick unit (milliseconds on the boards).
///
/// This is the loss-recovery clock: a receiver re-requests what it is missing, a sender
/// re-advertises an offer nobody answered. Without it, one lost frame is a dead transfer —
/// which is exactly how N5's first hardware run failed. It must clear a request-plus-part
/// round trip at the slowest profile (about 3 s at SF11/250 kHz); deriving it from the
/// profile's airtime is the same recorded follow-up as the desktop's retry floors.
pub const RESOURCE_RETRY_INTERVAL: u64 = 12_000;

/// How long a link may go unheard before its slot is reclaimed, in milliseconds.
///
/// A board holds four link slots. Without expiry, four peers that establish a link and then
/// go quiet -- moved out of range, lost power, crashed -- hold every slot until one of them
/// politely closes or the board reboots, and a peer that vanished will not be doing the
/// former. That is a node bricked as a router by four absences, and on a pilot site nobody
/// is there to power-cycle it.
///
/// Fifteen minutes is long enough that an idle but live peer is not evicted (RNS keepalives
/// run far tighter than this), and short enough that a slot lost to a vanished peer comes
/// back within one visit.
pub const LINK_IDLE_TIMEOUT: u64 = 900_000;

/// How long a link request waits for its proof, per hop to the destination, in milliseconds.
///
/// Reticulum's `Link.ESTABLISHMENT_TIMEOUT_PER_HOP`, 6 s (the manual, as recorded in the wire
/// format reference, section 2.5). [`link_request_timeout`] composes it.
pub const LINK_ESTABLISHMENT_TIMEOUT_PER_HOP: u64 = 6_000;

/// How long a link request to a destination `relays` relays away waits for its proof before
/// [`Node::poll`] drops it, in milliseconds.
///
/// One per-hop allowance for the first hop, plus one for each hop to the destination
/// (`relays + 1`). A neighbour, or a destination with no route, gets 12 s; two relays get
/// 24 s. This is the base deadline: [`Node::open_link`] adds the outgoing interface's
/// [`Node::first_hop_airtime`], zero unless the caller set one.
///
/// RNS 1.5.4 was observed to wait 12.001, 18.001, 24.001 and 30.001 s at zero to three
/// relays on an unbounded TCP interface, sending the request once and never retrying
/// (`testing/receipts/rns-1.5.4-link-echo-corroboration`, Q1).
///
/// Without a deadline, requests nobody answers hold their pending slots for good: at the
/// board's four, four lost requests refuse every later `open_link`.
pub const fn link_request_timeout(relays: u8) -> u64 {
    LINK_ESTABLISHMENT_TIMEOUT_PER_HOP * (relays as u64 + 2)
}

/// The bits a first hop is allowed airtime for: one 500-byte Reticulum MTU.
///
/// RNS 1.5.4's first-hop extra was observed as `500 × 8 / bitrate` seconds: 12.079 s at
/// 62,500 bps against 12.001 s on unbounded TCP, and 24.071 s at two relays (the receipt
/// above, Q1). [`first_hop_airtime`] turns it into milliseconds for a known bitrate.
pub const FIRST_HOP_ALLOWANCE_BITS: u64 = 500 * 8;

/// The first-hop airtime allowance at `bitrate` bits per second, in milliseconds, rounded
/// up: 64 ms at 62,500 bps. Zero for a bitrate of zero, meaning unbounded, as TCP is.
pub const fn first_hop_airtime(bitrate: u64) -> u64 {
    if bitrate == 0 {
        0
    } else {
        (FIRST_HOP_ALLOWANCE_BITS * 1_000).div_ceil(bitrate)
    }
}

/// How many interfaces can carry a first-hop airtime allowance at once.
pub const FIRST_HOP_AIRTIME_INTERFACES: usize = 4;

/// [`Node::set_first_hop_airtime`] refused a new interface: every slot holds another one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AirtimeTableFull;

impl core::fmt::Display for AirtimeTableFull {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("first-hop airtime table full")
    }
}
impl core::error::Error for AirtimeTableFull {}
/// How long a learned transport route is usable, in the caller's tick unit.
///
/// A board that hears a peer once must not retain that route forever. Thirty minutes leaves
/// room for the ten-minute announce cadence, while making a disappeared peer's path become
/// eligible for replacement during one field visit.
pub const DEFAULT_ROUTE_TTL: u64 = 1_800_000;

/// Bounds for the receive-side announce freshness table.
///
/// The table is runtime state rather than a const-generic part of [`Node`], so firmware can
/// choose a smaller footprint and a desktop caller can choose a larger one without making a
/// second node type. The defaults are intentionally aligned with the node's peer budget and
/// keep eight accepted blobs per destination. A destination's freshness lives exactly as long
/// as its route, as RNS keeps announce blobs on the path-table row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessPolicy {
    /// Maximum destination rows retained by the freshness table. Evicting a row drops its
    /// route, so this also bounds how many routes keep replay protection.
    pub max_destinations: usize,
    /// Maximum accepted full announce blobs retained per destination.
    pub max_blobs_per_destination: usize,
}

impl FreshnessPolicy {
    /// Freshness belongs to routes, and evicting a row drops its route, so the table must
    /// never be the tighter bound: it covers every route (and every known peer). The route
    /// table's own eviction then decides which route goes. A row left behind for an evicted
    /// route is harmless, because without a live route an announce is a first sighting.
    pub const fn for_node(peers: usize, routes: usize) -> Self {
        Self::for_peers(if peers > routes { peers } else { routes })
    }

    pub const fn for_peers(peers: usize) -> Self {
        Self {
            // `AddressBook` can be instantiated with PEERS == 0 for a deliberately
            // non-learning node. Keep Node::new infallible while retaining a valid internal
            // freshness table; the zero-capacity address book still refuses every announce.
            max_destinations: if peers == 0 { 1 } else { peers },
            max_blobs_per_destination: 8,
        }
    }
}

/// How long a carried link remains bridgeable after it last carries traffic.
///
/// A link's own keepalives are considerably more frequent than this. The longer interval
/// avoids discarding a quiet but live remote link while still bounding stale transport state.
pub const LINK_TRANSPORT_TIMEOUT: u64 = 3_600_000;

/// How long this node remembers a forwarded packet hash on a shared radio.
///
/// A single-radio transport retransmits on the carrier it heard. Remembering a packet briefly
/// prevents its own relay from becoming a flood loop while still allowing a normal retry later.
pub const TRANSPORT_DEDUP_TIMEOUT: u64 = 60_000;

/// The Reticulum transport hop ceiling.
pub const DEFAULT_TRANSPORT_MAX_HOPS: u8 = 128;

/// How long a carried packet's return path is kept for its delivery proof (RNS
/// `Transport.REVERSE_TIMEOUT`, eight minutes).
pub const REVERSE_TIMEOUT: u64 = 480_000;

/// What this node agrees to carry for other destinations.
///
/// Transport is explicit because many boards are endpoints, not routers. The firmware can opt
/// in to transit without changing the behaviour of a desk fixture or an application node that
/// only answers for itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportConfig {
    /// Re-broadcast verified announces with this node as their next transport hop.
    pub relay_announces: bool,
    /// Carry header-type-2 packets addressed to this node, and packets on remembered links.
    pub relay_packets: bool,
    /// Packets at or above this hop count are dropped instead of relayed.
    pub max_hops: u8,
    /// Lifetime of a route learned from a verified announce.
    pub route_ttl: u64,
    /// Lifetime of a remembered carried link.
    pub bridge_ttl: u64,
}

impl TransportConfig {
    /// The default: carry nothing for other destinations.
    pub const fn none() -> Self {
        Self {
            relay_announces: false,
            relay_packets: false,
            max_hops: 0,
            route_ttl: DEFAULT_ROUTE_TTL,
            bridge_ttl: LINK_TRANSPORT_TIMEOUT,
        }
    }

    /// Carry verified announces and transit packets up to Reticulum's normal hop ceiling.
    pub const fn transit() -> Self {
        Self {
            relay_announces: true,
            relay_packets: true,
            max_hops: DEFAULT_TRANSPORT_MAX_HOPS,
            route_ttl: DEFAULT_ROUTE_TTL,
            bridge_ttl: LINK_TRANSPORT_TIMEOUT,
        }
    }
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self::none()
    }
}

/// What the bounded transport, freshness, and link-packet tables have done since this node
/// started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TransportCounters {
    /// Verified announces re-broadcast for another destination.
    pub forwarded_announces: u16,
    /// Data, link, and proof packets carried for another destination.
    pub forwarded_packets: u16,
    /// Routes removed after their announce freshness expired.
    pub expired_routes: u16,
    /// Live routes evicted to admit a newly heard destination.
    pub evicted_routes: u16,
    /// Carried-link entries removed after their idle timeout.
    pub expired_bridges: u16,
    /// Carried-link entries evicted to admit a newer transport link.
    pub evicted_bridges: u16,
    /// Transit dropped at the configured hop ceiling.
    pub hop_limit_dropped: u16,
    /// Transit that named this node but had no fresh route onward.
    pub unroutable_packets: u16,
    /// Valid announces rejected because their full blob is in the live route's history.
    pub replayed_announces: u16,
    /// Valid announces rejected because the live route already holds a no-older emission.
    pub stale_announces: u16,
    /// Freshness destination rows evicted under the configured capacity bound, with their
    /// routes.
    pub evicted_freshness_rows: u16,
    /// Accepted announce blobs evicted from per-destination history under the configured capacity.
    pub evicted_freshness_blobs: u16,
    /// Our own link data heard back from a relay, and dropped as ours.
    pub own_echo_dropped: u16,
    /// Far-end link packets heard again, directly or from a relay, and dropped as copies.
    pub duplicate_dropped: u16,
    /// Validly signed announces rejected because they name a known destination under a
    /// different public key.
    pub key_mismatch_announces: u16,
}

/// Side-effect-free local state relevant to pausing this node's radio.
///
/// These are local protocol facts, not a promise that a remote peer will keep a
/// link while this node is away. A caller supplies its own monotonic millisecond
/// clock to [`Self::can_pause_through`] and must still drain every already-issued
/// [`Action`] before changing the radio personality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PauseAssessment {
    /// Link requests awaiting a proof. They are not retried, and expire at their deadline
    /// (see [`link_request_timeout`]).
    pub pending_handshakes: usize,
    /// Inbound resource reassemblies that still need radio traffic.
    pub inbound_resources: usize,
    /// Outbound resource publications that still need radio traffic.
    pub outbound_resources: usize,
    /// Transit-link return-path records that still name this radio interface.
    pub transit_bridges: usize,
    /// Earliest established-link idle expiry, when every `last_seen + timeout`
    /// calculation fits in the caller's monotonic clock domain.
    pub earliest_link_expiry: Option<u64>,
    /// Latest observed activity on any retained established link. A caller
    /// whose clock precedes this value cannot safely assess a pause.
    pub latest_link_activity: Option<u64>,
    /// An established link's idle expiry overflowed `u64`; fail closed rather
    /// than treating that link as indefinitely safe to pause.
    pub link_expiry_overflow: bool,
}

/// Why [`PauseAssessment::can_pause_through`] refuses a proposed return bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PauseBlocked {
    ReturnBoundBeforeNow { now: u64, return_by: u64 },
    ClockBeforeLinkActivity { now: u64, latest_activity: u64 },
    PendingHandshakes { count: usize },
    ActiveResources { inbound: usize, outbound: usize },
    ActiveTransitBridges { count: usize },
    LinkExpiryOverflow,
    LinkExpiresAt { expiry: u64, return_by: u64 },
}

/// Caller admission for a destructive, Node-wide interruption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterruptionPermission {
    PreserveSessions,
    AllowSessionLoss,
}

/// A refused interruption leaves all protocol state untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionLossNotPermitted;

/// Session state removed by ordinary elapsed-time maintenance. No packets are
/// transmitted by expiry, and identities/freshness counters remain intact.
#[derive(Debug, Default)]
pub struct SessionExpiryReport<const LINKS: usize> {
    pub links: BoundedVec<AddressHash, LINKS>,
    /// Link requests this node opened that went unanswered past their deadline.
    pub pending_links: BoundedVec<AddressHash, LINKS>,
    pub inbound_resources: BoundedVec<AddressHash, LINKS>,
    pub outbound_resources: BoundedVec<AddressHash, LINKS>,
}

/// Complete local loss report, independent of the ordinary action queue capacity.
///
/// Close packets are best effort. The caller chooses their interfaces and must
/// record any it cannot transmit. They do not acknowledge remote termination.
#[derive(Debug)]
pub struct InterruptionReport<const LINKS: usize, const ROUTES: usize> {
    pub closed_links: BoundedVec<AddressHash, LINKS>,
    pub close_packets: BoundedVec<Packet, LINKS>,
    pub pending_links: BoundedVec<AddressHash, LINKS>,
    pub inbound_resources: BoundedVec<AddressHash, LINKS>,
    pub outbound_resources: BoundedVec<AddressHash, LINKS>,
    pub transit_bridges: BoundedVec<AddressHash, ROUTES>,
}

impl PauseAssessment {
    /// Whether the node has no local obligation that would be interrupted before
    /// `return_by`. The comparison is strict: a link expiring exactly at the
    /// promised return time is not resumable. This method does not mutate node
    /// state or stop its timers.
    pub fn can_pause_through(self, now: u64, return_by: u64) -> Result<(), PauseBlocked> {
        if return_by < now {
            return Err(PauseBlocked::ReturnBoundBeforeNow { now, return_by });
        }
        if let Some(latest_activity) = self.latest_link_activity
            && now < latest_activity
        {
            return Err(PauseBlocked::ClockBeforeLinkActivity {
                now,
                latest_activity,
            });
        }
        if self.pending_handshakes != 0 {
            return Err(PauseBlocked::PendingHandshakes {
                count: self.pending_handshakes,
            });
        }
        if self.inbound_resources != 0 || self.outbound_resources != 0 {
            return Err(PauseBlocked::ActiveResources {
                inbound: self.inbound_resources,
                outbound: self.outbound_resources,
            });
        }
        if self.transit_bridges != 0 {
            return Err(PauseBlocked::ActiveTransitBridges {
                count: self.transit_bridges,
            });
        }
        if self.link_expiry_overflow {
            return Err(PauseBlocked::LinkExpiryOverflow);
        }
        if let Some(expiry) = self.earliest_link_expiry
            && expiry <= return_by
        {
            return Err(PauseBlocked::LinkExpiresAt { expiry, return_by });
        }
        Ok(())
    }
}

/// Where a fresh route leaves this node, as [`Node::next_hop`] reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NextHop {
    /// The interface the route was learned on.
    pub interface: InterfaceId,
    /// The identity hash of the transport node that relayed the announce, or `None` when the
    /// destination was heard directly. A request to the destination is addressed to it.
    pub via: Option<AddressHash>,
    /// The announce's hop count on arrival: the number of relays between here and there.
    pub hops: u8,
}

#[derive(Debug, Clone, Copy)]
struct Route {
    destination: AddressHash,
    interface: InterfaceId,
    /// The next transport hop that announced this destination, if it is not direct.
    transport: Option<AddressHash>,
    hops: u8,
    learned: u64,
}

#[derive(Debug, Clone, Copy)]
struct LinkBridge {
    link_id: AddressHash,
    from: InterfaceId,
    out: InterfaceId,
    seen: u64,
}

/// The way back for a carried packet's proof: RNS's reverse-table entry, keyed by the
/// truncated packet hash the proof is addressed to.
#[derive(Debug, Clone, Copy)]
struct ReverseEntry {
    packet: AddressHash,
    received: InterfaceId,
    outbound: InterfaceId,
    seen: u64,
}

#[derive(Debug, Clone, Copy)]
struct SeenPacket {
    hash: AddressHash,
    seen: u64,
}

/// One derived resource IV: `full_hash(tag || identity secret || link id || counter)`.
///
/// Deterministic on purpose — this layer holds no RNG — and unique by the counter, which
/// the node owns and never resets.
fn derived_iv(
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
fn is_resource_context(context: u8) -> bool {
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
/// link is the same unencrypted byte, so each one repeats the last one's hash.
pub(crate) fn is_deduplicated_link_context(context: u8) -> bool {
    !is_resource_context(context) && context != link::CTX_KEEPALIVE
}

/// Remember a packet hash in a window, forgetting the oldest at capacity. A burst can outrun
/// the window; that only lets a late copy through, never drops a new packet.
fn remember_hash<const N: usize>(window: &mut BoundedVec<AddressHash, N>, hash: AddressHash) {
    if window.is_full() {
        window.remove(0);
    }
    let _ = window.push(hash);
}

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
    /// Paths learned from verified announces. This is separate from the address book: the book
    /// has keys needed to initiate a link, while a route says where a transport packet goes.
    routes: BoundedVec<Route, ROUTES>,
    /// Receive-side announce freshness: the blobs of the announces behind each live route. A
    /// destination without a live route has none, so its next announce is a first sighting.
    freshness: AnnounceFreshness,
    freshness_policy: FreshnessPolicy,
    /// Link ids this node is carrying, with their ingress and egress interfaces. A proof or
    /// link-data packet names a link id rather than its original destination, so this is the
    /// small fact that lets return traffic take the same bridge back.
    bridges: BoundedVec<LinkBridge, ROUTES>,
    /// Return paths for the proofs of carried packets, consumed by the proof that uses them
    /// and forgotten after [`REVERSE_TIMEOUT`]. The oldest gives way at capacity.
    reverse: BoundedVec<ReverseEntry, ROUTES>,
    /// Recently relayed packet hashes. Bounded and time-limited because a shared radio hears
    /// its own relays; without this, one transport node can keep repeating the same frame.
    seen_transit: BoundedVec<SeenPacket, ROUTES>,
    /// Hashes of the link data this node most recently sent, oldest first. On a shared
    /// medium a relay's retransmission of our own packet reaches us under the shared link
    /// key; its packet hash excludes hops and header type, so it matches what we sent and
    /// marks the copy as ours rather than the far end's. Sized by
    /// [`capacity::small::OWN_ECHO_HASHES`](crate::capacity::small::OWN_ECHO_HASHES).
    sent_link_data: BoundedVec<AddressHash, { crate::capacity::small::OWN_ECHO_HASHES }>,
    /// Hashes of the link packets most recently received from far ends, oldest first. The
    /// same medium that echoes our packets hands us theirs twice, directly and from a relay,
    /// under one hash; the second copy is dropped. Sized by
    /// [`capacity::small::DUPLICATE_HASHES`](crate::capacity::small::DUPLICATE_HASHES).
    received_link_data: BoundedVec<AddressHash, { crate::capacity::small::DUPLICATE_HASHES }>,
    /// When we last announced, and how often to. `None` until the first poll, so a node
    /// announces promptly on boot rather than waiting a full interval.
    last_announce: Option<u64>,
    announce_interval: u64,
    /// Established links, each with the proof that established it.
    ///
    /// The proof is kept so a retransmitted request is answered with the *same* proof
    /// rather than establishing a second link. On a medium that drops, the peer not hearing
    /// our proof is ordinary, and answering twice would leave the two sides holding
    /// different keys for what the initiator thinks is one link.
    /// Established links, each with the time its peer was last heard from.
    links: BoundedVec<(Link, Packet, u64), LINKS>,
    /// Links we opened, awaiting the peer's proof, each with the time it expires unanswered.
    pending: BoundedVec<(PendingLink, u64), LINKS>,
    /// Per-interface first-hop airtime allowances, added to a request's deadline. An
    /// interface with no entry gets none.
    first_hop_airtime: BoundedVec<(InterfaceId, u64), FIRST_HOP_AIRTIME_INTERFACES>,
    /// Inbound resource transfers, at most one per link.
    receivers: BoundedVec<(AddressHash, ResourceReceiver, u64), LINKS>,
    /// Outbound resource transfers, at most one per link.
    senders: BoundedVec<(AddressHash, ResourceSender, u64), LINKS>,
    /// Counter feeding derived resource IVs. Node state rather than a per-call local so the
    /// sequence never restarts: an IV must not repeat under a link key, and a counter that
    /// reset on every ingest repeated the whole sequence on every ingest.
    iv_counter: u32,
    /// Link requests refused because the table was full. Visible rather than silent.
    refused_links: u16,
    /// Slots reclaimed from peers that went silent. Distinguishes a busy node from one
    /// whose peers keep vanishing, which need different answers.
    expired_links: u16,
    /// Link requests dropped unanswered at their deadline.
    expired_link_requests: u16,
    /// Announces refused because the address book was full. The book keeps serving every
    /// peer it already knows; this says how many new ones were turned away.
    refused_peers: u16,
    /// Resource offers refused: an advertisement past the part ceiling or naming several
    /// segments, a body past the decompression limit, or arriving with every receiver slot
    /// held. The peer's ambition, counted rather than honoured.
    refused_offers: u16,
    transport_counters: TransportCounters,
}

impl<const PEERS: usize, const ACTIONS: usize, const LINKS: usize, const ROUTES: usize>
    Node<PEERS, ACTIONS, LINKS, ROUTES>
{
    /// A node with an identity and the destination it answers to.
    pub fn new(identity: PrivateIdentity, name_hash: NameHash) -> Self {
        Self {
            identity,
            name_hash,
            book: AddressBook::with_max_peers(PEERS),
            app_data: Vec::new(),
            payload_limits: PayloadLimits::default(),
            logical_mtu: LINK_MTU,
            refused_payloads: 0,
            transport: TransportConfig::none(),
            routes: BoundedVec::new(),
            freshness_policy: FreshnessPolicy::for_node(PEERS, ROUTES),
            freshness: AnnounceFreshness::new(AnnounceFreshnessConfig {
                destination_capacity: FreshnessPolicy::for_node(PEERS, ROUTES).max_destinations,
                blob_capacity: 8,
            })
            .expect("nonzero fallback freshness capacity"),
            bridges: BoundedVec::new(),
            reverse: BoundedVec::new(),
            seen_transit: BoundedVec::new(),
            sent_link_data: BoundedVec::new(),
            received_link_data: BoundedVec::new(),
            last_announce: None,
            announce_interval: DEFAULT_ANNOUNCE_INTERVAL,
            links: BoundedVec::new(),
            pending: BoundedVec::new(),
            first_hop_airtime: BoundedVec::new(),
            receivers: BoundedVec::new(),
            senders: BoundedVec::new(),
            iv_counter: 0,
            refused_links: 0,
            expired_links: 0,
            expired_link_requests: 0,
            refused_peers: 0,
            refused_offers: 0,
            transport_counters: TransportCounters::default(),
        }
    }

    /// Construct an empty Node with caller-selected payload budgets.
    pub fn new_with_payload_limits(
        identity: PrivateIdentity,
        name_hash: NameHash,
        limits: PayloadLimits,
    ) -> Self {
        let mut node = Self::new(identity, name_hash);
        node.payload_limits = limits;
        node
    }

    /// Logical packet bytes available after the carrier reserves its own envelope.
    pub fn logical_mtu(&self) -> u32 {
        self.logical_mtu
    }

    /// Whether negotiation, links, transfers or transit bridges still retain session state.
    /// Carriers use this to refuse changing credentials around an existing logical session.
    pub fn has_active_sessions(&self) -> bool {
        !self.links.is_empty()
            || !self.pending.is_empty()
            || !self.receivers.is_empty()
            || !self.senders.is_empty()
            || !self.bridges.is_empty()
    }

    /// Install a carrier budget before negotiation. Repeating the current value is safe.
    /// Changing it requires all links, pending requests, transfers and bridges to end.
    /// The carrier must separately reject oversized final frames, including relays.
    pub fn set_logical_mtu(&mut self, mtu: u32) -> Result<(), LogicalMtuError> {
        if !(MIN_LOGICAL_MTU..=LINK_MTU).contains(&mtu) {
            return Err(LogicalMtuError::OutOfRange);
        }
        if mtu == self.logical_mtu {
            return Ok(());
        }
        if self.has_active_sessions() {
            return Err(LogicalMtuError::SessionsActive);
        }
        if self.app_data.len() > mtu as usize - MIN_LOGICAL_MTU as usize {
            return Err(LogicalMtuError::AppDataTooLarge);
        }
        self.logical_mtu = mtu;
        Ok(())
    }

    pub fn payload_limits(&self) -> PayloadLimits {
        self.payload_limits
    }

    /// Oversized inbound packets, relay packets, announcements and outbound resource
    /// requests refused so far, plus inbound packets refused for carrying the IFAC flag.
    /// `send` is immutable and reports its refusal through `None`.
    pub fn refused_payloads(&self) -> u64 {
        self.refused_payloads
    }

    /// Replace announce data, refusing before allocation or mutation.
    pub fn try_set_app_data(&mut self, app_data: &[u8]) -> Result<(), AppDataTooLarge> {
        if app_data.len() > self.payload_limits.max_app_data
            || app_data.len() > self.logical_mtu as usize - MIN_LOGICAL_MTU as usize
        {
            return Err(AppDataTooLarge);
        }
        self.app_data = app_data.to_vec();
        Ok(())
    }

    /// Set the application data carried in our announces.
    ///
    /// Panics if it exceeds configured limits. Use [`Self::try_set_app_data`]
    /// for fallible application input.
    pub fn with_app_data(mut self, app_data: &[u8]) -> Self {
        self.try_set_app_data(app_data)
            .expect("node app data limit");
        self
    }

    /// Set the re-announce cadence, in the caller's tick unit.
    pub fn with_announce_interval(mut self, interval: u64) -> Self {
        self.announce_interval = interval;
        self
    }

    /// Configure the traffic this node will carry for other destinations.
    pub fn with_transport_config(mut self, config: TransportConfig) -> Self {
        self.transport = config;
        self
    }

    /// Configure receive-side announce freshness bounds.
    pub fn with_freshness_policy(
        mut self,
        policy: FreshnessPolicy,
    ) -> Result<Self, crate::announce_freshness::AnnounceFreshnessConfigError> {
        self.set_freshness_policy(policy)?;
        Ok(self)
    }

    /// Change receive-side freshness bounds without changing identity or transport policy.
    /// Retained rows are kept and history is trimmed deterministically; a destination whose
    /// row is evicted loses its route with it. Invalid zero capacities leave the old policy
    /// intact.
    pub fn set_freshness_policy(
        &mut self,
        policy: FreshnessPolicy,
    ) -> Result<
        crate::announce_freshness::AnnounceFreshnessReconfigure,
        crate::announce_freshness::AnnounceFreshnessConfigError,
    > {
        let report = self.freshness.reconfigure(AnnounceFreshnessConfig {
            destination_capacity: policy.max_destinations,
            blob_capacity: policy.max_blobs_per_destination,
        })?;
        self.routes
            .retain(|route| !report.evicted_destinations.contains(&route.destination));
        self.transport_counters.evicted_freshness_rows = self
            .transport_counters
            .evicted_freshness_rows
            .saturating_add(u16::try_from(report.evicted_destinations.len()).unwrap_or(u16::MAX));
        self.transport_counters.evicted_freshness_blobs = self
            .transport_counters
            .evicted_freshness_blobs
            .saturating_add(u16::try_from(report.evicted_blobs).unwrap_or(u16::MAX));
        self.freshness_policy = policy;
        Ok(report)
    }

    /// The current receive-side announce freshness policy.
    pub fn freshness_policy(&self) -> FreshnessPolicy {
        self.freshness_policy
    }

    /// Change the transport policy without replacing the node's learned state.
    pub fn set_transport_config(&mut self, config: TransportConfig) {
        self.transport = config;
    }

    /// The current transport policy.
    pub fn transport_config(&self) -> TransportConfig {
        self.transport
    }

    /// This node's own destination hash: what a peer addresses to reach it.
    pub fn destination(&self) -> AddressHash {
        crate::destination::destination_hash(self.name_hash, self.identity.hash())
    }

    /// The peers this node has heard announce.
    pub fn peers(&self) -> &AddressBook {
        &self.book
    }

    /// Established links.
    pub fn link_count(&self) -> usize {
        self.links.len()
    }

    /// Explicitly end all local sessions and transfer obligations.
    ///
    /// Before calling, the owner must admit session loss, finish any in-flight
    /// hardware operation, and cancel or account for every previously returned
    /// action. Such actions are caller-owned and cannot be revoked here; replaying
    /// them after interruption can create new remote work. Stop ingesting during
    /// the switch. New incoming requests after return can establish new sessions.
    ///
    /// Returns every affected ID and encrypted close packet without squeezing
    /// loss notifications into `ACTIONS`. Generate a fresh IV for each link using
    /// the caller's normal entropy source. Denied permission never calls it.
    /// Identity, peers, route/freshness history and the resource IV counter are
    /// retained. This operation neither freezes time nor reports a radio change.
    pub fn force_interrupt(
        &mut self,
        permission: InterruptionPermission,
        mut iv: impl FnMut() -> [u8; crate::token::IV_LEN],
    ) -> Result<InterruptionReport<LINKS, ROUTES>, SessionLossNotPermitted> {
        if permission != InterruptionPermission::AllowSessionLoss {
            return Err(SessionLossNotPermitted);
        }
        let mut report = InterruptionReport {
            closed_links: BoundedVec::new(),
            close_packets: BoundedVec::new(),
            pending_links: BoundedVec::new(),
            inbound_resources: BoundedVec::new(),
            outbound_resources: BoundedVec::new(),
            transit_bridges: BoundedVec::new(),
        };
        // Each output has exactly the capacity of its source table. Prepare the
        // complete report before mutation, so no ordinary Actions overflow can
        // hide a discarded session or transfer.
        for (link, _, _) in &self.links {
            report.closed_links.push(link.id()).expect("link bound");
            report
                .close_packets
                .push(link.close_packet(&iv()))
                .expect("link bound");
        }
        for (pending, _) in &self.pending {
            report
                .pending_links
                .push(pending.link_id())
                .expect("pending bound");
        }
        for (id, _, _) in &self.receivers {
            report.inbound_resources.push(*id).expect("receiver bound");
        }
        for (id, _, _) in &self.senders {
            report.outbound_resources.push(*id).expect("sender bound");
        }
        for bridge in &self.bridges {
            report
                .transit_bridges
                .push(bridge.link_id)
                .expect("bridge bound");
        }
        self.links.clear();
        self.pending.clear();
        self.receivers.clear();
        self.senders.clear();
        self.bridges.clear();
        Ok(report)
    }

    /// Inspect local work before asking a radio owner to pause this node.
    ///
    /// This neither polls nor expires state. In particular, established links
    /// retain their original `last_seen` timestamps while the caller is away;
    /// call [`PauseAssessment::can_pause_through`] with the caller's monotonic
    /// `now` and proposed return bound before admission, then call [`Self::poll`]
    /// normally after return. Remote peers may still expire independently.
    pub fn pause_assessment(&self) -> PauseAssessment {
        let mut earliest_link_expiry: Option<u64> = None;
        let mut latest_link_activity: Option<u64> = None;
        let mut link_expiry_overflow = false;
        for (_, _, last_seen) in &self.links {
            latest_link_activity = Some(match latest_link_activity {
                Some(current) => current.max(*last_seen),
                None => *last_seen,
            });
            match last_seen.checked_add(LINK_IDLE_TIMEOUT) {
                Some(expiry) => {
                    earliest_link_expiry = Some(match earliest_link_expiry {
                        Some(current) => current.min(expiry),
                        None => expiry,
                    });
                }
                None => link_expiry_overflow = true,
            }
        }
        PauseAssessment {
            pending_handshakes: self.pending.len(),
            inbound_resources: self.receivers.len(),
            outbound_resources: self.senders.len(),
            transit_bridges: self.bridges.len(),
            earliest_link_expiry,
            latest_link_activity,
            link_expiry_overflow,
        }
    }

    /// Number of fresh or not-yet-polled route entries currently held.
    pub fn route_count(&self) -> usize {
        self.routes.len()
    }

    /// A fresh route's radio interface and hop count. Lookup also evicts an expired entry, so
    /// a stale path does not linger until an unrelated new announce arrives.
    pub fn route_to(&mut self, destination: AddressHash, now: u64) -> Option<(InterfaceId, u8)> {
        self.expire_routes(now);
        self.routes
            .iter()
            .find(|route| route.destination == destination)
            .map(|route| (route.interface, route.hops))
    }

    /// A route's next hop, read-only. A route past its TTL is reported as absent but not
    /// evicted; [`Self::route_to`] and [`Self::poll`] do that.
    pub fn next_hop(&self, destination: AddressHash, now: u64) -> Option<NextHop> {
        self.routes
            .iter()
            .find(|route| {
                route.destination == destination
                    && now.saturating_sub(route.learned) < self.transport.route_ttl
            })
            .map(|route| NextHop {
                interface: route.interface,
                via: route.transport,
                hops: route.hops,
            })
    }

    /// Transport activity and bounded-state pressure since boot.
    pub fn transport_counters(&self) -> TransportCounters {
        self.transport_counters
    }

    /// Reconcile session expiry after an absence without generating radio work.
    /// Associated resource state is removed with its link, including orphaned
    /// entries left by earlier maintenance. A link request unanswered at its
    /// deadline is dropped and reported in `pending_links`.
    pub fn expire_sessions(&mut self, now: u64) -> SessionExpiryReport<LINKS> {
        let mut report = SessionExpiryReport {
            pending_links: self.expire_link_requests(now),
            ..Default::default()
        };
        self.links.retain(|(link, _, seen)| {
            let expired = now.saturating_sub(*seen) >= LINK_IDLE_TIMEOUT;
            if expired {
                let _ = report.links.push(link.id());
            }
            !expired
        });
        self.expired_links = self.expired_links.saturating_add(report.links.len() as u16);
        self.receivers.retain(|(id, _, _)| {
            let keep = self.links.iter().any(|(link, _, _)| link.id() == *id);
            if !keep {
                let _ = report.inbound_resources.push(*id);
            }
            keep
        });
        self.senders.retain(|(id, _, _)| {
            let keep = self.links.iter().any(|(link, _, _)| link.id() == *id);
            if !keep {
                let _ = report.outbound_resources.push(*id);
            }
            keep
        });
        report
    }

    /// The first-hop airtime allowance a link request leaving by `interface` adds to its
    /// deadline, in milliseconds. Zero unless [`Self::set_first_hop_airtime`] set one.
    pub fn first_hop_airtime(&self, interface: InterfaceId) -> u64 {
        self.first_hop_airtime
            .iter()
            .find(|(id, _)| *id == interface)
            .map_or(0, |(_, allowance)| *allowance)
    }

    /// Set the first-hop airtime allowance for requests leaving by `interface`, in
    /// milliseconds; [`first_hop_airtime`] computes one from a bitrate. A radio shell sets
    /// it from its modulation, and an unbounded link such as TCP leaves it at zero. Zero
    /// clears the entry. Requests already pending keep the deadline they were given.
    pub fn set_first_hop_airtime(
        &mut self,
        interface: InterfaceId,
        allowance: u64,
    ) -> Result<(), AirtimeTableFull> {
        let existing = self
            .first_hop_airtime
            .iter()
            .position(|(id, _)| *id == interface);
        match (existing, allowance) {
            (Some(index), 0) => {
                self.first_hop_airtime.swap_remove(index);
            }
            (Some(index), _) => self.first_hop_airtime[index].1 = allowance,
            (None, 0) => {}
            (None, _) => self
                .first_hop_airtime
                .push((interface, allowance))
                .map_err(|_| AirtimeTableFull)?,
        }
        Ok(())
    }

    /// Drop link requests unanswered at their deadline, returning their link ids.
    fn expire_link_requests(&mut self, now: u64) -> BoundedVec<AddressHash, LINKS> {
        let mut expired_ids = BoundedVec::new();
        self.pending.retain(|(attempt, deadline)| {
            let expired = now >= *deadline;
            if expired {
                let _ = expired_ids.push(attempt.link_id());
            }
            !expired
        });
        self.expired_link_requests = self
            .expired_link_requests
            .saturating_add(expired_ids.len() as u16);
        expired_ids
    }

    /// Whether a link with this id is established.
    pub fn has_link(&self, link_id: AddressHash) -> bool {
        self.links.iter().any(|(link, _, _)| link.id() == link_id)
    }

    /// Link requests refused because the table was full. Nonzero means `LINKS` is too small
    /// for the traffic this node sees, and peers are being turned away.
    pub fn refused_links(&self) -> u16 {
        self.refused_links
    }

    /// Link slots reclaimed from peers that stopped answering.
    ///
    /// Read alongside [`Node::refused_links`]: refusals with no expiries is a node with more
    /// demand than slots, while expiries climbing is a node whose peers keep vanishing. The
    /// two want different answers, and before expiry existed they were the same silence.
    pub fn expired_links(&self) -> u16 {
        self.expired_links
    }

    /// Link requests this node opened that were dropped unanswered at their deadline.
    ///
    /// Climbing while [`Node::refused_links`] stays at zero is a lossy path, not a busy node.
    pub fn expired_link_requests(&self) -> u16 {
        self.expired_link_requests
    }

    /// Announces whose identity a full address book could not take, because every peer in it
    /// had a live route. The route is still learned and the announce still relayed. See
    /// [`Node::refused_links`] for the posture: refusals are visible, never silent.
    pub fn refused_peers(&self) -> u16 {
        self.refused_peers
    }

    /// Resource offers turned away, by the part ceiling or by full receiver slots.
    pub fn refused_offers(&self) -> u16 {
        self.refused_offers
    }

    /// Publish a resource on an established link.
    ///
    /// `random_hash` and `iv` are caller-supplied, per the same no-RNG discipline as
    /// everything else here. Returns `None` if the link is unknown or a transfer is already
    /// running on it: one at a time, because a board cannot hold two.
    pub fn publish(
        &mut self,
        link_id: AddressHash,
        interface: InterfaceId,
        data: &[u8],
        random_hash: [u8; crate::resource::RANDOM_HASH_LEN],
        iv: &[u8; crate::token::IV_LEN],
        now: u64,
    ) -> Option<Actions<ACTIONS>> {
        if data.len() > self.payload_limits.max_outbound_resource {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return None;
        }
        if self.senders.iter().any(|(id, _, _)| *id == link_id) || self.senders.is_full() {
            return None;
        }
        let (link, _, _) = self.links.iter().find(|(l, _, _)| l.id() == link_id)?;

        let sender = ResourceSender::publish(link.clone(), data, random_hash, iv);
        let advertisement = sender.advertisement(iv);
        let _ = self.senders.push((link_id, sender, now));

        let mut actions = Actions::new();
        actions.push(Action::Send {
            interface,
            packet: advertisement,
        });
        Some(actions)
    }

    /// Whether a resource is being received or sent on this link.
    pub fn transfer_active(&self, link_id: AddressHash) -> bool {
        self.receivers.iter().any(|(id, _, _)| *id == link_id)
            || self.senders.iter().any(|(id, _, _)| *id == link_id)
    }

    /// Open a link to a destination this node has heard announce.
    ///
    /// `ephemeral_seed` is caller-supplied, per attempt, for the same reason every other
    /// key here is: no RNG in the protocol layer. Returns `None` if the peer is unknown or
    /// the pending table is full.
    ///
    /// A destination learned through a transport node is addressed to it (header type 2),
    /// as `Endpoint` does, so the relay carries the request on. Every node learns routes,
    /// whatever its transport policy. A route past its TTL at `now` is not used, whether or
    /// not it has been evicted yet; the request then goes out as header type 1.
    ///
    /// The request is not retried. If no proof arrives by `now` plus
    /// [`link_request_timeout`] of the route's relay count (zero with no route) plus
    /// `interface`'s [`Self::first_hop_airtime`], the first
    /// [`Node::poll`] at or after that deadline drops it and reports
    /// [`Action::LinkRequestTimedOut`] with its link id, which `link::link_id` reads from the
    /// request. A later proof is ignored.
    ///
    /// Requests already past their deadline at `now` are dropped first, so a full table is
    /// never refused only because the caller has not polled. Each is reported as
    /// [`Action::LinkRequestTimedOut`] ahead of the new request's send, exactly as `poll`
    /// would have. A refusal (unknown peer, or a table full of live requests) drops nothing.
    pub fn open_link(
        &mut self,
        destination: AddressHash,
        interface: InterfaceId,
        ephemeral_seed: &[u8; 64],
        now: u64,
    ) -> Option<Actions<ACTIONS>> {
        let peer = self.book.resolve(destination)?.identity;
        let mut actions = Actions::new();
        for link_id in self.expire_link_requests(now) {
            actions.push(Action::LinkRequestTimedOut { link_id });
        }
        if self.pending.is_full() {
            // Only live requests remain: had any expired, its slot would now be free.
            debug_assert!(actions.is_empty());
            self.refused_links = self.refused_links.saturating_add(1);
            return None;
        }

        let (attempt, mut request) = PendingLink::open(
            destination,
            peer,
            ephemeral_seed,
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: self.logical_mtu,
            },
        );
        let hop = self.next_hop(destination, now);
        request.address_via(hop.and_then(|hop| hop.via));
        let deadline = now
            .saturating_add(link_request_timeout(hop.map_or(0, |hop| hop.hops)))
            .saturating_add(self.first_hop_airtime(interface));
        let _ = self.pending.push((attempt, deadline));

        actions.push(Action::Send {
            interface,
            packet: request,
        });
        Some(actions)
    }

    /// Send application bytes on an established link.
    ///
    /// `iv` is caller-supplied and must not repeat for a link's key. Both ends share that key,
    /// so this holds across the two of them: the packet's hash is remembered, and a copy heard
    /// back (a relay's retransmission) is recognised as our own rather than delivered.
    pub fn send(
        &mut self,
        link_id: AddressHash,
        interface: InterfaceId,
        payload: &[u8],
        iv: &[u8; crate::token::IV_LEN],
    ) -> Option<Actions<ACTIONS>> {
        if payload.len() > self.payload_limits.max_link_payload {
            return None;
        }
        let (link, _, _) = self.links.iter().find(|(l, _, _)| l.id() == link_id)?;
        // CBC always adds a padding block, even for aligned plaintext. Refuse before
        // encryption/allocation, then verify the codec's actual packet size as well.
        let padded = payload
            .len()
            .checked_div(16)?
            .checked_add(1)?
            .checked_mul(16)?;
        let encoded = padded
            .checked_add(crate::token::TOKEN_OVERHEAD)?
            .checked_add(crate::packet::HEADER_MIN_LEN)?;
        let budget = link.mtu().min(self.logical_mtu) as usize;
        if encoded > budget {
            return None;
        }
        let packet = link.data_packet(payload, iv);
        if packet.encoded_len() > budget {
            return None;
        }
        self.remember_sent_link_data(packet.hash());
        let mut actions = Actions::new();
        actions.push(Action::Send { interface, packet });
        Some(actions)
    }

    /// Remember a link data packet we sent. At capacity the oldest is forgotten, so a long
    /// burst can outrun the window; that only lets a late echo through, never drops data.
    fn remember_sent_link_data(&mut self, hash: AddressHash) {
        remember_hash(&mut self.sent_link_data, hash);
    }

    /// Remove routes and carried-link records that have outlived the policy that admitted
    /// them. This is called both from [`Node::poll`] and before a transit decision, so a slow
    /// board clock cannot leave a stale route usable merely because it has not polled yet.
    fn expire_transport_state(&mut self, now: u64) {
        self.expire_routes(now);
        while let Some(index) = self
            .bridges
            .iter()
            .position(|bridge| now.saturating_sub(bridge.seen) >= self.transport.bridge_ttl)
        {
            self.bridges.swap_remove(index);
            self.transport_counters.expired_bridges =
                self.transport_counters.expired_bridges.saturating_add(1);
        }
        self.seen_transit
            .retain(|seen| now.saturating_sub(seen.seen) < TRANSPORT_DEDUP_TIMEOUT);
        self.reverse
            .retain(|entry| now.saturating_sub(entry.seen) < REVERSE_TIMEOUT);
    }

    fn expire_routes(&mut self, now: u64) {
        while let Some(index) = self
            .routes
            .iter()
            .position(|route| now.saturating_sub(route.learned) >= self.transport.route_ttl)
        {
            self.routes.swap_remove(index);
            self.transport_counters.expired_routes =
                self.transport_counters.expired_routes.saturating_add(1);
        }
    }

    /// Record a route from a freshness-accepted announce. The accepted announce is the route
    /// incumbent regardless of hop count. Freshness decides whether an announce may mutate any
    /// observable state; route selection must not apply a second shortest-path filter.
    fn learn_route(
        &mut self,
        destination: AddressHash,
        interface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
        now: u64,
    ) {
        if destination == self.destination() {
            return;
        }
        self.expire_routes(now);
        if let Some(route) = self
            .routes
            .iter_mut()
            .find(|route| route.destination == destination)
        {
            *route = Route {
                destination,
                interface,
                transport,
                hops,
                learned: now,
            };
            return;
        }

        if self.routes.is_full()
            && let Some(index) = self
                .routes
                .iter()
                .enumerate()
                .min_by_key(|(_, route)| route.learned)
                .map(|(index, _)| index)
        {
            self.routes.swap_remove(index);
            self.transport_counters.evicted_routes =
                self.transport_counters.evicted_routes.saturating_add(1);
        }
        let _ = self.routes.push(Route {
            destination,
            interface,
            transport,
            hops,
            learned: now,
        });
    }

    /// Whether this is a fresh packet for a shared-radio relay. At capacity, forget the
    /// oldest observation rather than growing or refusing all later traffic.
    fn transit_is_new(&mut self, hash: AddressHash, now: u64) -> bool {
        self.seen_transit
            .retain(|seen| now.saturating_sub(seen.seen) < TRANSPORT_DEDUP_TIMEOUT);
        if self.seen_transit.iter().any(|seen| seen.hash == hash) {
            return false;
        }
        if self.seen_transit.is_full()
            && let Some(index) = self
                .seen_transit
                .iter()
                .enumerate()
                .min_by_key(|(_, seen)| seen.seen)
                .map(|(index, _)| index)
        {
            self.seen_transit.swap_remove(index);
        }
        self.seen_transit
            .push(SeenPacket { hash, seen: now })
            .is_ok()
    }

    fn remember_bridge(
        &mut self,
        link_id: AddressHash,
        from: InterfaceId,
        out: InterfaceId,
        now: u64,
    ) {
        if let Some(bridge) = self
            .bridges
            .iter_mut()
            .find(|bridge| bridge.link_id == link_id)
        {
            *bridge = LinkBridge {
                link_id,
                from,
                out,
                seen: now,
            };
            return;
        }
        if self.bridges.is_full()
            && let Some(index) = self
                .bridges
                .iter()
                .enumerate()
                .min_by_key(|(_, bridge)| bridge.seen)
                .map(|(index, _)| index)
        {
            self.bridges.swap_remove(index);
            self.transport_counters.evicted_bridges =
                self.transport_counters.evicted_bridges.saturating_add(1);
        }
        let _ = self.bridges.push(LinkBridge {
            link_id,
            from,
            out,
            seen: now,
        });
    }

    /// Relay a packet already associated with a carried link. Link proofs and data name the
    /// link id rather than the original destination, so this lookup precedes normal transit
    /// routing.
    fn forward_bridged_packet(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) -> bool {
        if !self.transport.relay_packets {
            return false;
        }
        let Some(index) = self
            .bridges
            .iter()
            .position(|bridge| bridge.link_id == packet.destination)
        else {
            return false;
        };
        let bridge = &self.bridges[index];
        let out = if interface == bridge.from {
            bridge.out
        } else if interface == bridge.out {
            bridge.from
        } else {
            // A third interface cannot extend this bridge's lifetime or poison the
            // relay de-duplication cache for a later packet from a real endpoint.
            return true;
        };
        if packet.hops >= self.transport.max_hops {
            self.transport_counters.hop_limit_dropped =
                self.transport_counters.hop_limit_dropped.saturating_add(1);
            return true;
        }
        if !self.transit_is_new(packet.hash(), now) {
            return true;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        forwarded.header_type = HeaderType::Type1;
        forwarded.transport = None;
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return true;
        }
        if actions.push(Action::Send {
            interface: out,
            packet: forwarded,
        }) {
            self.bridges[index].seen = now;
            self.transport_counters.forwarded_packets =
                self.transport_counters.forwarded_packets.saturating_add(1);
        }
        true
    }

    /// Carry a header-type-2 packet addressed to this node towards its learned destination.
    fn forward_transport_packet(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) -> bool {
        if !self.transport.relay_packets
            || packet.header_type != HeaderType::Type2
            || packet.transport != Some(self.identity.hash())
            || packet.destination == self.destination()
        {
            return false;
        }
        if packet.hops >= self.transport.max_hops {
            self.transport_counters.hop_limit_dropped =
                self.transport_counters.hop_limit_dropped.saturating_add(1);
            return true;
        }
        let Some(route) = self
            .routes
            .iter()
            .find(|route| route.destination == packet.destination)
            .copied()
        else {
            self.transport_counters.unroutable_packets =
                self.transport_counters.unroutable_packets.saturating_add(1);
            return true;
        };
        if !self.transit_is_new(packet.hash(), now) {
            return true;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        forwarded.header_type = HeaderType::Type1;
        forwarded.transport = None;
        if let Some(next_transport) = route.transport {
            forwarded.header_type = HeaderType::Type2;
            forwarded.transport = Some(next_transport);
        }
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return true;
        }
        if actions.push(Action::Send {
            interface: route.interface,
            packet: forwarded,
        }) {
            if packet.packet_type == PacketType::LinkRequest {
                if let Ok(link_id) = link::link_id(packet) {
                    self.remember_bridge(link_id, interface, route.interface, now);
                }
            } else {
                self.remember_reverse(packet.hash(), interface, route.interface, now);
            }
            self.transport_counters.forwarded_packets =
                self.transport_counters.forwarded_packets.saturating_add(1);
        }
        true
    }

    /// Record the way back for a carried packet's proof (RNS `Transport.py` 2104-2110).
    fn remember_reverse(
        &mut self,
        packet: AddressHash,
        received: InterfaceId,
        outbound: InterfaceId,
        now: u64,
    ) {
        self.reverse.retain(|entry| entry.packet != packet);
        if self.reverse.is_full()
            && let Some(index) = self
                .reverse
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.seen)
                .map(|(index, _)| index)
        {
            self.reverse.swap_remove(index);
        }
        let _ = self.reverse.push(ReverseEntry {
            packet,
            received,
            outbound,
            seen: now,
        });
    }

    /// Carry a delivery proof back along a remembered reverse path. The entry is consumed
    /// either way; a proof arriving on any interface but the one the packet left by is not
    /// carried (RNS `Transport.py` 2733-2744). Returns whether the proof was carried.
    fn forward_reverse_proof(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        actions: &mut Actions<ACTIONS>,
    ) -> bool {
        if !self.transport.relay_packets
            || matches!(packet.context, link::CTX_LRPROOF | link::CTX_RESOURCE_PRF)
        {
            return false;
        }
        let Some(index) = self
            .reverse
            .iter()
            .position(|entry| entry.packet == packet.destination)
        else {
            return false;
        };
        let entry = self.reverse.swap_remove(index);
        if interface != entry.outbound {
            return false;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return true;
        }
        if actions.push(Action::Send {
            interface: entry.received,
            packet: forwarded,
        }) {
            self.transport_counters.forwarded_packets =
                self.transport_counters.forwarded_packets.saturating_add(1);
        }
        true
    }

    /// Re-broadcast a verified announce with this node recorded as the transport hop.
    fn relay_announce(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        destination: AddressHash,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // A path response answers one requester. RNS learns from it but never queues it for
        // rebroadcast, so one path request cannot flood the mesh.
        if !self.transport.relay_announces
            || destination == self.destination()
            || packet.context == crate::path::CTX_PATH_RESPONSE
        {
            return;
        }
        if packet.hops >= self.transport.max_hops {
            self.transport_counters.hop_limit_dropped =
                self.transport_counters.hop_limit_dropped.saturating_add(1);
            return;
        }
        if !self.transit_is_new(packet.hash(), now) {
            return;
        }
        let mut forwarded = packet.clone();
        forwarded.hops = forwarded.hops.saturating_add(1);
        forwarded.header_type = HeaderType::Type2;
        forwarded.transport = Some(self.identity.hash());
        if forwarded.encoded_len() > self.logical_mtu as usize {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return;
        }
        if actions.push(Action::Send {
            interface,
            packet: forwarded,
        }) {
            self.transport_counters.forwarded_announces = self
                .transport_counters
                .forwarded_announces
                .saturating_add(1);
        }
    }

    /// Feed a received packet in.
    ///
    /// Anything malformed, unsigned, or not addressed to work this node does is dropped
    /// silently, exactly as the desktop drops it: a peer must not be able to make a board
    /// spend memory by sending rubbish.
    pub fn ingest(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
    ) -> Actions<ACTIONS> {
        let mut actions = Actions::new();

        // IFAC is the interface's envelope: `Ifac::open` strips the flag, so a packet still
        // carrying it was decoded raw off an interface without IFAC. RNS drops those.
        if packet.ifac {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return actions;
        }

        if packet.encoded_len() > self.payload_limits.max_ingress_bytes {
            self.refused_payloads = self.refused_payloads.saturating_add(1);
            return actions;
        }

        self.expire_transport_state(now);
        if packet.packet_type == PacketType::Proof
            && self.forward_reverse_proof(interface, packet, &mut actions)
        {
            return actions;
        }
        if packet.packet_type != PacketType::Announce
            && (self.forward_bridged_packet(interface, packet, now, &mut actions)
                || self.forward_transport_packet(interface, packet, now, &mut actions))
        {
            return actions;
        }

        match packet.packet_type {
            PacketType::Announce => {
                // A relay rebroadcasting our own announce echoes it back to us. We are not
                // our own peer, and the echo carries nothing new: drop it before it costs a
                // signature check or touches the peer, freshness or route state.
                if packet.destination == self.destination() {
                    return actions;
                }
                // `Announce::decode` verifies the signature and that the destination hash
                // matches the announced identity, so an entry can only come from an
                // announce whose maths checked out. The invalid fixtures are the proof.
                if let Ok(announce) = Announce::decode(packet) {
                    // A known destination announced under another key is rejected outright,
                    // before freshness, routes or relaying (RNS `Identity.validate_announce`).
                    if self.book.key_conflicts(&announce) {
                        self.transport_counters.key_mismatch_announces = self
                            .transport_counters
                            .key_mismatch_announces
                            .saturating_add(1);
                        return actions;
                    }
                    let candidate = AnnounceFreshnessCandidate {
                        destination: announce.destination,
                        blob: crate::announce::AnnounceBlob::from_wire(announce.rand_hash),
                    };
                    // Freshness belongs to the route. A destination without a live one is a
                    // first sighting, as after an RNS path cull.
                    let route_live = self.routes.iter().any(|route| {
                        route.destination == announce.destination
                            && now.saturating_sub(route.learned) < self.transport.route_ttl
                    });
                    let accepted = match self.freshness.evaluate(candidate, route_live) {
                        AnnounceFreshnessDecision::Accept(accepted) => accepted,
                        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay) => {
                            self.transport_counters.replayed_announces =
                                self.transport_counters.replayed_announces.saturating_add(1);
                            return actions;
                        }
                        AnnounceFreshnessDecision::Reject(
                            AnnounceFreshnessReject::StaleTimebase,
                        ) => {
                            self.transport_counters.stale_announces =
                                self.transport_counters.stale_announces.saturating_add(1);
                            return actions;
                        }
                    };

                    // The book makes room by evicting the least recently heard peer with no
                    // live route. Links and pending requests carry their own copy of the
                    // peer's keys, so they do not need the entry. A refusal (every peer
                    // routed) only keeps the identity out of the book: route learning and
                    // relaying follow the route table, as RNS relays from its path table.
                    let route_ttl = self.transport.route_ttl;
                    let routes = &self.routes;
                    let admitted = self.book.ingest_at(&announce, now, |destination| {
                        routes.iter().any(|route| {
                            route.destination == destination
                                && now.saturating_sub(route.learned) < route_ttl
                        })
                    }) != Ingested::Refused;
                    if !admitted {
                        self.refused_peers = self.refused_peers.saturating_add(1);
                    }

                    let record = self.freshness.record_accepted(candidate, accepted);
                    if let Some(evicted) = record.evicted_destination {
                        // A route never outlives its freshness row.
                        self.routes.retain(|route| route.destination != evicted);
                        self.transport_counters.evicted_freshness_rows = self
                            .transport_counters
                            .evicted_freshness_rows
                            .saturating_add(1);
                    }
                    self.transport_counters.evicted_freshness_blobs = self
                        .transport_counters
                        .evicted_freshness_blobs
                        .saturating_add(u16::from(record.evicted_blob.is_some()));

                    // Every node learns routes, as `Endpoint` learns paths whatever its
                    // policy: a leaf needs one to address its first relay. Only the
                    // transport policy decides whether this node forwards.
                    self.learn_route(
                        announce.destination,
                        interface,
                        packet.hops,
                        packet.transport,
                        now,
                    );
                    if admitted {
                        actions.push(Action::Learned {
                            destination: announce.destination,
                        });
                    }
                    self.relay_announce(interface, packet, announce.destination, now, &mut actions);
                }
            }
            PacketType::LinkRequest => self.on_link_request(interface, packet, now, &mut actions),
            PacketType::Proof => self.on_proof(interface, packet, now, &mut actions),
            PacketType::Data => self.on_link_data(interface, packet, now, &mut actions),
        }

        actions
    }

    /// A peer wants a link to us.
    fn on_link_request(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // Only for the destination this node answers to. Transport requests were handled
        // before local dispatch; anything that reaches here is not ours to answer.
        if packet.destination != self.destination() {
            return;
        }
        let Ok(id) = link::link_id(packet) else {
            return;
        };

        // Already established: the peer did not hear our proof, so send the same one again.
        // A fresh accept here would give the two sides different keys for one link.
        if let Some((_, proof, _)) = self.links.iter().find(|(link, _, _)| link.id() == id) {
            actions.push(Action::Send {
                interface,
                packet: proof.clone(),
            });
            return;
        }

        if self.links.is_full() {
            self.refused_links = self.refused_links.saturating_add(1);
            return;
        }

        // The responder's ephemeral seed is derived rather than random, because this layer
        // holds no RNG. It is bound to the link id and our identity, so it differs per
        // request and cannot be predicted without our private key.
        let seed = self.responder_seed(&id);
        let offered = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: self.logical_mtu,
        };
        if let Ok((link, proof)) = link::accept(packet, &self.identity, &seed, offered) {
            let link_id = link.id();
            let _ = self.links.push((link, proof.clone(), now));
            actions.push(Action::Send {
                interface,
                packet: proof,
            });
            actions.push(Action::LinkUp { link_id });
        }
    }

    /// A proof for a link we opened, or a resource proof for a transfer we are sending.
    fn on_proof(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // RNS proves receipt of a resource with a PROOF-type packet on the link. It belongs
        // to an outbound transfer only, so with no sender on that link it is dropped rather
        // than handed to a receiver it could only confuse.
        if packet.context == link::CTX_RESOURCE_PRF {
            let link_id = packet.destination;
            if self.senders.iter().any(|(id, _, _)| *id == link_id)
                && let Some(index) = self
                    .links
                    .iter()
                    .position(|(link, _, _)| link.id() == link_id)
            {
                self.links[index].2 = now;
                self.on_resource(interface, link_id, index, packet, now, actions);
            }
            return;
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|(attempt, _)| attempt.prove(packet).is_ok())
        else {
            return;
        };
        let (attempt, _) = self.pending.swap_remove(index);
        let Ok(link) = attempt.prove(packet) else {
            return;
        };
        if self.links.is_full() {
            self.refused_links = self.refused_links.saturating_add(1);
            return;
        }
        let link_id = link.id();
        // Our own proof has no place here: this side was the initiator, so there is nothing
        // to re-send. The stored packet is the proof we received, kept only for symmetry.
        let _ = self.links.push((link, packet.clone(), now));
        actions.push(Action::LinkUp { link_id });
    }

    /// Traffic on an established link.
    fn on_link_data(
        &mut self,
        interface: InterfaceId,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        let link_id = packet.destination;
        let Some(index) = self
            .links
            .iter()
            .position(|(link, _, _)| link.id() == link_id)
        else {
            return;
        };

        // Our own packet, heard back from a relay. Not the far end's data, and not evidence
        // that the far end is alive.
        if self.sent_link_data.contains(&packet.hash()) {
            self.transport_counters.own_echo_dropped =
                self.transport_counters.own_echo_dropped.saturating_add(1);
            return;
        }

        // The far end's packet heard a second time, directly and from a relay. Dropped
        // before the liveness stamp, as the echo is: the copy is no newer than the original.
        if is_deduplicated_link_context(packet.context) {
            let hash = packet.hash();
            if self.received_link_data.contains(&hash) {
                self.transport_counters.duplicate_dropped =
                    self.transport_counters.duplicate_dropped.saturating_add(1);
                return;
            }
            remember_hash(&mut self.received_link_data, hash);
        }

        // Heard from: this is what keeps the slot. Recorded before dispatching, so a
        // resource transfer counts as liveness exactly as a keepalive does.
        self.links[index].2 = now;

        // Resource contexts are a transfer's business, not the link's.
        if is_resource_context(packet.context) {
            self.on_resource(interface, link_id, index, packet, now, actions);
            return;
        }

        match self.links[index].0.receive(packet) {
            Some(Inbound::Data(payload)) => {
                actions.push(Action::Data { link_id, payload });
            }
            Some(Inbound::Close) => {
                self.drop_link(index, actions);
            }
            // Keepalives, RTT, requests and responses are not this gate's work. They are
            // dropped rather than mishandled, and the boundary is pinned by a test so the
            // next gate's work shows up as a change.
            _ => {}
        }
    }

    /// A packet belonging to a resource transfer on this link.
    fn on_resource(
        &mut self,
        interface: InterfaceId,
        link_id: AddressHash,
        link_index: usize,
        packet: &Packet,
        now: u64,
        actions: &mut Actions<ACTIONS>,
    ) {
        // The IV feeds the transfer's own sealing. Derived rather than random for the same
        // reason the responder seed is: this layer holds no RNG, and a transfer answers
        // packets it did not ask for. The counter is node state, never reset, because an IV
        // must not repeat under a link key and a counter local to this call would replay
        // the whole sequence on the next call.
        let seed = self.identity.to_secret_bytes();
        let mut counter = self.iv_counter;
        let mut iv = || derived_iv(&seed, link_id, &mut counter);

        // An outbound transfer's replies come back on the same link, so try the sender
        // first: only one direction can own a given context on a given link at a time.
        if let Some(pos) = self.senders.iter().position(|(id, _, _)| *id == link_id) {
            let replies = self.senders[pos].1.on_packet(packet, &mut iv);
            self.iv_counter = counter;
            self.senders[pos].2 = now;
            let finished = self.senders[pos].1.is_done() || self.senders[pos].1.is_canceled();
            for reply in replies {
                actions.push(Action::Send {
                    interface,
                    packet: reply,
                });
            }
            if finished {
                self.senders.swap_remove(pos);
            }
            return;
        }

        let existing = self.receivers.iter().position(|(id, _, _)| *id == link_id);
        let is_new = existing.is_none();
        let pos = match existing {
            Some(pos) => pos,
            None => {
                if self.receivers.is_full() {
                    self.refused_offers = self.refused_offers.saturating_add(1);
                    return;
                }
                let link = self.links[link_index].0.clone();
                let receiver = ResourceReceiver::with_limits(
                    link,
                    RESOURCE_REQUEST_WINDOW,
                    self.payload_limits.max_resource_parts,
                );
                let _ = self.receivers.push((link_id, receiver, now));
                self.receivers.len() - 1
            }
        };

        let replies = self.receivers[pos].1.on_packet(packet, &mut iv);
        self.iv_counter = counter;
        self.receivers[pos].2 = now;

        // A receiver created for this packet that then said nothing did not accept the
        // transfer: an advertisement past the part ceiling is refused this way. Keeping it
        // would hold a slot, and on a board with a handful of slots that is the difference
        // between refusing one oversized offer and refusing every peer afterwards.
        if is_new && replies.is_empty() && self.receivers[pos].1.data().is_none() {
            self.receivers.swap_remove(pos);
            self.refused_offers = self.refused_offers.saturating_add(1);
            return;
        }

        for reply in replies {
            actions.push(Action::Send {
                interface,
                packet: reply,
            });
        }

        if let Some(data) = self.receivers[pos].1.data() {
            actions.push(Action::Resource {
                link_id,
                data: data.to_vec(),
            });
            self.receivers.swap_remove(pos);
        } else if self.receivers[pos].1.failure().is_some() {
            // A multi-segment offer, or a body past the decompression limit: its cancel
            // went out above, and nothing further is held for it.
            self.receivers.swap_remove(pos);
            self.refused_offers = self.refused_offers.saturating_add(1);
        } else if self.receivers[pos].1.is_canceled() {
            self.receivers.swap_remove(pos);
        }
    }

    /// Drop a link and everything riding on it.
    fn drop_link(&mut self, index: usize, actions: &mut Actions<ACTIONS>) {
        let link_id = self.links[index].0.id();
        self.links.swap_remove(index);
        // A transfer without its link is state nobody can finish, so it goes too. Leaving
        // it would hold reassembly memory for a peer that is no longer there.
        self.receivers.retain(|(id, _, _)| *id != link_id);
        self.senders.retain(|(id, _, _)| *id != link_id);
        actions.push(Action::LinkDown { link_id });
    }

    /// A responder ephemeral seed, derived from our identity and the link id.
    ///
    /// This layer has no RNG, and an initiator supplies its own seed from the shell. A
    /// responder answers packets it did not ask for, so it cannot be handed one per
    /// request without threading entropy through every ingest. Deriving it keeps the
    /// forward secrecy that matters (the seed is unpredictable without our private key)
    /// and makes a retransmitted request reproduce the same proof.
    fn responder_seed(&self, link_id: &AddressHash) -> [u8; 64] {
        let secret = self.identity.to_secret_bytes();
        let half = |tag: &[u8]| {
            let mut input = Vec::with_capacity(tag.len() + secret.len() + 16);
            input.extend_from_slice(tag);
            input.extend_from_slice(&secret);
            input.extend_from_slice(link_id.as_slice());
            crate::hash::full_hash(&input)
        };
        let mut seed = [0_u8; 64];
        seed[..32].copy_from_slice(&half(b"retinue/node/responder/a"));
        seed[32..].copy_from_slice(&half(b"retinue/node/responder/b"));
        seed
    }

    /// Whether the node should attempt its own announce at `now`.
    ///
    /// This predicate is separate from blob availability. A shell may be due to announce
    /// while it is still waiting for a reservation-backed blob; in that case [`Self::poll`]
    /// runs maintenance and leaves this predicate true for the next poll.
    pub fn announce_due(&self, now: u64) -> bool {
        match self.last_announce {
            None => true,
            Some(last) => now.saturating_sub(last) >= self.announce_interval,
        }
    }

    /// Advance the node's own timers.
    ///
    /// The shell supplies an optional typed announce blob. Clock acquisition, durable
    /// reservation, and nonce policy stay outside this executor-neutral layer. If an announce
    /// is due but no blob is available, the announce is skipped and remains due on the next
    /// poll; maintenance still runs.
    pub fn poll(
        &mut self,
        now: u64,
        interface: InterfaceId,
        blob: Option<&AnnounceBlob>,
    ) -> Actions<ACTIONS> {
        let mut actions = Actions::new();

        self.expire_transport_state(now);

        // Ordinary expiry also releases resource buffers and tells the caller.
        // A resident caller can call expire_sessions first to retain its full
        // resource-loss report, then poll without emitting duplicate reports.
        let expired = self.expire_sessions(now);
        for link_id in expired.links {
            actions.push(Action::LinkDown { link_id });
        }
        for link_id in expired.pending_links {
            actions.push(Action::LinkRequestTimedOut { link_id });
        }

        if self.announce_due(now)
            && let Some(blob) = blob
        {
            self.last_announce = Some(now);
            match self.try_announce(blob, None) {
                Ok(packet) => {
                    actions.push(Action::Send { interface, packet });
                }
                Err(_) => {
                    self.refused_payloads = self.refused_payloads.saturating_add(1);
                }
            }
        }

        // Loss recovery. A transfer that has heard nothing for a retry interval is
        // redriven: a receiver re-requests exactly what it is missing, a sender re-offers
        // an advertisement nobody answered. This is the mechanism behind N5's survive-loss
        // condition; without it, one lost frame was a dead transfer.
        let seed = self.identity.to_secret_bytes();
        let mut counter = self.iv_counter;
        for index in 0..self.receivers.len() {
            if now.saturating_sub(self.receivers[index].2) < RESOURCE_RETRY_INTERVAL {
                continue;
            }
            let link_id = self.receivers[index].0;
            let mut iv = || derived_iv(&seed, link_id, &mut counter);
            let replies = self.receivers[index].1.retransmit(&mut iv);
            self.receivers[index].2 = now;
            for reply in replies {
                actions.push(Action::Send {
                    interface,
                    packet: reply,
                });
            }
        }
        for index in 0..self.senders.len() {
            if now.saturating_sub(self.senders[index].2) < RESOURCE_RETRY_INTERVAL {
                continue;
            }
            let link_id = self.senders[index].0;
            let mut iv = || derived_iv(&seed, link_id, &mut counter);
            let advertisement = self.senders[index].1.advertisement(&iv());
            self.senders[index].2 = now;
            actions.push(Action::Send {
                interface,
                packet: advertisement,
            });
        }
        self.iv_counter = counter;

        actions
    }

    /// Forget that we announced, so the next [`Node::poll`] announces again.
    ///
    /// `poll` stamps the announce when it *decides* to send one, because it cannot know
    /// whether the shell got it onto the air. When the shell could not — a busy channel, a
    /// radio fault — the stamp would otherwise swallow the failure and the node would go
    /// quiet for a whole interval believing it had spoken. A shell that knows its send
    /// failed calls this; the shell is also responsible for bounding how often, since a
    /// permanently unusable radio must not turn into an announce loop.
    pub fn retry_announce(&mut self) {
        self.last_announce = None;
    }

    /// Build an announce within the logical carrier budget, including optional ratchet bytes.
    pub fn try_announce(
        &self,
        blob: &AnnounceBlob,
        ratchet: Option<&[u8; RATCHET_LEN]>,
    ) -> Result<Packet, AppDataTooLarge> {
        let base = MIN_LOGICAL_MTU as usize + if ratchet.is_some() { RATCHET_LEN } else { 0 };
        if base + self.app_data.len() > self.logical_mtu as usize {
            return Err(AppDataTooLarge);
        }
        Ok(self.announce(blob, ratchet))
    }

    /// Build this node's announce packet without enforcing the carrier budget.
    /// Use [`Self::try_announce`] for production egress; this builder also serves wire fixtures.
    pub fn announce(&self, blob: &AnnounceBlob, ratchet: Option<&[u8; RATCHET_LEN]>) -> Packet {
        announce::build(
            &self.identity,
            self.name_hash,
            blob,
            ratchet,
            &self.app_data,
        )
    }
}

#[cfg(test)]
mod tests {
    use alloc::vec;

    use super::*;
    use crate::announce::RAND_HASH_LEN;
    use crate::destination::DestinationName;
    use crate::identity::PrivateIdentity;

    const IFACE: InterfaceId = 0;

    fn fixture(name: &str) -> Packet {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/");
        let raw = std::fs::read(std::format!("{path}{name}")).unwrap();
        Packet::decode(&raw).unwrap()
    }

    /// The first packet a set of actions wants sent.
    fn sent<const N: usize>(actions: &Actions<N>) -> Option<Packet> {
        actions.iter().find_map(|a| match a {
            Action::Send { packet, .. } => Some(packet.clone()),
            _ => None,
        })
    }

    fn blob(bytes: [u8; RAND_HASH_LEN]) -> AnnounceBlob {
        AnnounceBlob::from_wire(bytes)
    }

    /// The link id a set of actions reports coming up.
    fn link_up<const N: usize>(actions: &Actions<N>) -> Option<AddressHash> {
        actions.iter().find_map(|a| match a {
            Action::LinkUp { link_id } => Some(*link_id),
            _ => None,
        })
    }

    #[test]
    fn logical_mtu_validates_configuration_and_announce_shapes() {
        let mut n = node();
        assert_eq!(n.logical_mtu(), LINK_MTU);
        assert!(!n.has_active_sessions());
        assert_eq!(
            n.set_logical_mtu(MIN_LOGICAL_MTU - 1),
            Err(LogicalMtuError::OutOfRange)
        );
        assert_eq!(
            n.set_logical_mtu(LINK_MTU + 1),
            Err(LogicalMtuError::OutOfRange)
        );
        n.set_logical_mtu(247).unwrap();
        n.try_set_app_data(&[1; 80]).unwrap();
        assert_eq!(
            n.try_announce(&blob([1; RAND_HASH_LEN]), None)
                .unwrap()
                .encoded_len(),
            247
        );
        assert!(
            n.try_announce(&blob([1; RAND_HASH_LEN]), Some(&[0; RATCHET_LEN]))
                .is_err()
        );
        assert!(n.try_set_app_data(&[2; 81]).is_err());
        assert_eq!(
            n.set_logical_mtu(246),
            Err(LogicalMtuError::AppDataTooLarge)
        );
        assert_eq!(n.logical_mtu(), 247);
    }

    #[test]
    fn logical_mtu_negotiates_both_roles_and_bounds_direct_data() {
        let (mut a, mut b) = pair();
        a.set_logical_mtu(247).unwrap();
        b.set_logical_mtu(239).unwrap();
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
        assert!(
            a.has_active_sessions(),
            "pending negotiation retains session state"
        );
        assert_eq!(a.set_logical_mtu(246), Err(LogicalMtuError::SessionsActive));
        let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
        let id = link_up(&a.ingest(IFACE, &proof, 0)).unwrap();
        assert!(a.has_active_sessions());
        assert!(b.has_active_sessions());
        assert_eq!(a.links[0].0.mtu(), 239);
        assert_eq!(b.links[0].0.mtu(), 239);
        assert_eq!(b.set_logical_mtu(238), Err(LogicalMtuError::SessionsActive));
        assert_eq!(b.set_logical_mtu(239), Ok(()));
        let exact = sent(
            &a.send(id, IFACE, &[0; 159], &[1; crate::token::IV_LEN])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(exact.encoded_len(), 227);
        assert!(
            a.send(id, IFACE, &[0; 160], &[2; crate::token::IV_LEN])
                .is_none()
        );
        assert!(
            b.send(id, IFACE, &[0; 160], &[3; crate::token::IV_LEN])
                .is_none()
        );
    }

    #[test]
    fn logical_mtu_bridge_lifecycle_and_refused_request_leave_no_phantom_session() {
        let mut relay = node().with_transport_config(TransportConfig::transit());
        relay.remember_bridge(AddressHash::from_bytes([9; 16]), 1, 2, 0);
        assert!(relay.has_active_sessions());
        assert_eq!(
            relay.set_logical_mtu(247),
            Err(LogicalMtuError::SessionsActive)
        );
        relay.expire_transport_state(LINK_TRANSPORT_TIMEOUT);
        assert!(!relay.has_active_sessions());
        relay.set_logical_mtu(247).unwrap();
        let (mut source, destination) = pair();
        let announce = destination.announce(&blob([3; RAND_HASH_LEN]), None);
        relay.ingest(IFACE + 1, &announce, LINK_TRANSPORT_TIMEOUT);
        source.ingest(IFACE, &announce, 0);
        let mut request = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x55; 64], 0)
                .unwrap(),
        )
        .unwrap();
        request.header_type = HeaderType::Type2;
        request.transport = Some(relay.identity.hash());
        request.payload.resize(250, 0);
        assert!(sent(&relay.ingest(IFACE, &request, LINK_TRANSPORT_TIMEOUT + 1)).is_none());
        assert!(relay.bridges.is_empty());
        assert_eq!(relay.refused_payloads(), 1);
        relay.set_logical_mtu(246).unwrap();
    }

    #[test]
    fn relay_refuses_type_two_growth_beyond_logical_mtu() {
        let mut relay = node().with_transport_config(TransportConfig::transit());
        relay.set_logical_mtu(247).unwrap();
        let (_, mut peer) = pair();
        peer.try_set_app_data(&[0; 80]).unwrap();
        let packet = peer.announce(&blob([4; RAND_HASH_LEN]), None);
        assert_eq!(packet.encoded_len(), 247);
        let actions = relay.ingest(IFACE, &packet, 0);
        assert!(sent(&actions).is_none());
        assert_eq!(relay.refused_payloads(), 1);
        assert!(relay.peers().knows(peer.destination()));
    }

    #[test]
    fn ingest_refuses_a_packet_carrying_the_ifac_flag() {
        let (mut node, peer) = pair();
        let mut flagged = peer.announce(&blob([5; RAND_HASH_LEN]), None);
        flagged.ifac = true;
        assert!(node.ingest(IFACE, &flagged, 0).is_empty());
        assert_eq!(node.refused_payloads(), 1);
        assert!(!node.peers().knows(peer.destination()));

        flagged.ifac = false;
        node.ingest(IFACE, &flagged, 0);
        assert_eq!(node.refused_payloads(), 1);
        assert!(node.peers().knows(peer.destination()));
    }

    /// Two nodes that have not met.
    fn pair() -> (Node<32, 8, 4>, Node<32, 8, 4>) {
        (
            Node::new(
                PrivateIdentity::from_secret_bytes(&[0x11; 64]),
                DestinationName::new("retinue", ["a"]).name_hash(),
            ),
            Node::new(
                PrivateIdentity::from_secret_bytes(&[0x22; 64]),
                DestinationName::new("retinue", ["b"]).name_hash(),
            ),
        )
    }

    /// Two nodes with a link already established between them.
    fn linked() -> (Node<32, 8, 4>, Node<32, 8, 4>, AddressHash) {
        let (mut a, mut b) = pair();
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
        let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
        let id = link_up(&a.ingest(IFACE, &proof, 0)).expect("link did not come up");
        (a, b, id)
    }

    #[test]
    fn pause_assessment_rejects_a_clock_before_retained_link_activity() {
        let (mut a, mut b) = pair();
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 1_000);
        let request = sent(
            &a.open_link(b.destination(), IFACE, &[0x31; 64], 1_000)
                .unwrap(),
        )
        .unwrap();
        let proof = sent(&b.ingest(IFACE, &request, 1_000)).unwrap();
        a.ingest(IFACE, &proof, 1_000);

        let assessment = a.pause_assessment();
        assert_eq!(assessment.latest_link_activity, Some(1_000));
        assert_eq!(
            assessment.can_pause_through(0, 100),
            Err(PauseBlocked::ClockBeforeLinkActivity {
                now: 0,
                latest_activity: 1_000,
            })
        );
    }

    fn node() -> Node {
        let identity = PrivateIdentity::from_secret_bytes(&[0x11; 64]);
        let name = DestinationName::new("retinue", ["node"]);
        Node::new(identity, name.name_hash())
    }

    /// A real RNS announce teaches the node a peer it can then reach.
    #[test]
    fn a_real_announce_is_learned() {
        let mut n = node();
        let packet = fixture("announce_appdata.bin");
        let actions = n.ingest(IFACE, &packet, 0);

        assert_eq!(actions.len(), 1, "one learned destination");
        assert_eq!(n.peers().len(), 1);
        match actions.iter().next().unwrap() {
            Action::Learned { destination } => assert!(n.peers().knows(*destination)),
            other => panic!("expected Learned, got {other:?}"),
        }
    }

    /// Every RNS-generated invalid announce is refused, and none of them leaves a trace.
    ///
    /// This is the oracle that matters most for a board: the fixtures were produced by real
    /// RNS with one field corrupted each, so a node that accepted any of them would be
    /// letting a peer populate its tables with unverified identity.
    #[test]
    fn every_invalid_announce_fixture_is_refused() {
        for name in [
            "announce_invalid_signature.bin",
            "announce_invalid_pubkey.bin",
            "announce_invalid_desthash.bin",
            "announce_invalid_namehash.bin",
            "announce_invalid_randhash.bin",
            "announce_invalid_appdata.bin",
        ] {
            let mut n = node();
            let actions = n.ingest(IFACE, &fixture(name), 0);
            assert!(actions.is_empty(), "{name} produced an action");
            assert_eq!(n.peers().len(), 0, "{name} populated the address book");
        }
    }

    /// A node announces itself promptly on boot, then holds off for its interval.
    #[test]
    fn announces_on_boot_then_waits_for_the_interval() {
        let mut n = node().with_announce_interval(1_000);
        let announce_blob = blob([0x55; RAND_HASH_LEN]);

        assert!(n.announce_due(0), "a fresh node is due on boot");
        let first = n.poll(0, IFACE, Some(&announce_blob));
        assert_eq!(first.len(), 1, "a fresh node announces without waiting");

        assert!(!n.announce_due(1), "the interval has not elapsed");
        assert!(
            n.poll(1, IFACE, Some(&announce_blob)).is_empty(),
            "not due yet"
        );
        assert!(
            n.poll(999, IFACE, Some(&announce_blob)).is_empty(),
            "still not due"
        );
        assert!(n.announce_due(1_000), "the interval has elapsed");
        assert_eq!(
            n.poll(1_000, IFACE, Some(&announce_blob)).len(),
            1,
            "due at the interval"
        );
    }

    #[test]
    fn a_due_announce_without_a_blob_stays_due_until_supplied() {
        let mut n = node().with_announce_interval(1_000);

        assert!(n.announce_due(0));
        assert!(n.poll(0, IFACE, None).is_empty());
        assert!(n.announce_due(1), "missing blob must not consume due state");
        assert!(n.poll(1, IFACE, None).is_empty());

        let announce_blob = blob([0x56; RAND_HASH_LEN]);
        assert!(sent(&n.poll(1, IFACE, Some(&announce_blob))).is_some());
        assert!(!n.announce_due(2), "successful emission consumes due state");
        assert!(n.poll(2, IFACE, Some(&announce_blob)).is_empty());
    }

    /// Our own announce is a real one: it decodes, verifies, and names us.
    #[test]
    fn our_announce_round_trips_through_the_decoder() {
        let n = node().with_app_data(b"retinue-node");
        let packet = n.announce(&blob([0x22; RAND_HASH_LEN]), None);

        let decoded = Announce::decode(&packet).expect("our own announce must verify");
        assert_eq!(decoded.destination, n.destination());
        assert_eq!(decoded.app_data, b"retinue-node");
    }

    /// Two nodes learn each other from each other's announces, which is the whole of the
    /// discovery half of a link.
    #[test]
    fn two_nodes_learn_each_other() {
        let mut a = Node::<32, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0xA1; 64]),
            DestinationName::new("retinue", ["a"]).name_hash(),
        );
        let mut b = Node::<32, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0xB2; 64]),
            DestinationName::new("retinue", ["b"]).name_hash(),
        );

        let from_a = a.announce(&blob([1; RAND_HASH_LEN]), None);
        let from_b = b.announce(&blob([2; RAND_HASH_LEN]), None);

        assert_eq!(b.ingest(IFACE, &from_a, 0).len(), 1);
        assert_eq!(a.ingest(IFACE, &from_b, 0).len(), 1);

        assert!(b.peers().knows(a.destination()), "b can now reach a");
        assert!(a.peers().knows(b.destination()), "a can now reach b");
    }

    /// A full address book keeps serving and stops learning, rather than growing.
    #[test]
    fn a_full_book_stops_learning_without_faulting() {
        let mut n = Node::<1, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["node"]).name_hash(),
        );
        assert_eq!(
            n.ingest(IFACE, &fixture("announce_appdata.bin"), 0).len(),
            1
        );

        // A different destination cannot be learned, and says nothing rather than faulting.
        let other = Node::<32, 8>::new(
            PrivateIdentity::from_secret_bytes(&[0xC3; 64]),
            DestinationName::new("retinue", ["other"]).name_hash(),
        )
        .announce(&blob([9; RAND_HASH_LEN]), None);
        assert!(n.ingest(IFACE, &other, 0).is_empty());
        assert_eq!(n.peers().len(), 1, "the established peer survives");
        assert_eq!(n.peers().refused(), 1, "and the refusal is counted");
    }

    #[test]
    fn freshness_gates_effects_and_newer_route_replaces_regardless_of_hops() {
        let mut relay = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x81; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x82; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        );

        let mut first = peer.announce(&blob([1, 0, 0, 0, 0, 0, 0, 0, 0, 10]), None);
        first.hops = 1;
        let accepted = relay.ingest(IFACE, &first, 0);
        assert_eq!(accepted.len(), 2, "learn plus relay");
        assert_eq!(relay.route_to(peer.destination(), 0), Some((IFACE, 1)));

        let mut newer_equal = peer.announce(&blob([2, 0, 0, 0, 0, 0, 0, 0, 0, 11]), None);
        newer_equal.hops = 1;
        let accepted = relay.ingest(IFACE + 1, &newer_equal, 1);
        assert_eq!(accepted.len(), 2, "newer equal-hop announce replaces");
        assert_eq!(relay.route_to(peer.destination(), 1), Some((IFACE + 1, 1)));

        let mut newer_worse = peer.announce(&blob([3, 0, 0, 0, 0, 0, 0, 0, 0, 12]), None);
        newer_worse.hops = 7;
        let accepted = relay.ingest(IFACE + 2, &newer_worse, 2);
        assert_eq!(accepted.len(), 2, "newer announce still learns and relays");
        assert_eq!(relay.route_to(peer.destination(), 2), Some((IFACE + 2, 7)));

        let mut stale = peer.announce(&blob([4, 0, 0, 0, 0, 0, 0, 0, 0, 11]), None);
        stale.hops = 0;
        assert!(relay.ingest(IFACE, &stale, 3).is_empty());
        assert_eq!(relay.peers().len(), 1);
        assert_eq!(relay.route_to(peer.destination(), 3), Some((IFACE + 2, 7)));
        assert_eq!(relay.transport_counters().stale_announces, 1);

        assert!(relay.ingest(IFACE, &newer_worse, 4).is_empty());
        assert_eq!(relay.transport_counters().replayed_announces, 1);
    }

    /// RNS culls a path row with its random blobs (`Transport.py` 957-978, 1086-1090), so the
    /// next announce is a first sighting whatever its emission time or hops.
    #[test]
    fn an_expired_route_admits_any_announce_as_a_first_sighting() {
        let mut relay = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x83; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig {
            route_ttl: 10,
            ..TransportConfig::transit()
        });
        let better_peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x84; 64]),
            DestinationName::new("retinue", ["better"]).name_hash(),
        );
        let equal_peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x85; 64]),
            DestinationName::new("retinue", ["equal"]).name_hash(),
        );
        let worse_peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x86; 64]),
            DestinationName::new("retinue", ["worse"]).name_hash(),
        );
        for peer in [&better_peer, &equal_peer, &worse_peer] {
            let mut first = peer.announce(&blob([1, 0, 0, 0, 0, 0, 0, 0, 0, 20]), None);
            first.hops = 2;
            assert_eq!(relay.ingest(IFACE, &first, 0).len(), 2);
        }
        assert_eq!(relay.route_count(), 3);
        let _ = relay.poll(10, IFACE, Some(&blob([0; RAND_HASH_LEN])));
        assert_eq!(relay.route_count(), 0, "route TTL removed the routes");

        for (peer, hops) in [(&better_peer, 1), (&equal_peer, 2), (&worse_peer, 3)] {
            let mut older = peer.announce(&blob([2, 0, 0, 0, 0, 0, 0, 0, 0, 19]), None);
            older.hops = hops;
            assert_eq!(relay.ingest(IFACE + 1, &older, 11).len(), 2);
            assert_eq!(
                relay.route_to(peer.destination(), 11),
                Some((IFACE + 1, hops))
            );
        }
        assert_eq!(relay.transport_counters().stale_announces, 0);
    }

    /// A transport answering `request_path` from its cache sends the blob it already relayed
    /// (`Transport.py` 3459-3530). Once the route has gone, that same blob restores it.
    #[test]
    fn a_same_blob_announce_restores_an_expired_route() {
        let mut n = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x91; 64]),
            DestinationName::new("retinue", ["node"]).name_hash(),
        )
        .with_transport_config(TransportConfig {
            route_ttl: 10,
            ..TransportConfig::none()
        });
        let peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x92; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        );
        let mut announce = peer.announce(&blob([7, 0, 0, 0, 0, 0, 0, 0, 0, 30]), None);
        announce.hops = 3;
        assert_eq!(n.ingest(IFACE, &announce, 0).len(), 1);

        assert!(n.ingest(IFACE, &announce, 9).is_empty(), "live: a replay");
        assert_eq!(n.transport_counters().replayed_announces, 1);

        let mut cached = announce.clone();
        cached.context = crate::path::CTX_PATH_RESPONSE;
        assert_eq!(n.route_to(peer.destination(), 10), None, "route expired");
        assert_eq!(n.ingest(IFACE + 1, &cached, 10).len(), 1);
        assert_eq!(n.route_to(peer.destination(), 10), Some((IFACE + 1, 3)));
        assert_eq!(n.transport_counters().replayed_announces, 1);

        assert!(
            n.ingest(IFACE, &announce, 11).is_empty(),
            "the restored route refuses the blob again"
        );
        assert_eq!(n.transport_counters().replayed_announces, 2);
    }

    #[test]
    fn a_live_route_admits_only_a_later_emission() {
        let mut n = node();
        let peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x93; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        );
        let at = |nonce: u8, timebase: u8, hops: u8| {
            let mut packet = peer.announce(&blob([nonce, 0, 0, 0, 0, 0, 0, 0, 0, timebase]), None);
            packet.hops = hops;
            packet
        };
        assert_eq!(n.ingest(IFACE, &at(1, 10, 4), 0).len(), 1);
        assert_eq!(n.ingest(IFACE, &at(2, 12, 4), 1).len(), 1);
        // Between the accepted emissions, at better, equal, and worse hops: all stale.
        for hops in [1, 4, 9] {
            assert!(n.ingest(IFACE, &at(3, 11, hops), 2).is_empty());
        }
        assert!(
            n.ingest(IFACE, &at(4, 12, 1), 2).is_empty(),
            "equal emission"
        );
        assert_eq!(n.transport_counters().stale_announces, 4);
        assert_eq!(n.route_to(peer.destination(), 2), Some((IFACE, 4)));
        assert_eq!(n.ingest(IFACE + 1, &at(5, 13, 9), 3).len(), 1);
        assert_eq!(n.route_to(peer.destination(), 3), Some((IFACE + 1, 9)));
    }

    /// A book full of routed peers refuses the identity, but the announce is still a route
    /// and still relayed, as RNS relays from its path table rather than its known
    /// destinations. Because the route changed, the freshness candidate is committed.
    #[test]
    fn address_book_refusal_still_learns_and_relays_the_route() {
        let mut n = Node::<1, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x85; 64]),
            DestinationName::new("retinue", ["node"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let first_peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x86; 64]),
            DestinationName::new("retinue", ["first"]).name_hash(),
        );
        let second_peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x87; 64]),
            DestinationName::new("retinue", ["second"]).name_hash(),
        );
        n.ingest(
            IFACE,
            &first_peer.announce(&blob([1; RAND_HASH_LEN]), None),
            0,
        );
        let packet = second_peer.announce(&blob([2; RAND_HASH_LEN]), None);
        let candidate = AnnounceFreshnessCandidate {
            destination: second_peer.destination(),
            blob: crate::announce::AnnounceBlob::from_wire([2; RAND_HASH_LEN]),
        };
        let actions = n.ingest(IFACE, &packet, 1);
        assert_eq!(n.refused_peers(), 1);
        assert!(!actions.iter().any(|a| matches!(a, Action::Learned { .. })));
        assert!(
            actions
                .iter()
                .any(|a| matches!(a, Action::Send { packet, .. }
                if packet.packet_type == PacketType::Announce
                    && packet.destination == second_peer.destination())),
            "the refused identity's announce is still relayed"
        );
        assert!(!n.peers().knows(second_peer.destination()));
        assert!(n.peers().knows(first_peer.destination()));
        assert_eq!(n.route_count(), 2);
        assert_eq!(n.route_to(second_peer.destination(), 1), Some((IFACE, 0)));
        assert!(matches!(
            n.freshness.evaluate(candidate, true),
            AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay)
        ));
    }

    /// A small book does not stop a node learning new destinations forever. Once the
    /// routes of the peers it holds expire, the least recently heard of them yields its slot.
    #[test]
    fn a_full_book_evicts_a_peer_whose_route_expired() {
        let mut n = Node::<2, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x9A; 64]),
            DestinationName::new("retinue", ["node"]).name_hash(),
        )
        .with_transport_config(TransportConfig {
            route_ttl: 10,
            ..TransportConfig::transit()
        });
        let peers: [Node<8, 8, 4, 4>; 4] = core::array::from_fn(|i| {
            Node::new(
                PrivateIdentity::from_secret_bytes(&[0xA0 + i as u8; 64]),
                DestinationName::new("retinue", ["peer"]).name_hash(),
            )
        });
        let relayed = |actions: &Actions<8>, destination: AddressHash| {
            actions.iter().any(|a| {
                matches!(a, Action::Send { packet, .. }
                if packet.packet_type == PacketType::Announce
                    && packet.destination == destination)
            })
        };
        let learned = |actions: &Actions<8>, destination: AddressHash| {
            actions
                .iter()
                .any(|a| *a == Action::Learned { destination })
        };

        for (i, peer) in peers[..3].iter().enumerate() {
            let at = i as u64;
            let actions = n.ingest(
                IFACE,
                &peer.announce(&blob([i as u8; RAND_HASH_LEN]), None),
                at,
            );
            assert!(relayed(&actions, peer.destination()));
            assert_eq!(learned(&actions, peer.destination()), i < 2);
        }
        assert_eq!(
            n.refused_peers(),
            1,
            "both held peers still had live routes"
        );

        // Past every route's TTL, nothing protects the held peers.
        let later = 20;
        let fourth = peers[3].destination();
        let actions = n.ingest(
            IFACE,
            &peers[3].announce(&blob([3; RAND_HASH_LEN]), None),
            later,
        );
        assert!(learned(&actions, fourth), "admitted by eviction");
        assert!(relayed(&actions, fourth));
        assert!(n.peers().knows(fourth));
        assert!(
            !n.peers().knows(peers[0].destination()),
            "the least recently heard peer went"
        );
        assert!(n.peers().knows(peers[1].destination()));
        assert_eq!(n.peers().len(), 2);
    }

    /// RNS learns from a path response but never rebroadcasts it: it answers one requester.
    #[test]
    fn a_path_response_is_learned_but_not_relayed() {
        let mut relay = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x9B; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x9C; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        );
        let mut response = peer.announce(&blob([5; RAND_HASH_LEN]), None);
        response.context = crate::path::CTX_PATH_RESPONSE;
        response.hops = 2;
        let actions = relay.ingest(IFACE, &response, 0);
        assert_eq!(
            actions.iter().collect::<Vec<_>>(),
            [&Action::Learned {
                destination: peer.destination()
            }]
        );
        assert!(relay.peers().knows(peer.destination()));
        assert_eq!(relay.route_to(peer.destination(), 0), Some((IFACE, 2)));
        assert_eq!(relay.transport_counters().forwarded_announces, 0);
    }

    #[test]
    fn packet_loop_dedup_is_after_freshness() {
        let mut relay = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x88; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x89; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        );
        let packet = peer.announce(&blob([4; RAND_HASH_LEN]), None);
        assert!(relay.transit_is_new(packet.hash(), 0));
        let actions = relay.ingest(IFACE, &packet, 1);
        assert_eq!(
            actions.len(),
            1,
            "freshness learns before loop dedup suppresses relay"
        );
        assert!(actions.iter().any(|a| matches!(a, Action::Learned { .. })));
        assert_eq!(relay.peers().len(), 1);
        assert_eq!(relay.transport_counters().replayed_announces, 0);
    }

    #[test]
    fn stale_same_blob_cannot_roll_back_ratchet_or_app_data() {
        let mut n = node();
        let peer = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x90; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        )
        .with_app_data(b"current");
        let announce_blob = blob([9; RAND_HASH_LEN]);
        let current = peer.announce(&announce_blob, Some(&[0xA1; RATCHET_LEN]));
        assert_eq!(n.ingest(IFACE, &current, 0).len(), 1);
        assert_eq!(
            n.peers().resolve(peer.destination()).unwrap().app_data,
            b"current"
        );
        assert_eq!(
            n.peers().resolve(peer.destination()).unwrap().ratchet,
            Some([0xA1; RATCHET_LEN])
        );

        let older = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x90; 64]),
            DestinationName::new("retinue", ["peer"]).name_hash(),
        )
        .with_app_data(b"rollback");
        let rollback = older.announce(&announce_blob, Some(&[0xB2; RATCHET_LEN]));
        assert!(n.ingest(IFACE, &rollback, 1).is_empty());
        let retained = n.peers().resolve(peer.destination()).unwrap();
        assert_eq!(retained.app_data, b"current");
        assert_eq!(retained.ratchet, Some([0xA1; RATCHET_LEN]));
    }

    #[test]
    fn freshness_policy_is_bounded_and_reconfigures_without_resetting_history() {
        let mut n = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x8A; 64]),
            DestinationName::new("retinue", ["node"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        assert!(
            Node::<8, 8, 4, 4>::new(
                PrivateIdentity::from_secret_bytes(&[0x8B; 64]),
                DestinationName::new("retinue", ["invalid"]).name_hash(),
            )
            .with_freshness_policy(FreshnessPolicy {
                max_destinations: 0,
                max_blobs_per_destination: 8,
            })
            .is_err()
        );

        let a = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x8C; 64]),
            DestinationName::new("retinue", ["a"]).name_hash(),
        );
        let b = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x8D; 64]),
            DestinationName::new("retinue", ["b"]).name_hash(),
        );
        let a_announce = a.announce(&blob([5; RAND_HASH_LEN]), None);
        n.ingest(IFACE, &a_announce, 0);
        n.ingest(IFACE, &b.announce(&blob([6; RAND_HASH_LEN]), None), 1);
        assert_eq!(n.freshness.config().destination_capacity, 8);
        let report = n
            .set_freshness_policy(FreshnessPolicy {
                max_destinations: 1,
                max_blobs_per_destination: 1,
            })
            .expect("valid bounds");
        assert_eq!(report.evicted_destinations, [a.destination()]);
        assert_eq!(n.freshness_policy().max_destinations, 1);
        assert_eq!(n.transport_counters().evicted_freshness_rows, 1);
        assert_eq!(n.transport_counters().evicted_freshness_blobs, 0);
        // A route never outlives its freshness row, so A's evicted row took its route along
        // and A's blob is a first sighting again. B's route and history survive.
        assert_eq!(n.route_to(a.destination(), 1), None);
        assert!(n.route_to(b.destination(), 1).is_some());
        assert!(
            n.ingest(IFACE, &b.announce(&blob([6; RAND_HASH_LEN]), None), 1)
                .is_empty()
        );
        assert_eq!(n.transport_counters().replayed_announces, 1);

        // Readmitting A evicts B, row and route, under the one-row bound. Packet-loop dedup
        // still suppresses the relay of a packet relayed a moment ago.
        assert!(
            n.ingest(IFACE, &a_announce, 2)
                .iter()
                .any(|action| matches!(action, Action::Learned { .. }))
        );
        assert_eq!(n.transport_counters().evicted_freshness_rows, 2);
        assert_eq!(n.route_to(b.destination(), 2), None);

        // The remaining destination's second accepted blob now exercises per-row history
        // pressure independently of destination-row pressure.
        let mut a_again = a.announce(&blob([7; RAND_HASH_LEN]), None);
        a_again.hops = 1;
        n.ingest(IFACE, &a_again, 3);
        assert_eq!(n.transport_counters().evicted_freshness_blobs, 1);
    }

    /// Actions are bounded, and say so when they fill.
    #[test]
    fn actions_report_overflow_rather_than_dropping_silently() {
        let mut actions = Actions::<2>::new();
        for _ in 0..5 {
            actions.push(Action::Learned {
                destination: AddressHash::from_bytes([0; 16]),
            });
        }
        assert_eq!(actions.len(), 2, "held to its bound");
        assert_eq!(actions.overflowed(), 3, "and counted what did not fit");
    }

    /// Data and link packets are not yet handled, and must be dropped rather than
    /// mishandled. This pins the boundary so the next gate's work is visible as a change.
    #[test]
    fn packets_this_gate_does_not_handle_are_dropped() {
        let mut n = node();
        // The same bytes that would be learned as an announce, relabelled. Nothing is
        // learned, because the type decides the handling and this gate handles one type.
        let packet = Packet {
            packet_type: PacketType::Data,
            ..fixture("announce_appdata.bin")
        };
        let actions = n.ingest(IFACE, &packet, 0);
        assert!(actions.is_empty());
        assert_eq!(n.peers().len(), 0);
    }

    /// Two nodes establish a link in the shape a radio carries it: announce, learn, open,
    /// accept, prove.
    #[test]
    fn two_nodes_establish_a_link() {
        let (mut a, mut b) = pair();

        // Discovery first: a must have heard b announce before it can address b.
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);

        let opened = a
            .open_link(b.destination(), IFACE, &[0x31; 64], 0)
            .expect("b is known, so a link can be opened");
        let request = sent(&opened).expect("a link request goes out");

        let accepted = b.ingest(IFACE, &request, 0);
        let proof = sent(&accepted).expect("b answers with a proof");
        assert_eq!(b.link_count(), 1, "b holds the link immediately");
        assert!(link_up(&accepted).is_some(), "b reports the link up");

        let completed = a.ingest(IFACE, &proof, 0);
        assert_eq!(a.link_count(), 1, "a holds the link once proved");
        let id = link_up(&completed).expect("a reports the link up");

        assert!(
            a.has_link(id) && b.has_link(id),
            "one link, one id, both sides"
        );
    }

    #[test]
    fn exact_idle_expiry_reclaims_a_real_link_and_its_resource_sender() {
        let (mut a, _b, id) = linked();
        assert!(
            a.publish(
                id,
                IFACE,
                b"retained resource",
                [0xA5; crate::resource::RANDOM_HASH_LEN],
                &[0x5A; crate::token::IV_LEN],
                0,
            )
            .is_some()
        );
        assert!(a.transfer_active(id));

        let report = a.expire_sessions(LINK_IDLE_TIMEOUT);
        assert_eq!(report.links.as_slice(), [id]);
        assert_eq!(report.inbound_resources.as_slice(), []);
        assert_eq!(report.outbound_resources.as_slice(), [id]);
        assert!(!a.has_link(id));
        assert!(!a.transfer_active(id));
        assert_eq!(a.expired_links(), 1);
    }

    /// A retransmitted link request is answered with the SAME proof, not a second link.
    ///
    /// A lossy medium creates this constantly: the initiator does not hear the proof and
    /// asks again. Accepting twice would leave the two sides holding different keys for
    /// what the initiator believes is one link, which fails later and confusingly.
    #[test]
    fn a_retransmitted_request_is_answered_with_the_same_proof() {
        let (mut a, mut b) = pair();
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();

        let first = sent(&b.ingest(IFACE, &request, 0)).expect("first proof");
        let second = sent(&b.ingest(IFACE, &request, 0)).expect("second proof");

        assert_eq!(first, second, "the same proof, byte for byte");
        assert_eq!(b.link_count(), 1, "and still exactly one link");
    }

    /// Data crosses an established link and arrives decrypted.
    #[test]
    fn data_crosses_an_established_link() {
        let (mut a, mut b, id) = linked();

        let out = a
            .send(id, IFACE, b"hello over the air", &[7; crate::token::IV_LEN])
            .expect("a can send on a link it holds");
        let packet = sent(&out).expect("a data packet goes out");

        let received = b.ingest(IFACE, &packet, 0);
        let found = received.iter().find_map(|x| match x {
            Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
            _ => None,
        });
        match found {
            Some((link_id, payload)) => {
                assert_eq!(link_id, id);
                assert_eq!(payload.as_slice(), b"hello over the air");
            }
            None => panic!("expected decrypted Data"),
        }
    }

    /// On a shared medium the first relay's retransmission of our own link data reaches us
    /// too. It decrypts under the shared link key, but it is our own payload and not data
    /// from the far end, and hearing it is not evidence that the peer is alive. Genuine data
    /// from the far end, relayed the same way, is still delivered.
    #[test]
    fn a_senders_own_data_overheard_from_a_relay_is_not_received() {
        let (mut source, mut destination) = pair();
        let mut relay = Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x44; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());

        let announce = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
        let relayed_announce = sent(&relay.ingest(IFACE, &announce, 0)).unwrap();
        source.ingest(IFACE, &relayed_announce, 1);
        let mut request = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x99; 64], 1)
                .unwrap(),
        )
        .unwrap();
        request.header_type = HeaderType::Type2;
        request.transport = Some(relay.identity.hash());
        let forwarded_request = sent(&relay.ingest(IFACE, &request, 2)).unwrap();
        let proof = sent(&destination.ingest(IFACE, &forwarded_request, 3)).unwrap();
        let forwarded_proof = sent(&relay.ingest(IFACE, &proof, 4)).unwrap();
        let id = link_up(&source.ingest(IFACE, &forwarded_proof, 5)).unwrap();

        let data_from = |actions: &Actions<8>| {
            actions.iter().find_map(|action| match action {
                Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
                _ => None,
            })
        };

        // The source sends. The relay retransmits, and the source hears the relay.
        let own = sent(
            &source
                .send(id, IFACE, b"from the source", &[0x51; 16])
                .unwrap(),
        )
        .unwrap();
        let retransmitted = sent(&relay.ingest(IFACE, &own, 6)).expect("the relay carries it on");
        assert_eq!(retransmitted.hops, 1);
        assert_eq!(
            data_from(&source.ingest(IFACE, &retransmitted, 7)),
            None,
            "a node must not surface its own payload as received data"
        );
        assert_eq!(
            source.pause_assessment().latest_link_activity,
            Some(5),
            "our own echo is not the far end being heard from"
        );

        // The same retransmission, as the far end hears it, is genuine delivery.
        assert_eq!(
            data_from(&destination.ingest(IFACE, &retransmitted, 7)),
            Some((id, b"from the source".to_vec()))
        );

        // And data the far end sends, relayed back, still reaches the source.
        let reply = sent(
            &destination
                .send(id, IFACE, b"from the far end", &[0x52; 16])
                .unwrap(),
        )
        .unwrap();
        let relayed_reply = sent(&relay.ingest(IFACE, &reply, 8)).unwrap();
        assert_eq!(
            data_from(&source.ingest(IFACE, &relayed_reply, 9)),
            Some((id, b"from the far end".to_vec())),
            "genuine data from the far end is still delivered"
        );
        assert_eq!(source.pause_assessment().latest_link_activity, Some(9));
        assert_eq!(
            data_from(&destination.ingest(IFACE, &relayed_reply, 9)),
            None,
            "the responder does not surface its own relayed reply either"
        );

        // The filter matches packets, not payloads: the far end sending the very bytes we
        // sent, under its own IV, is a different packet and is delivered.
        let same_bytes = sent(
            &destination
                .send(id, IFACE, b"from the source", &[0x53; 16])
                .unwrap(),
        )
        .unwrap();
        let relayed_same_bytes = sent(&relay.ingest(IFACE, &same_bytes, 10)).unwrap();
        assert_eq!(
            data_from(&source.ingest(IFACE, &relayed_same_bytes, 11)),
            Some((id, b"from the source".to_vec())),
            "equal plaintext from the far end is not mistaken for our own"
        );

        // The one collision is the far end reusing our IV for our plaintext, which yields our
        // packet byte for byte. That breaks the shared key's IV rule, and is refused as ours.
        let iv_reuse = sent(
            &destination
                .send(id, IFACE, b"from the source", &[0x51; 16])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(iv_reuse.hash(), own.hash());
        assert_eq!(data_from(&source.ingest(IFACE, &iv_reuse, 12)), None);
    }

    /// A shared medium hands us the far end's packet more than once: directly, and again
    /// from a relay with hops+1 and the same hash. The application sees it once, and the
    /// far end's next packet still arrives.
    #[test]
    fn a_far_end_packet_heard_twice_is_received_once() {
        let (mut a, mut b, id) = linked();
        let data_from = |actions: &Actions<8>| {
            actions.iter().find_map(|action| match action {
                Action::Data { link_id, payload } => Some((*link_id, payload.clone())),
                _ => None,
            })
        };

        let theirs = sent(&b.send(id, IFACE, b"from b", &[0x61; 16]).unwrap()).unwrap();
        let mut via_relay = theirs.clone();
        via_relay.hops += 1;
        assert_eq!(via_relay.hash(), theirs.hash());

        assert_eq!(
            data_from(&a.ingest(IFACE, &theirs, 1)),
            Some((id, b"from b".to_vec()))
        );
        assert_eq!(
            data_from(&a.ingest(IFACE, &theirs, 2)),
            None,
            "a verbatim duplicate is not delivered again"
        );
        assert_eq!(
            data_from(&a.ingest(IFACE, &via_relay, 3)),
            None,
            "the relay's copy is not delivered again"
        );
        assert_eq!(
            a.pause_assessment().latest_link_activity,
            Some(1),
            "a copy is no newer evidence of the far end than the original"
        );

        let next = sent(&b.send(id, IFACE, b"second", &[0x62; 16]).unwrap()).unwrap();
        assert_eq!(
            data_from(&a.ingest(IFACE, &next, 4)),
            Some((id, b"second".to_vec()))
        );
    }

    /// The own-echo and duplicate windows hold their own bounds, not the route table's: a
    /// node with four routes still knows its sixteenth-latest packet, and forgets the oldest
    /// only past the small profile's constant.
    #[test]
    fn link_packet_windows_follow_their_own_constants_not_routes() {
        use crate::capacity::small::{DUPLICATE_HASHES, OWN_ECHO_HASHES};
        let node = |seed, name| {
            Node::<32, 8, 4, 4>::new(
                PrivateIdentity::from_secret_bytes(&[seed; 64]),
                DestinationName::new("retinue", [name]).name_hash(),
            )
        };
        let (mut a, mut b) = (node(0x11, "a"), node(0x22, "b"));
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();
        let proof = sent(&b.ingest(IFACE, &request, 0)).unwrap();
        let id = link_up(&a.ingest(IFACE, &proof, 0)).unwrap();
        let delivered = |actions: &Actions<8>| {
            actions
                .iter()
                .any(|action| matches!(action, Action::Data { .. }))
        };
        let relayed = |packet: &Packet| {
            let mut copy = packet.clone();
            copy.hops += 1;
            copy
        };

        let ours: Vec<Packet> = (0..=OWN_ECHO_HASHES as u8)
            .map(|i| sent(&a.send(id, IFACE, b"ours", &[i; 16]).unwrap()).unwrap())
            .collect();
        assert!(
            !delivered(&a.ingest(IFACE, &relayed(&ours[1]), 1)),
            "an echo deeper than ROUTES is still ours"
        );
        assert!(
            delivered(&a.ingest(IFACE, &relayed(&ours[0]), 1)),
            "past the bound the oldest is forgotten"
        );

        let theirs: Vec<Packet> = (0..=DUPLICATE_HASHES as u8)
            .map(|i| sent(&b.send(id, IFACE, b"theirs", &[0x80 + i; 16]).unwrap()).unwrap())
            .collect();
        for packet in &theirs {
            assert!(delivered(&a.ingest(IFACE, packet, 2)));
        }
        assert!(
            !delivered(&a.ingest(IFACE, &relayed(&theirs[1]), 3)),
            "a copy deeper than ROUTES is still a copy"
        );
        assert!(
            delivered(&a.ingest(IFACE, &relayed(&theirs[0]), 3)),
            "past the bound the oldest is forgotten"
        );
    }

    /// Each dropped own echo and each dropped copy counts once, in its own counter, and a
    /// delivered packet counts in neither.
    #[test]
    fn own_echo_and_duplicate_drops_are_counted_separately() {
        let (mut a, mut b, id) = linked();
        let delivered = |actions: &Actions<8>| {
            actions
                .iter()
                .any(|action| matches!(action, Action::Data { .. }))
        };
        let counts = |node: &Node<32, 8, 4>| {
            let c = node.transport_counters();
            (c.own_echo_dropped, c.duplicate_dropped)
        };
        let relayed = |packet: &Packet| {
            let mut copy = packet.clone();
            copy.hops += 1;
            copy
        };
        assert_eq!(counts(&a), (0, 0));

        let ours = sent(&a.send(id, IFACE, b"ours", &[0x71; 16]).unwrap()).unwrap();
        assert!(delivered(&b.ingest(IFACE, &ours, 1)));
        assert_eq!(counts(&b), (0, 0), "a delivered packet is not counted");
        assert!(!delivered(&a.ingest(IFACE, &relayed(&ours), 1)));
        assert_eq!(counts(&a), (1, 0));
        assert!(!delivered(&a.ingest(IFACE, &relayed(&ours), 2)));
        assert_eq!(counts(&a), (2, 0), "each echo counts once");

        let theirs = sent(&b.send(id, IFACE, b"theirs", &[0x72; 16]).unwrap()).unwrap();
        assert!(delivered(&a.ingest(IFACE, &theirs, 3)));
        assert_eq!(counts(&a), (2, 0), "a delivered packet is not counted");
        assert!(!delivered(&a.ingest(IFACE, &theirs, 4)));
        assert_eq!(counts(&a), (2, 1));
        assert!(!delivered(&a.ingest(IFACE, &relayed(&theirs), 5)));
        assert_eq!(counts(&a), (2, 2), "each copy counts once");

        let next = sent(&b.send(id, IFACE, b"next", &[0x73; 16]).unwrap()).unwrap();
        assert!(delivered(&a.ingest(IFACE, &next, 6)));
        assert_eq!(counts(&a), (2, 2));
        assert_eq!(counts(&b), (0, 0));
    }

    /// A link request for another destination is ignored by a non-transport node and must
    /// never be answered as if its destination were local.
    #[test]
    fn a_link_request_for_another_destination_is_ignored() {
        let (mut a, b) = pair();
        a.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 0);
        let request = sent(&a.open_link(b.destination(), IFACE, &[0x31; 64], 0).unwrap()).unwrap();

        let mut c = Node::<32, 8, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0xCC; 64]),
            DestinationName::new("retinue", ["c"]).name_hash(),
        );
        assert!(c.ingest(IFACE, &request, 0).is_empty());
        assert_eq!(c.link_count(), 0);
    }

    /// A peer closing the link drops it and reports it.
    #[test]
    fn a_peer_closing_the_link_drops_it() {
        let (mut a, b, id) = linked();
        let close = b
            .links
            .iter()
            .find(|(l, _, _)| l.id() == id)
            .map(|(l, _, _)| l.close_packet(&[3; crate::token::IV_LEN]))
            .unwrap();

        let actions = a.ingest(IFACE, &close, 0);
        assert!(actions.iter().any(|x| matches!(x, Action::LinkDown { .. })));
        assert_eq!(a.link_count(), 0, "the link is gone");
        assert!(!a.has_link(id));
    }

    /// A full link table refuses new peers and keeps the ones it has.
    #[test]
    fn a_full_link_table_refuses_and_counts() {
        let mut server = Node::<32, 8, 1>::new(
            PrivateIdentity::from_secret_bytes(&[0x22; 64]),
            DestinationName::new("retinue", ["b"]).name_hash(),
        );
        let mut first = Node::<32, 8, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x11; 64]),
            DestinationName::new("retinue", ["a"]).name_hash(),
        );
        let mut second = Node::<32, 8, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0xDD; 64]),
            DestinationName::new("retinue", ["d"]).name_hash(),
        );
        let ann = server.announce(&blob([2; RAND_HASH_LEN]), None);
        first.ingest(IFACE, &ann, 0);
        second.ingest(IFACE, &ann, 0);

        let r1 = sent(
            &first
                .open_link(server.destination(), IFACE, &[0x31; 64], 0)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(
            server.ingest(IFACE, &r1, 0).len(),
            2,
            "accepted: proof plus LinkUp"
        );

        let r2 = sent(
            &second
                .open_link(server.destination(), IFACE, &[0x41; 64], 0)
                .unwrap(),
        )
        .unwrap();
        assert!(
            server.ingest(IFACE, &r2, 0).is_empty(),
            "refused, and nothing goes to the wire"
        );
        assert_eq!(server.link_count(), 1, "the established link survives");
        assert_eq!(server.refused_links(), 1, "and the refusal is counted");
    }

    /// Link traffic this gate does not handle is dropped rather than mishandled.
    #[test]
    fn unhandled_link_traffic_is_dropped() {
        let (mut a, b, id) = linked();
        let keepalive = b
            .links
            .iter()
            .find(|(l, _, _)| l.id() == id)
            .map(|(l, _, _)| l.keepalive_packet(0xff))
            .unwrap();
        assert!(a.ingest(IFACE, &keepalive, 0).is_empty());
        assert!(a.has_link(id), "and the link survives being spoken to");
    }

    /// Drive every packet between two nodes until neither has anything more to say.
    ///
    /// This is the desk stand-in for a radio: it carries whatever each side wants sent to
    /// the other, in order, with no loss. What it proves is that the two halves of a
    /// transfer agree; loss and retransmission are the medium's business and are measured
    /// on real hardware at the gates.
    fn pump(
        a: &mut Node<32, 8, 4>,
        b: &mut Node<32, 8, 4>,
        first: Actions<8>,
    ) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
        let mut to_b: Vec<Packet> = first
            .iter()
            .filter_map(|x| match x {
                Action::Send { packet, .. } => Some(packet.clone()),
                _ => None,
            })
            .collect();
        let mut to_a: Vec<Packet> = Vec::new();
        let (mut got_a, mut got_b) = (Vec::new(), Vec::new());

        for _ in 0..64 {
            if to_a.is_empty() && to_b.is_empty() {
                break;
            }
            let (mut next_a, mut next_b) = (Vec::new(), Vec::new());

            for packet in to_b.drain(..) {
                for action in b.ingest(IFACE, &packet, 0) {
                    match action {
                        Action::Send { packet, .. } => next_a.push(packet),
                        Action::Resource { data, .. } => got_b.push(data),
                        _ => {}
                    }
                }
            }
            for packet in to_a.drain(..) {
                for action in a.ingest(IFACE, &packet, 0) {
                    match action {
                        Action::Send { packet, .. } => next_b.push(packet),
                        Action::Resource { data, .. } => got_a.push(data),
                        _ => {}
                    }
                }
            }
            to_a = next_a;
            to_b = next_b;
        }
        (got_a, got_b)
    }

    /// A resource crosses a link whole, reassembled and hash-verified.
    ///
    /// Multi-part on purpose: one part would not exercise the request window, the hashmap,
    /// or reassembly, which is where the interesting failures live.
    #[test]
    fn a_resource_crosses_a_link_whole() {
        let (mut a, mut b, id) = linked();
        let payload: Vec<u8> = (0..3_000u32).map(|i| (i.wrapping_mul(31)) as u8).collect();

        let started = a
            .publish(
                id,
                IFACE,
                &payload,
                [0xAB; 4],
                &[5; crate::token::IV_LEN],
                0,
            )
            .expect("a holds the link, so it can publish");
        assert!(a.transfer_active(id), "the transfer is running");

        let (_, got_b) = pump(&mut a, &mut b, started);

        assert_eq!(got_b.len(), 1, "b received exactly one resource");
        assert_eq!(got_b[0], payload, "byte for byte");
        assert!(!b.transfer_active(id), "and b cleared its receiver");
    }

    /// An advertisement past the node's part ceiling is refused, and nothing is held.
    ///
    /// The sender picks the advertised size, so this is the point where a peer's ambition
    /// stops being the board's problem. Without it a peer could name a resource far larger
    /// than the board's memory and the board would try.
    #[test]
    fn an_oversized_resource_is_refused_without_holding_state() {
        let (mut a, mut b, id) = linked();

        // Comfortably past MAX_RESOURCE_PARTS even when compression is enabled.
        // The old repeating-byte fixture compressed below the advertised ceiling.
        let huge: Vec<u8> = (0..2_500u32)
            .flat_map(|i| crate::hash::full_hash(&i.to_le_bytes()))
            .collect();
        let started = a
            .publish(id, IFACE, &huge, [0xCD; 4], &[6; crate::token::IV_LEN], 0)
            .expect("a will happily offer it");

        let advertisement = sent(&started).expect("an advertisement goes out");
        let answer = b.ingest(IFACE, &advertisement, 0);

        assert!(answer.is_empty(), "b says nothing rather than starting");
        assert!(!b.transfer_active(id), "and holds no reassembly state");
        assert!(b.has_link(id), "while the link itself is untouched");
    }

    /// Run a transfer from `a` to `b` until `b` proves receipt, returning that proof
    /// undelivered. `a`'s sender is still waiting for it.
    fn transfer_until_proof(
        a: &mut Node<32, 8, 4>,
        b: &mut Node<32, 8, 4>,
        id: AddressHash,
    ) -> Packet {
        let payload: Vec<u8> = (0..2_000u32).map(|i| (i.wrapping_mul(13)) as u8).collect();
        let started = a
            .publish(
                id,
                IFACE,
                &payload,
                [0x5A; 4],
                &[8; crate::token::IV_LEN],
                0,
            )
            .unwrap();
        let mut to_b = vec![sent(&started).unwrap()];
        for _ in 0..64 {
            let mut to_a = Vec::new();
            for packet in to_b.drain(..) {
                for action in b.ingest(IFACE, &packet, 0) {
                    if let Action::Send { packet, .. } = action {
                        to_a.push(packet);
                    }
                }
            }
            if let Some(index) = to_a
                .iter()
                .position(|p| p.context == link::CTX_RESOURCE_PRF)
            {
                return to_a.swap_remove(index);
            }
            for packet in to_a {
                for action in a.ingest(IFACE, &packet, 0) {
                    if let Action::Send { packet, .. } = action {
                        to_b.push(packet);
                    }
                }
            }
        }
        panic!("b never proved receipt");
    }

    /// A Node proves a resource with the PROOF-type packet RNS accepts, and a Node sender
    /// completes on one. Before, a PROOF-type packet only ever reached link setup, so a
    /// Node publishing to RNS never saw its receipt and held the sender until it expired.
    #[test]
    fn a_proof_type_resource_proof_completes_a_node_sender() {
        let (mut a, mut b, id) = linked();
        let proof = transfer_until_proof(&mut a, &mut b, id);
        assert_eq!(proof.packet_type, PacketType::Proof);
        assert!(a.transfer_active(id), "a is still waiting for the receipt");

        assert!(a.ingest(IFACE, &proof, 0).is_empty());
        assert!(
            !a.transfer_active(id),
            "the PROOF-type receipt completes a's sender"
        );
        assert!(a.has_link(id));
    }

    /// For one release a Node sender still accepts the DATA-type proof older retinue sent.
    #[test]
    fn a_node_sender_still_accepts_the_legacy_data_type_proof() {
        let (mut a, mut b, id) = linked();
        let mut proof = transfer_until_proof(&mut a, &mut b, id);
        proof.packet_type = PacketType::Data;
        a.ingest(IFACE, &proof, 0);
        assert!(!a.transfer_active(id));
    }

    /// A PROOF-type resource proof on a link with no outbound transfer is dropped: it is
    /// not an offer, so it neither opens a receiver nor counts as a refused one.
    #[test]
    fn a_stray_resource_proof_opens_nothing() {
        let (mut a, b, id) = linked();
        let link = b.links.iter().find(|(l, _, _)| l.id() == id).unwrap();
        let stray = link.0.resource_proof_packet(&[1; 32], &[2; 32]);
        assert!(a.ingest(IFACE, &stray, 0).is_empty());
        assert!(!a.transfer_active(id));
        assert_eq!(a.refused_offers(), 0);
    }

    /// A multi-segment offer is refused with a sealed cancel and holds no state, rather
    /// than being received as its first segment.
    #[test]
    fn a_multi_segment_offer_is_refused_with_a_cancel() {
        let (a, mut b, id) = linked();
        let link = a
            .links
            .iter()
            .find(|(l, _, _)| l.id() == id)
            .unwrap()
            .0
            .clone();
        let segment = [0x42_u8; 600];
        let random_hash = [1, 2, 3, 4];
        let iv = [0x11; crate::token::IV_LEN];
        let token = link.seal(&crate::resource::content(&segment, &random_hash), &iv);
        let out = crate::resource::Outgoing::new(&segment, &token, random_hash, false)
            .with_segment(
                1,
                3,
                1_800,
                crate::resource::resource_hash(&segment, &random_hash),
            );
        let advertisement =
            link.sealed_packet(link::CTX_RESOURCE_ADV, &out.advertisement().pack(), &iv);

        let answer = b.ingest(IFACE, &advertisement, 0);
        let cancel = sent(&answer).expect("b tells the sender to stop");
        assert_eq!(cancel.context, link::CTX_RESOURCE_RCL);
        assert_eq!(link.decrypt(&cancel).unwrap(), out.resource_hash().to_vec());
        assert!(
            !answer.iter().any(|x| matches!(x, Action::Resource { .. })),
            "no data"
        );
        assert!(!b.transfer_active(id), "and holds no reassembly state");
        assert_eq!(b.refused_offers(), 1);
    }

    /// A shell that could not send the announce can say so, and the next poll announces
    /// again instead of waiting out the whole interval.
    ///
    /// Found on hardware: a jammed channel made listen-before-talk refuse the announce,
    /// and the board then believed it had announced — invisible for ten minutes after a
    /// ten-second jam.
    #[test]
    fn a_failed_announce_can_be_retried_before_the_interval() {
        let (mut a, _b) = pair();

        assert!(
            sent(&a.poll(0, IFACE, Some(&blob([1; RAND_HASH_LEN])))).is_some(),
            "the first poll announces"
        );
        assert!(
            a.poll(1_000, IFACE, Some(&blob([2; RAND_HASH_LEN])))
                .is_empty(),
            "and the next is not due for a whole interval"
        );
        assert!(!a.announce_due(1_000), "the interval is not elapsed yet");

        // The shell reports that the frame never reached the air.
        a.retry_announce();
        assert!(
            a.announce_due(1_001),
            "retry makes the announce due immediately"
        );
        assert!(
            sent(&a.poll(1_001, IFACE, Some(&blob([3; RAND_HASH_LEN])))).is_some(),
            "so the node announces again rather than waiting out the interval"
        );
    }

    /// One lost part no longer kills a transfer: the receiver's poll re-requests exactly
    /// what is missing, and the sender serves it. This is the mechanism N5's first hardware
    /// run proved was absent, when one dropped frame at SF11 stalled a five-part transfer
    /// forever on a clean link.
    #[test]
    fn a_lost_part_is_re_requested_on_poll() {
        let (mut a, mut b, id) = linked();
        // Drain the boot announce, so later polls answer only for the transfer.
        let _ = b.poll(0, IFACE, Some(&blob([0; RAND_HASH_LEN])));
        let payload: Vec<u8> = (0..1_024u32).map(|i| (i.wrapping_mul(7)) as u8).collect();

        let started = a
            .publish(
                id,
                IFACE,
                &payload,
                [0xEE; 4],
                &[7; crate::token::IV_LEN],
                0,
            )
            .unwrap();

        // Deliver the advertisement, take b's request, serve it — but LOSE one part.
        let advertisement = sent(&started).unwrap();
        let request = sent(&b.ingest(IFACE, &advertisement, 0)).unwrap();
        let parts: Vec<Packet> = a
            .ingest(IFACE, &request, 0)
            .into_iter()
            .filter_map(|x| match x {
                Action::Send { packet, .. } => Some(packet),
                _ => None,
            })
            .collect();
        assert!(parts.len() >= 2, "the window carries several parts");
        let mut arrived = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            if index == 1 {
                continue; // the air ate it
            }
            arrived.extend(b.ingest(IFACE, part, 0));
        }
        assert!(
            !arrived.iter().any(|x| matches!(x, Action::Send { .. })),
            "with a part outstanding, b waits rather than re-requesting early"
        );
        assert!(b.transfer_active(id), "the transfer is stalled, not dead");

        // Before the retry interval: silence. At it: the re-request, unprompted.
        assert!(
            b.poll(
                RESOURCE_RETRY_INTERVAL - 1,
                IFACE,
                Some(&blob([0; RAND_HASH_LEN]))
            )
            .is_empty(),
            "no retry before its time"
        );
        let retry = sent(&b.poll(
            RESOURCE_RETRY_INTERVAL,
            IFACE,
            Some(&blob([0; RAND_HASH_LEN])),
        ))
        .expect("the poll re-requests the missing part");

        // The sender answers with the missing part, and the transfer completes.
        let served: Vec<Packet> = a
            .ingest(IFACE, &retry, 0)
            .into_iter()
            .filter_map(|x| match x {
                Action::Send { packet, .. } => Some(packet),
                _ => None,
            })
            .collect();
        let mut done = Vec::new();
        for part in &served {
            done.extend(b.ingest(IFACE, part, 0));
        }
        // Drain the remaining request/serve rounds if any, then check the payload landed.
        let mut to_a: Vec<Packet> = done
            .iter()
            .filter_map(|x| match x {
                Action::Send { packet, .. } => Some(packet.clone()),
                _ => None,
            })
            .collect();
        let mut received: Vec<Vec<u8>> = done
            .iter()
            .filter_map(|x| match x {
                Action::Resource { data, .. } => Some(data.clone()),
                _ => None,
            })
            .collect();
        for _ in 0..16 {
            if to_a.is_empty() {
                break;
            }
            let mut to_b = Vec::new();
            for packet in to_a.drain(..) {
                for action in a.ingest(IFACE, &packet, 0) {
                    if let Action::Send { packet, .. } = action {
                        to_b.push(packet);
                    }
                }
            }
            for packet in to_b {
                for action in b.ingest(IFACE, &packet, 0) {
                    match action {
                        Action::Send { packet, .. } => to_a.push(packet),
                        Action::Resource { data, .. } => received.push(data),
                        _ => {}
                    }
                }
            }
        }
        assert_eq!(received, vec![payload], "byte for byte, after the loss");
        assert!(
            !b.transfer_active(id),
            "and the receiver slot is free again"
        );
    }

    /// A lost advertisement is re-offered by the sender's poll, so a fetch whose first
    /// offer the air ate still begins.
    #[test]
    fn a_lost_advertisement_is_re_offered_on_poll() {
        let (mut a, mut b, id) = linked();
        // Drain the boot announce, so the retry poll answers only for the transfer.
        let _ = a.poll(0, IFACE, Some(&blob([0; RAND_HASH_LEN])));
        let payload: Vec<u8> = (0..600u32).map(|i| i as u8).collect();

        // The advertisement from publish is LOST: b never hears it.
        let _ = a
            .publish(
                id,
                IFACE,
                &payload,
                [0xEF; 4],
                &[8; crate::token::IV_LEN],
                0,
            )
            .unwrap();
        assert!(a.transfer_active(id));

        let again = sent(&a.poll(
            RESOURCE_RETRY_INTERVAL,
            IFACE,
            Some(&blob([0; RAND_HASH_LEN])),
        ))
        .expect("the poll re-advertises the unanswered offer");
        let request = sent(&b.ingest(IFACE, &again, 0));
        assert!(request.is_some(), "and the re-offer starts the transfer");
    }

    /// Two sealed packets never share an IV, even across separate ingest calls. The
    /// counter is node state; a fresh counter per call would replay the sequence.
    #[test]
    fn derived_ivs_never_repeat_across_calls() {
        let (mut a, mut b, id) = linked();
        let payload: Vec<u8> = (0..600u32).map(|i| i as u8).collect();

        let started = a
            .publish(
                id,
                IFACE,
                &payload,
                [0xAA; 4],
                &[9; crate::token::IV_LEN],
                0,
            )
            .unwrap();
        let advertisement = sent(&started).unwrap();
        let first = sent(&b.ingest(IFACE, &advertisement, 0)).expect("first request");
        // The same advertisement again: the receiver rebuilds the same logical request. If
        // IVs repeated, the sealed bytes would be identical.
        let second = sent(&b.ingest(IFACE, &advertisement, 0)).expect("second request");
        assert_ne!(
            first.payload, second.payload,
            "the same request sealed twice must differ, or the IV repeated"
        );
    }

    /// Losing the link discards the transfer riding on it.
    ///
    /// Reassembly state without a link is memory held for a peer that is gone, which on a
    /// board is exactly the leak worth preventing.
    #[test]
    fn closing_a_link_discards_its_transfer() {
        let (mut a, mut b, id) = linked();
        let payload: Vec<u8> = (0..3_000u32).map(|i| i as u8).collect();

        // Start a transfer and deliver only the advertisement, so b is mid-receive.
        let started = a
            .publish(
                id,
                IFACE,
                &payload,
                [0xAB; 4],
                &[5; crate::token::IV_LEN],
                0,
            )
            .unwrap();
        let advertisement = sent(&started).unwrap();
        b.ingest(IFACE, &advertisement, 0);
        assert!(b.transfer_active(id), "b is mid-transfer");

        // a closes the link.
        let close = a
            .links
            .iter()
            .find(|(l, _, _)| l.id() == id)
            .map(|(l, _, _)| l.close_packet(&[9; crate::token::IV_LEN]))
            .unwrap();
        let actions = b.ingest(IFACE, &close, 0);

        assert!(actions.iter().any(|x| matches!(x, Action::LinkDown { .. })));
        assert!(!b.transfer_active(id), "the transfer went with the link");
        assert_eq!(b.link_count(), 0);
    }

    /// One transfer per link at a time: a board cannot hold two.
    /// A peer that establishes a link and then vanishes used to hold its slot forever: a
    /// board that lost power sends no close, and nothing else freed one. Four such absences
    /// bricked a node as a router until somebody rebooted it.
    #[test]
    fn a_silent_peer_releases_its_link_slot() {
        let (mut a, _b, _id) = linked();
        assert_eq!(a.link_count(), 1, "the link is up");

        // Nobody says anything for longer than the timeout, then the node's clock ticks.
        let later = LINK_IDLE_TIMEOUT + 1;
        let _ = a.poll(later, IFACE, Some(&blob([0x11; RAND_HASH_LEN])));

        assert_eq!(a.link_count(), 0, "a silent slot must come back");
        assert_eq!(
            a.expired_links(),
            1,
            "and be attributable, so a busy node reads differently from a deserted one",
        );
    }

    /// The other half: a link being used must not be reclaimed underneath it.
    #[test]
    fn a_link_that_keeps_talking_keeps_its_slot() {
        let (mut a, b, id) = linked();
        let keepalive = b
            .links
            .iter()
            .find(|(l, _, _)| l.id() == id)
            .map(|(l, _, _)| l.keepalive_packet(0xff))
            .unwrap();

        let mut now = 0;
        for _ in 0..4 {
            now += LINK_IDLE_TIMEOUT - 1;
            a.ingest(IFACE, &keepalive, now);
            let _ = a.poll(now, IFACE, Some(&blob([0x22; RAND_HASH_LEN])));
            assert_eq!(a.link_count(), 1, "a live peer keeps its slot at {now}");
        }
        assert_eq!(a.expired_links(), 0, "nothing reclaimed from a live peer");
    }

    #[test]
    fn a_second_publish_on_a_busy_link_is_refused() {
        let (mut a, _b, id) = linked();
        let payload = vec![1_u8; 1_000];

        assert!(
            a.publish(id, IFACE, &payload, [1; 4], &[1; crate::token::IV_LEN], 0)
                .is_some(),
            "the first publish starts"
        );
        assert!(
            a.publish(id, IFACE, &payload, [2; 4], &[2; crate::token::IV_LEN], 0)
                .is_none(),
            "the second is refused while the first runs"
        );
    }

    /// The transport table is a fixed board resource: expired paths go first, then the
    /// quietest live route makes room. A flood cannot turn it into a lifetime allocation.
    #[test]
    fn transport_routes_expire_then_evict_at_their_bound() {
        let mut relay = Node::<8, 8, 4, 2>::new(
            PrivateIdentity::from_secret_bytes(&[0x50; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig {
            route_ttl: 100,
            ..TransportConfig::transit()
        });
        let peer = |seed, name| {
            Node::<8, 8, 4, 2>::new(
                PrivateIdentity::from_secret_bytes(&[seed; 64]),
                DestinationName::new("retinue", [name]).name_hash(),
            )
        };
        let a = peer(0x11, "a");
        let b = peer(0x22, "b");
        let c = peer(0x33, "c");

        relay.ingest(IFACE, &a.announce(&blob([1; RAND_HASH_LEN]), None), 0);
        relay.ingest(IFACE, &b.announce(&blob([2; RAND_HASH_LEN]), None), 1);
        assert_eq!(relay.route_count(), 2, "the typed route bound is full");

        relay.ingest(IFACE, &c.announce(&blob([3; RAND_HASH_LEN]), None), 2);
        assert_eq!(
            relay.route_count(),
            2,
            "a third route displaces, never grows"
        );
        assert_eq!(
            relay.route_to(a.destination(), 2),
            None,
            "the quietest live route was evicted"
        );
        assert_eq!(relay.transport_counters().evicted_routes, 1);

        let _ = relay.poll(102, IFACE, Some(&blob([0; RAND_HASH_LEN])));
        assert_eq!(relay.route_count(), 0, "stale routes are reclaimed by poll");
        assert_eq!(relay.transport_counters().expired_routes, 2);
    }

    /// A transport node relays both sides of a link setup: the announce makes the route
    /// visible, the type-2 request reaches the destination, and the remembered link bridge
    /// returns its proof. This is the smallest real transport transaction, not a broadcast
    /// counter that could pass without carrying a packet.
    #[test]
    fn transport_relays_announce_request_and_proof() {
        let (mut source, mut destination) = pair();
        let mut relay = Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x44; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());

        let announce = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
        let relayed_announce = sent(&relay.ingest(IFACE, &announce, 0))
            .expect("a transport node re-broadcasts a verified announce");
        assert_eq!(relayed_announce.header_type, HeaderType::Type2);
        assert_eq!(relayed_announce.transport, Some(relay.identity.hash()));
        assert_eq!(relayed_announce.hops, 1);
        source.ingest(IFACE, &relayed_announce, 1);
        assert!(
            source.peers().knows(destination.destination()),
            "the source learned the destination through the relay"
        );

        let mut request = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x99; 64], 1)
                .expect("the announced destination is linkable"),
        )
        .unwrap();
        request.header_type = HeaderType::Type2;
        request.transport = Some(relay.identity.hash());
        let forwarded_request = sent(&relay.ingest(IFACE, &request, 2))
            .expect("the type-2 request is carried toward its route");
        assert_eq!(forwarded_request.header_type, HeaderType::Type1);
        assert_eq!(forwarded_request.transport, None);
        assert_eq!(forwarded_request.hops, 1);

        let proof = sent(&destination.ingest(IFACE, &forwarded_request, 3))
            .expect("the destination accepts the transported request");
        let forwarded_proof = sent(&relay.ingest(IFACE, &proof, 4))
            .expect("the remembered bridge carries the proof back");
        assert_eq!(forwarded_proof.hops, 1);
        assert!(
            link_up(&source.ingest(IFACE, &forwarded_proof, 5)).is_some(),
            "the source completes the transported link"
        );
        let counters = relay.transport_counters();
        assert_eq!(counters.forwarded_announces, 1);
        assert_eq!(counters.forwarded_packets, 2);

        // An explicit interruption reports the transit obligation without
        // pretending to close either remote endpoint. Late link data loses its
        // return path, while learned destination/freshness state is retained.
        let id = source.links[0].0.id();
        let late = sent(
            &source
                .send(id, IFACE, b"after-relay-loss", &[0xA1; 16])
                .unwrap(),
        )
        .unwrap();
        assert_eq!(relay.pause_assessment().transit_bridges, 1);
        let report = relay
            .force_interrupt(InterruptionPermission::AllowSessionLoss, || {
                panic!("transit-only node has no local link to close")
            })
            .unwrap();
        assert_eq!(report.transit_bridges.as_slice(), [id]);
        assert!(report.closed_links.is_empty());
        assert_eq!(relay.pause_assessment().transit_bridges, 0);
        assert!(relay.ingest(IFACE, &late, 6).is_empty());
        assert!(relay.peers().knows(destination.destination()));
    }

    /// A relay remembers the way back for every packet it carries, and carries the proof
    /// back once, only from the interface the packet left by, within `REVERSE_TIMEOUT`
    /// (RNS `Transport.py` 2104-2110, 863-870, 2733-2744).
    #[test]
    fn transport_carries_a_single_packet_proof_back_once() {
        const OUT: InterfaceId = IFACE + 1;
        let (_, destination) = pair();
        let mut relay = Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x44; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        relay.ingest(
            OUT,
            &destination.announce(&blob([0x77; RAND_HASH_LEN]), None),
            0,
        );

        let carry = |relay: &mut Node<32, 8, 4, 4>, byte: u8, now: u64| {
            let packet = Packet {
                ifac: false,
                header_type: HeaderType::Type2,
                context_flag: false,
                propagation: crate::packet::Propagation::Transport,
                destination_type: crate::packet::DestinationType::Single,
                packet_type: PacketType::Data,
                hops: 0,
                transport: Some(relay.identity.hash()),
                destination: destination.destination(),
                context: 0,
                payload: vec![byte; 48],
            };
            let actions = relay.ingest(IFACE, &packet, now);
            assert!(matches!(
                actions.iter().next(),
                Some(Action::Send { interface: OUT, .. })
            ));
            crate::proof::proof_packet(&destination.identity, &packet.full_hash(), true)
        };

        // Carried back from the egress interface, once.
        let proof = carry(&mut relay, 1, 1);
        let actions = relay.ingest(OUT, &proof, 2);
        let Some(Action::Send { interface, packet }) = actions.iter().next() else {
            panic!("the proof is carried back");
        };
        assert_eq!(*interface, IFACE);
        assert_eq!(packet.hops, proof.hops + 1);
        assert_eq!(packet.payload, proof.payload);
        assert!(
            relay.ingest(OUT, &proof, 3).is_empty(),
            "the entry was consumed"
        );

        // A proof from any other interface consumes the entry without being carried.
        let proof = carry(&mut relay, 2, 4);
        assert!(relay.ingest(IFACE + 2, &proof, 5).is_empty());
        assert!(relay.ingest(OUT, &proof, 6).is_empty());

        // The way back is forgotten after REVERSE_TIMEOUT.
        let proof = carry(&mut relay, 3, 10);
        assert!(relay.ingest(OUT, &proof, 10 + REVERSE_TIMEOUT).is_empty());
        assert_eq!(relay.transport_counters().forwarded_packets, 4);
    }

    /// The reverse table is bounded by `ROUTES`: the oldest way back gives way.
    #[test]
    fn reverse_table_is_bounded_by_routes() {
        let (_, destination) = pair();
        let mut relay = Node::<32, 8, 4, 2>::new(
            PrivateIdentity::from_secret_bytes(&[0x44; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        relay.ingest(
            IFACE + 1,
            &destination.announce(&blob([0x77; RAND_HASH_LEN]), None),
            0,
        );
        let mut proofs = vec![];
        for byte in 0..3u8 {
            let packet = Packet {
                ifac: false,
                header_type: HeaderType::Type2,
                context_flag: false,
                propagation: crate::packet::Propagation::Transport,
                destination_type: crate::packet::DestinationType::Single,
                packet_type: PacketType::Data,
                hops: 0,
                transport: Some(relay.identity.hash()),
                destination: destination.destination(),
                context: 0,
                payload: vec![byte; 48],
            };
            assert!(sent(&relay.ingest(IFACE, &packet, u64::from(byte) + 1)).is_some());
            proofs.push(crate::proof::proof_packet(
                &destination.identity,
                &packet.full_hash(),
                false,
            ));
        }
        assert_eq!(relay.reverse.len(), 2);
        assert!(relay.ingest(IFACE + 1, &proofs[0], 5).is_empty());
        assert!(sent(&relay.ingest(IFACE + 1, &proofs[1], 5)).is_some());
        assert!(sent(&relay.ingest(IFACE + 1, &proofs[2], 5)).is_some());
    }

    /// A validly signed announce naming a known destination under a different key is
    /// rejected before it can touch freshness, a route or a relay (RNS `Identity.py`
    /// 569-577).
    #[test]
    fn an_announce_with_a_different_key_for_a_known_destination_is_rejected() {
        let (_, destination) = pair();
        let mut relay = Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x44; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let packet = destination.announce(&blob([0x77; RAND_HASH_LEN]), None);
        // Stand in for an earlier announce of the same destination hash under another key:
        // a real one would need a hash collision.
        let mut known = Announce::decode(&packet).unwrap();
        known.identity = *PrivateIdentity::from_secret_bytes(&[0x45; 64]).public();
        assert_eq!(relay.book.ingest(&known), Ingested::Learned);

        assert!(relay.ingest(IFACE, &packet, 0).is_empty());
        assert_eq!(relay.transport_counters().key_mismatch_announces, 1);
        assert_eq!(relay.route_count(), 0);
        assert_eq!(
            relay
                .peers()
                .resolve(destination.destination())
                .unwrap()
                .identity,
            known.identity
        );
    }

    /// A transit source addresses its own request to the relay that taught it the route, and
    /// reports that relay as the next hop until the route's TTL passes.
    #[test]
    fn open_link_addresses_the_first_relay() {
        let (_, mut destination) = pair();
        let transit = |seed, name| {
            Node::<32, 8, 4, 4>::new(
                PrivateIdentity::from_secret_bytes(&[seed; 64]),
                DestinationName::new("retinue", [name]).name_hash(),
            )
            .with_transport_config(TransportConfig::transit())
        };
        let mut source = transit(0x46, "source");
        let mut relay = transit(0x47, "relay");

        let announce = destination.announce(&blob([0x78; RAND_HASH_LEN]), None);
        let relayed = sent(&relay.ingest(IFACE, &announce, 0)).unwrap();
        source.ingest(IFACE, &relayed, 1);
        let hop = source.next_hop(destination.destination(), 1).unwrap();
        assert_eq!(hop.via, Some(relay.identity.hash()));
        assert_eq!(hop.hops, 1);
        assert_eq!(
            relay.next_hop(destination.destination(), 1).unwrap().via,
            None
        );

        let request = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x9A; 64], 1)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(request.header_type, HeaderType::Type2);
        assert_eq!(request.transport, Some(relay.identity.hash()));
        let forwarded = sent(&relay.ingest(IFACE, &request, 2)).unwrap();
        let proof = sent(&destination.ingest(IFACE, &forwarded, 3)).unwrap();
        let back = sent(&relay.ingest(IFACE, &proof, 4)).unwrap();
        assert!(link_up(&source.ingest(IFACE, &back, 5)).is_some());

        let expired = 1 + DEFAULT_ROUTE_TTL;
        assert_eq!(source.next_hop(destination.destination(), expired), None);
        assert_eq!(
            source.route_count(),
            1,
            "the read-only accessor does not evict"
        );
    }

    /// `open_link` reads the route's TTL at its own `now`: one tick before expiry it addresses
    /// the relay, and at expiry it does not, though nothing has evicted the route yet.
    #[test]
    fn open_link_does_not_address_via_an_expired_unevicted_route() {
        let (mut source, destination) = pair();
        let mut relay = Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x4A; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let announce = destination.announce(&blob([0x7A; RAND_HASH_LEN]), None);
        let learned = 1;
        source.ingest(
            IFACE,
            &sent(&relay.ingest(IFACE, &announce, 0)).unwrap(),
            learned,
        );
        let expiry = learned + DEFAULT_ROUTE_TTL;

        let fresh = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x9C; 64], expiry - 1)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(fresh.header_type, HeaderType::Type2);
        assert_eq!(fresh.transport, Some(relay.identity.hash()));

        let stale = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x9D; 64], expiry)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(source.route_count(), 1, "the expired route is not evicted");
        assert_eq!(stale.header_type, HeaderType::Type1);
        assert_eq!(stale.transport, None);
    }

    /// A leaf learns routes without relaying: it addresses its first relay and reaches a
    /// destination two relays away, yet carries nothing for anyone else.
    #[test]
    fn non_transit_sender_addresses_its_first_relay_and_forwards_nothing() {
        let (mut source, mut destination) = pair();
        assert_eq!(source.transport_config(), TransportConfig::none());
        let transit = |seed, name| {
            Node::<32, 8, 4, 4>::new(
                PrivateIdentity::from_secret_bytes(&[seed; 64]),
                DestinationName::new("retinue", [name]).name_hash(),
            )
            .with_transport_config(TransportConfig::transit())
        };
        // source - near - far - destination
        let mut near = transit(0x48, "near");
        let mut far = transit(0x49, "far");

        let announce = destination.announce(&blob([0x79; RAND_HASH_LEN]), None);
        let via_far = sent(&far.ingest(IFACE, &announce, 0)).unwrap();
        let via_near = sent(&near.ingest(IFACE, &via_far, 1)).unwrap();
        let heard = source.ingest(IFACE, &via_near, 2);
        assert!(sent(&heard).is_none(), "a leaf does not re-broadcast");
        let hop = source.next_hop(destination.destination(), 2).unwrap();
        assert_eq!((hop.via, hop.hops), (Some(near.identity.hash()), 2));

        let request = sent(
            &source
                .open_link(destination.destination(), IFACE, &[0x9B; 64], 2)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(request.header_type, HeaderType::Type2);
        assert_eq!(request.transport, Some(near.identity.hash()));
        let at_far = sent(&near.ingest(IFACE, &request, 3)).unwrap();
        assert_eq!(at_far.transport, Some(far.identity.hash()));
        let at_destination = sent(&far.ingest(IFACE, &at_far, 4)).unwrap();
        let proof = sent(&destination.ingest(IFACE, &at_destination, 5)).unwrap();
        let proof = sent(&far.ingest(IFACE, &proof, 6)).unwrap();
        let proof = sent(&near.ingest(IFACE, &proof, 7)).unwrap();
        let id = link_up(&source.ingest(IFACE, &proof, 8)).expect("the link comes up");

        let data = sent(&source.send(id, IFACE, b"two relays", &[0xB1; 16]).unwrap()).unwrap();
        let data = sent(&near.ingest(IFACE, &data, 9)).unwrap();
        let data = sent(&far.ingest(IFACE, &data, 10)).unwrap();
        assert!(destination.ingest(IFACE, &data, 11).iter().any(
            |action| matches!(action, Action::Data { payload, .. } if payload == b"two relays")
        ));

        // A request naming the leaf as its transport, for a destination it has a route to.
        let mut through_source = request.clone();
        through_source.transport = Some(source.identity.hash());
        assert!(sent(&source.ingest(IFACE, &through_source, 12)).is_none());
        let counters = source.transport_counters();
        assert_eq!(
            (counters.forwarded_announces, counters.forwarded_packets),
            (0, 0)
        );
    }

    #[test]
    fn foreign_interface_cannot_refresh_or_poison_a_link_bridge() {
        let mut relay = Node::<8, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x45; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());
        let link_id = AddressHash::from_bytes([0xA3; 16]);
        relay.remember_bridge(link_id, 1, 2, 10);
        let mut packet = Packet {
            packet_type: PacketType::Data,
            header_type: HeaderType::Type1,
            transport: None,
            destination: link_id,
            payload: b"bridged data".to_vec(),
            ..fixture("announce_appdata.bin")
        };

        assert!(relay.ingest(3, &packet, 20).is_empty());
        assert_eq!(relay.bridges[0].seen, 10);
        assert!(relay.seen_transit.is_empty());

        let to_two = relay.ingest(1, &packet, 21);
        assert!(
            to_two
                .iter()
                .any(|action| matches!(action, Action::Send { interface: 2, .. }))
        );
        assert_eq!(relay.bridges[0].seen, 21);

        packet.payload = b"return data".to_vec();
        let to_one = relay.ingest(2, &packet, 22);
        assert!(
            to_one
                .iter()
                .any(|action| matches!(action, Action::Send { interface: 1, .. }))
        );
        assert_eq!(relay.transport_counters().forwarded_packets, 2);
    }

    /// This is the desk half of the T114 flood: enough distinct signed announces to turn the
    /// route table over many times, while every retained table stays at its declared ceiling.
    /// The board's allocator probe supplies the separate live-byte high-water receipt.
    #[test]
    fn transport_flood_keeps_retained_state_bounded() {
        let mut relay = Node::<128, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x55; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());

        for seed in 1_u8..=32 {
            let peer = Node::<128, 8, 4, 4>::new(
                PrivateIdentity::from_secret_bytes(&[seed; 64]),
                DestinationName::new("retinue", ["flood"]).name_hash(),
            );
            let actions = relay.ingest(
                IFACE,
                &peer.announce(&blob([seed; RAND_HASH_LEN]), None),
                seed.into(),
            );
            assert!(actions.len() <= 2, "one learn and one relay at most");
            assert_eq!(
                relay.route_count(),
                usize::from(seed).min(4),
                "route residency remains at its four-entry ceiling"
            );
        }
        let counters = relay.transport_counters();
        assert_eq!(counters.forwarded_announces, 32);
        assert_eq!(counters.evicted_routes, 28);
        assert_eq!(relay.route_count(), 4);
    }

    /// A relay rebroadcasts a node's own announce, so the node hears itself. The echo must not
    /// make the node its own peer: a five-node mesh reported five peers per node instead of four,
    /// and that count reaches the device's PEERS and STATUS pages. Genuine peers still learn the
    /// announce, and the relay still rebroadcasts it for them.
    #[test]
    fn own_announce_echoed_by_a_relay_is_not_a_peer() {
        let (mut a, mut b) = pair();
        let mut relay = Node::<32, 8, 4, 4>::new(
            PrivateIdentity::from_secret_bytes(&[0x46; 64]),
            DestinationName::new("retinue", ["relay"]).name_hash(),
        )
        .with_transport_config(TransportConfig::transit());

        let announce = a.announce(&blob([0x31; RAND_HASH_LEN]), None);
        let echo = sent(&relay.ingest(IFACE, &announce, 0))
            .expect("the relay still rebroadcasts the announce for others");
        assert_eq!(relay.transport_counters().forwarded_announces, 1);

        let heard = a.ingest(IFACE, &echo, 1);
        assert_eq!(a.peers().len(), 0, "a node is not its own peer");
        assert!(!a.peers().knows(a.destination()));
        assert!(
            heard.is_empty(),
            "the echo of our own announce does nothing"
        );
        assert_eq!(a.route_count(), 0, "no route to ourselves is learned");

        b.ingest(IFACE, &echo, 1);
        assert!(
            b.peers().knows(a.destination()),
            "a genuine peer is learned"
        );
        assert_eq!(b.peers().len(), 1);
        assert!(relay.peers().knows(a.destination()));
    }
}
