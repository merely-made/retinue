//! Configuration types and the protocol constants behind them.

#[cfg(doc)]
use super::Node;
use crate::announce;
#[cfg(doc)]
use crate::packet::Packet;

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
/// The sender picks the part count. Thirty-two parts is about 13 KB of reassembly at the
/// default part size, which a 256 KB board can hold; the desktop's 4096 (1.7 MB) it cannot.
pub const MAX_RESOURCE_PARTS: usize = 32;

/// Payload budgets for one Node. Table counts remain its const parameters.
///
/// The default is unbounded, for host callers; embedded callers choose finite values with
/// [`Node::new_with_payload_limits`], and must still bound raw input before decoding a
/// [`Packet`] and bound retained action queues. An inbound uncompressed resource is bounded by
/// `max_resource_parts` times `max_ingress_bytes`. With `compression` on, a compressed one is
/// also refused past [`DEFAULT_MAX_DECOMPRESSED_SIZE`](crate::resource::DEFAULT_MAX_DECOMPRESSED_SIZE).
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

/// The round trip a resource's watchdog assumes on a link whose RTT is not yet measured,
/// in the caller's tick unit (milliseconds on the boards): about a request-plus-part round
/// trip at the slowest profile, SF11/250 kHz.
pub const RESOURCE_FALLBACK_RTT: u64 = 3_000;

/// How long a link keeps the last resource proof this node sent, to answer a sender's cache
/// request for it, in milliseconds. A sender asks [`PROOF_CACHE_REQUESTS`] times, each
/// after RTT × 3 + 10 s of silence, so this covers them with room for slow airtime.
///
/// [`PROOF_CACHE_REQUESTS`]: crate::resource_transfer::PROOF_CACHE_REQUESTS
pub const RESOURCE_PROOF_CACHE_TTL: u64 = 120_000;

/// How long a link may go unheard before its slot is reclaimed, in milliseconds.
///
/// A vanished peer sends no close, so without expiry four of them would hold a board's four
/// slots for good. Fifteen minutes is far longer than RNS keepalives, so a live idle peer is
/// never evicted.
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
/// (`testing/receipts/rns-1.5.4-link-echo-corroboration`, Q1). Without a deadline, unanswered
/// requests would hold their pending slots for good.
pub const fn link_request_timeout(relays: u8) -> u64 {
    LINK_ESTABLISHMENT_TIMEOUT_PER_HOP * (relays as u64 + 2)
}

/// How long a transport hop holds a carried link request open for its proof, in milliseconds,
/// before the outgoing interface's first-hop airtime: one per-hop allowance for each hop still
/// ahead (`relays + 1`), as RNS's `ESTABLISHMENT_TIMEOUT_PER_HOP * max(1, remaining_hops)`.
pub(crate) const fn transit_proof_timeout(relays: u8) -> u64 {
    LINK_ESTABLISHMENT_TIMEOUT_PER_HOP * (relays as u64 + 1)
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
/// How long a learned transport route is usable, in the caller's tick unit (milliseconds on
/// the boards): one week, RNS's `PATHFINDER_E` and `DESTINATION_TIMEOUT` (`Transport.py`
/// 126, 155).
///
/// A route's announce freshness lives exactly as long as the route, so a shorter lifetime
/// would let an older emission back in once it lapsed. Traffic refreshes a route, and
/// `ROUTES` bounds the table: the quietest route is evicted to admit a new destination.
pub const DEFAULT_ROUTE_TTL: u64 = 604_800_000;

/// Route lifetime on an [`InterfaceMode::AccessPoint`] interface: one day, RNS's
/// `AP_PATH_TIME` (`Transport.py` 127).
pub const ACCESS_POINT_ROUTE_TTL: u64 = 86_400_000;

/// Route lifetime on an [`InterfaceMode::Roaming`] interface: six hours, RNS's
/// `ROAMING_PATH_TIME` (`Transport.py` 128).
pub const ROAMING_ROUTE_TTL: u64 = 21_600_000;

/// How an interface's peers come and go, which sets how long its routes live.
///
/// RNS's interface modes (`Interfaces/Interface.py` 45-51). Only access-point and roaming
/// shorten route expiry (`Transport.py` 964-969); every other RNS mode expires routes as
/// [`Self::Full`] does, so they are not separate variants here yet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum InterfaceMode {
    /// Peers are stable: routes live for the configured route TTL.
    #[default]
    Full,
    /// Peers are clients that come and go: routes live at most a day.
    AccessPoint,
    /// This node moves between peers: routes live at most six hours.
    Roaming,
}

