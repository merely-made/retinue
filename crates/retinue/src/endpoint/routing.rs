//! Transit policy and the routing counters.

use alloc::vec::Vec;

use std::sync::atomic::{AtomicU64, Ordering};

use super::interface::InterfaceId;
use super::queue::{ClassCounters, QueueCounters, QueueDepths, QueueWeights};
use super::runtime::Endpoint;

/// Maximum hops an announce or packet may travel before a transport node drops it. RNS's
/// default `m` (`PATHFINDER_M`).
pub(super) const MAX_HOPS: u8 = 128;

/// Which interfaces a routing rule applies to.
///
/// Transit is directional: a node bridging a public radio to a private wire can carry the
/// radio's traffic outward while injecting nothing from the wire onto the air.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum InterfaceSelector {
    /// No interface. Nothing is accepted from, or sent to, any of them.
    #[default]
    None,
    /// Every attached interface, including ones attached later.
    All,
    /// Only the listed interfaces.
    Only(Vec<InterfaceId>),
}

impl InterfaceSelector {
    /// Whether this selector covers `iface`.
    pub fn allows(&self, iface: InterfaceId) -> bool {
        match self {
            Self::None => false,
            Self::All => true,
            Self::Only(list) => list.contains(&iface),
        }
    }
}

/// What this endpoint carries on behalf of others.
///
/// Every axis is independent: a node may relay announces but not data, accept transit from
/// one interface only, or cap its hops. The default ([`RoutingPolicy::none`]) carries nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoutingPolicy {
    /// Re-broadcast others' announces (hops+1, de-duplicated, never back the way they came),
    /// which is what makes destinations behind this node discoverable.
    pub forward_announces: bool,
    /// Forward others' data, link, and proof packets toward their destinations.
    pub forward_packets: bool,
    /// Interfaces this endpoint will accept transit *from*.
    pub allowed_ingress: InterfaceSelector,
    /// Interfaces this endpoint will emit transit *on*.
    pub allowed_egress: InterfaceSelector,
    /// Hop ceiling for forwarded traffic: a packet at or above it is dropped, not relayed.
    pub max_hops: u8,
    /// Each class's share of a contended interface, which bounds transit against local
    /// traffic.
    pub queue_weights: QueueWeights,
    /// How deep each class may queue on one interface before packets are dropped.
    pub queue_depths: QueueDepths,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        Self::none()
    }
}

impl RoutingPolicy {
    /// Carry nothing: the endpoint moves only its own traffic. The default.
    pub const fn none() -> Self {
        Self {
            forward_announces: false,
            forward_packets: false,
            allowed_ingress: InterfaceSelector::None,
            allowed_egress: InterfaceSelector::None,
            max_hops: 0,
            queue_weights: QueueWeights::DEFAULT,
            queue_depths: QueueDepths::DEFAULT,
        }
    }

    /// Carry everything, in both directions, out to the protocol's hop ceiling. This is what
    /// [`Endpoint::enable_routing`] installs.
    pub const fn transit() -> Self {
        Self {
            forward_announces: true,
            forward_packets: true,
            allowed_ingress: InterfaceSelector::All,
            allowed_egress: InterfaceSelector::All,
            max_hops: MAX_HOPS,
            queue_weights: QueueWeights::DEFAULT,
            queue_depths: QueueDepths::DEFAULT,
        }
    }

    /// Whether a packet arriving on `iface` may be forwarded at all under this policy.
    pub(super) fn accepts_transit_from(&self, iface: InterfaceId) -> bool {
        self.forward_packets && self.allowed_ingress.allows(iface)
    }

    /// Whether an announce arriving on `iface` may be re-broadcast under this policy.
    pub(super) fn relays_announce_from(&self, iface: InterfaceId) -> bool {
        self.forward_announces && self.allowed_ingress.allows(iface)
    }
}

