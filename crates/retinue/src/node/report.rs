//! Counters, pause assessment and loss reports a shell reads back.

use heapless::Vec as BoundedVec;

use super::InterfaceId;
#[cfg(doc)]
use super::{Action, LINK_TRANSPORT_IDLE, Node, link_request_timeout};
use crate::hash::AddressHash;
use crate::packet::Packet;

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
    /// Carried-link entries evicted to admit a newer transport link: the stalest link still
    /// awaiting its proof, or else a validated one unheard for [`LINK_TRANSPORT_IDLE`].
    pub evicted_bridges: u16,
    /// Link requests not carried because every bridge slot held a recently heard validated
    /// link.
    pub refused_bridges: u16,
    /// Link requests dropped because their MTU had to be lowered under a link mode this node
    /// cannot encode.
    pub undecodable_link_requests: u16,
    /// Packets dropped on a carried link that has not validated: link traffic ahead of the
    /// proof, and proofs that arrived on the wrong side or failed the destination's signature.
    pub unvalidated_link_packets: u16,
    /// Transit dropped at the configured hop ceiling.
    pub hop_limit_dropped: u16,
    /// Transit that named this node but had no fresh route onward.
    pub unroutable_packets: u16,
    /// Packets RNS's packet filter drops before any dispatch: header-type-2 packets for
    /// another transport, PLAIN or GROUP packets past their first hop, and tagless or
    /// repeated path requests.
    pub filtered_packets: u16,
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