impl InterfaceMode {
    /// The lifetime of a route learned on an interface in this mode, given the configured
    /// full-mode lifetime. A mode never lengthens it, so a short configured TTL still applies.
    pub const fn route_ttl(self, full: u64) -> u64 {
        let cap = match self {
            Self::Full => return full,
            Self::AccessPoint => ACCESS_POINT_ROUTE_TTL,
            Self::Roaming => ROAMING_ROUTE_TTL,
        };
        if full < cap { full } else { cap }
    }
}

/// How many interfaces can carry a non-default [`InterfaceMode`] at once.
pub const INTERFACE_MODE_INTERFACES: usize = 4;

/// [`Node::set_interface_mode`] refused a new interface: every slot holds another one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfaceModeTableFull;

impl core::fmt::Display for InterfaceModeTableFull {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("interface mode table full")
    }
}
impl core::error::Error for InterfaceModeTableFull {}

/// Bounds for the receive-side announce freshness table.
///
/// Runtime state rather than a const parameter of [`Node`], so callers size it without a
/// second node type. Defaults follow the peer budget, with eight blobs per destination. A
/// destination's freshness lives exactly as long as its route, as RNS keeps announce blobs on
/// the path-table row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FreshnessPolicy {
    /// Maximum destination rows retained by the freshness table. Evicting a row drops its
    /// route, so this also bounds how many routes keep replay protection.
    pub max_destinations: usize,
    /// Maximum accepted full announce blobs retained per destination.
    pub max_blobs_per_destination: usize,
}

impl FreshnessPolicy {
    /// Evicting a row drops its route, so the table must never be the tighter bound: it
    /// covers every route and every known peer. A row left behind for an evicted route is
    /// harmless, because without a live route an announce is a first sighting.
    pub const fn for_node(peers: usize, routes: usize) -> Self {
        Self::for_peers(if peers > routes { peers } else { routes })
    }

    pub const fn for_peers(peers: usize) -> Self {
        Self {
            // PEERS == 0 is a deliberately non-learning node. Keep Node::new infallible with a
            // valid freshness table; the empty address book still refuses every announce.
            max_destinations: if peers == 0 { 1 } else { peers },
            max_blobs_per_destination: 8,
        }
    }
}

/// How long a carried link remains bridgeable after it last carries traffic.
///
/// Far longer than a link's keepalives, so a quiet but live link is kept while stale
/// transport state stays bounded.
pub const LINK_TRANSPORT_TIMEOUT: u64 = 3_600_000;

/// How long a validated carried link may go unheard before a new link request may take its
/// slot, in milliseconds.
///
/// RNS's `Transport.LINK_TIMEOUT`, 1.25 x the link stale time, 900 s. A live link's keepalives
/// refresh its bridge well inside this, so only a link that ended or went out of range yields;
/// without it, a full table of finished links would refuse every new one for
/// [`LINK_TRANSPORT_TIMEOUT`].
pub const LINK_TRANSPORT_IDLE: u64 = 900_000;

/// How long this node remembers a forwarded packet hash on a shared radio.
///
/// A single-radio transport hears its own relays, so a packet is remembered briefly to stop a
/// flood loop while still allowing a later retry. The filter starts a new generation at least
/// this often, so a hash is forgotten one to two of these after it was recorded.
pub const TRANSPORT_DEDUP_TIMEOUT: u64 = 60_000;

/// Transit packet hashes held per generation. The filter keeps two generations, as RNS keeps
/// its packet hash list and the one before it (`Transport.py` 832-834), so a burst forgets the
/// older half rather than everything.
pub const TRANSPORT_DEDUP_HASHES: usize = 32;

/// Path request tags held per generation, so a request heard twice is answered once
/// (`Transport.py` 1847-1856).
pub const PATH_REQUEST_TAGS: usize = 8;

/// The Reticulum transport hop ceiling, RNS's `PATHFINDER_M`.
pub const DEFAULT_TRANSPORT_MAX_HOPS: u8 = crate::packet::MAX_HOPS;

/// How long a carried packet's return path is kept for its delivery proof (RNS
/// `Transport.REVERSE_TIMEOUT`, eight minutes).
pub const REVERSE_TIMEOUT: u64 = 480_000;

/// What this node agrees to carry for other destinations.
///
/// Explicit, because many boards are endpoints, not routers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransportConfig {
    /// Re-broadcast verified announces with this node as their next transport hop.
    pub relay_announces: bool,
    /// Carry header-type-2 packets addressed to this node, and packets on remembered links.
    pub relay_packets: bool,
    /// A packet is relayed only while its forwarded hop count stays below this. See
    /// [`crate::packet::MAX_HOPS`] for how this meets RNS.
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