/// A snapshot of what routing has done, for diagnostics and for proving a policy is enforced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RoutingCounters {
    /// Data, link, and proof packets forwarded on behalf of others.
    pub forwarded_packets: u64,
    /// Announces re-broadcast on behalf of others.
    pub forwarded_announces: u64,
    /// Packets a policy refused: transit disabled, or the ingress/egress interface not
    /// permitted.
    pub policy_rejected: u64,
    /// Packets dropped for reaching the policy's hop ceiling.
    pub hop_limit_dropped: u64,
    /// Packets dropped for carrying the IFAC flag in from an interface without IFAC (an IFAC
    /// interface strips the flag when it verifies a frame), as RNS drops them.
    pub ifac_flag_rejected: u64,
    /// Packets RNS's packet filter drops: header-type-2 packets for another transport, PLAIN
    /// or GROUP packets past their first hop, repeated transit or single packets, and tagless
    /// or repeated path requests.
    pub filtered_packets: u64,
    /// Announces whose identity a full address book turned away, so no `PeerAnnounce` was
    /// published for them. Climbing means the book is at capacity.
    pub refused_announces: u64,
    /// Verified unknown-route announces retained during an ingress interface burst.
    pub held_announces: u64,
    /// Verified announces dropped because the bounded ingress hold queue was full.
    pub held_announces_dropped: u64,
    /// Valid announces learned locally but not relayed because their destination was rate
    /// blocked on this incoming interface.
    pub relay_rate_limited_announces: u64,
    /// Routes dropped, the quietest first, to make room in a full path table.
    pub paths_evicted: u64,
    /// Announces rejected because their exact freshness blob is in this destination's live
    /// route history. Packet-loop de-duplication is separate and runs later.
    pub freshness_replays_rejected: u64,
    /// Announces rejected because their emission is no newer than this destination's live
    /// route.
    pub freshness_stale_rejected: u64,
    /// Freshness destination rows evicted, with their routes, to retain the configured
    /// bounded ledger.
    pub freshness_rows_evicted: u64,
    /// Per-destination freshness blobs evicted to retain the configured bounded history.
    pub freshness_blobs_evicted: u64,
    /// Our own link packets heard back from a relay, and dropped as ours.
    pub own_echo_dropped: u64,
    /// Far-end link packets heard again, directly or from a relay, and dropped as copies.
    /// A reliable initiator's IDENTIFY re-sends are new packets and do not land here.
    pub duplicate_dropped: u64,
    /// Link requests refused because a cap was full of active links, or the destination's
    /// accept backlog was full.
    pub inbound_links_refused: u64,
    /// Inbound links that never activated, closed to make room for a newer request.
    pub inbound_links_evicted: u64,
    /// Link packets dropped because the link's queue was full: its reader fell behind.
    pub link_queue_dropped: u64,
    /// Validly signed announces rejected because they name a known destination under a
    /// different public key.
    pub key_mismatch_announces: u64,
}

/// The live counter cells behind [`RoutingCounters`].
#[derive(Debug, Default)]
pub(super) struct RoutingStats {
    pub(super) forwarded_packets: AtomicU64,
    pub(super) forwarded_announces: AtomicU64,
    pub(super) policy_rejected: AtomicU64,
    pub(super) hop_limit_dropped: AtomicU64,
    pub(super) ifac_flag_rejected: AtomicU64,
    pub(super) filtered_packets: AtomicU64,
    pub(super) refused_announces: AtomicU64,
    pub(super) held_announces: AtomicU64,
    pub(super) held_announces_dropped: AtomicU64,
    pub(super) relay_rate_limited_announces: AtomicU64,
    pub(super) paths_evicted: AtomicU64,
    pub(super) freshness_replays_rejected: AtomicU64,
    pub(super) freshness_stale_rejected: AtomicU64,
    pub(super) freshness_rows_evicted: AtomicU64,
    pub(super) freshness_blobs_evicted: AtomicU64,
    pub(super) own_echo_dropped: AtomicU64,
    pub(super) duplicate_dropped: AtomicU64,
    pub(super) inbound_links_refused: AtomicU64,
    pub(super) inbound_links_evicted: AtomicU64,
    pub(super) link_queue_dropped: AtomicU64,
    pub(super) key_mismatch_announces: AtomicU64,
}

impl RoutingStats {
    fn snapshot(&self) -> RoutingCounters {
        RoutingCounters {
            forwarded_packets: self.forwarded_packets.load(Ordering::Relaxed),
            forwarded_announces: self.forwarded_announces.load(Ordering::Relaxed),
            policy_rejected: self.policy_rejected.load(Ordering::Relaxed),
            hop_limit_dropped: self.hop_limit_dropped.load(Ordering::Relaxed),
            ifac_flag_rejected: self.ifac_flag_rejected.load(Ordering::Relaxed),
            filtered_packets: self.filtered_packets.load(Ordering::Relaxed),
            refused_announces: self.refused_announces.load(Ordering::Relaxed),
            held_announces: self.held_announces.load(Ordering::Relaxed),
            held_announces_dropped: self.held_announces_dropped.load(Ordering::Relaxed),
            relay_rate_limited_announces: self.relay_rate_limited_announces.load(Ordering::Relaxed),
            paths_evicted: self.paths_evicted.load(Ordering::Relaxed),
            freshness_replays_rejected: self.freshness_replays_rejected.load(Ordering::Relaxed),
            freshness_stale_rejected: self.freshness_stale_rejected.load(Ordering::Relaxed),
            freshness_rows_evicted: self.freshness_rows_evicted.load(Ordering::Relaxed),
            freshness_blobs_evicted: self.freshness_blobs_evicted.load(Ordering::Relaxed),
            own_echo_dropped: self.own_echo_dropped.load(Ordering::Relaxed),
            duplicate_dropped: self.duplicate_dropped.load(Ordering::Relaxed),
            inbound_links_refused: self.inbound_links_refused.load(Ordering::Relaxed),
            inbound_links_evicted: self.inbound_links_evicted.load(Ordering::Relaxed),
            link_queue_dropped: self.link_queue_dropped.load(Ordering::Relaxed),
            key_mismatch_announces: self.key_mismatch_announces.load(Ordering::Relaxed),
        }
    }
}

impl Endpoint {
    /// Act as a transport node, relaying announces and forwarding packets: shorthand for
    /// [`RoutingPolicy::transit`]. Use [`set_routing_policy`](Self::set_routing_policy) for
    /// anything narrower.
    pub fn enable_routing(&self) {
        self.set_routing_policy(RoutingPolicy::transit());
    }

    /// Install the transit policy: what this endpoint carries for others, from and to which
    /// interfaces, and how far. Takes effect for packets routed after it returns.
    ///
    /// Transit is independent of local service: carrying nothing does not affect this
    /// endpoint's own links, announces, or destinations.
    pub fn set_routing_policy(&self, policy: RoutingPolicy) {
        let (weights, depths) = (policy.queue_weights, policy.queue_depths);
        *self.shared.routing.lock().unwrap() = policy;
        // Interfaces already attached take the new queue policy too.
        for i in self.shared.interfaces.lock().unwrap().iter() {
            i.outbound.set_policy(weights, depths);
        }
    }

    /// What the outbound schedule has done across every interface: released and dropped, by
    /// class.
    pub fn queue_counters(&self) -> QueueCounters {
        let mut out = QueueCounters::default();
        for i in self.shared.interfaces.lock().unwrap().iter() {
            let (sent, dropped) = i.outbound.counters();
            out.sent.add(ClassCounters::from_array(sent));
            out.dropped.add(ClassCounters::from_array(dropped));
        }
        out
    }

    /// Packets currently queued or in flight across attached interfaces.
    ///
    /// A point-in-time observation, not a delivery receipt.
    pub fn outbound_queue_depth(&self) -> usize {
        self.shared
            .interfaces
            .lock()
            .unwrap()
            .iter()
            .map(|interface| interface.outbound.depth())
            .sum()
    }

    /// The transit policy currently installed.
    pub fn routing_policy(&self) -> RoutingPolicy {
        self.shared.routing.lock().unwrap().clone()
    }

    /// What routing has done since this endpoint started: forwarded, refused, and dropped.
    pub fn routing_counters(&self) -> RoutingCounters {
        self.shared.routing_stats.snapshot()
    }
}
