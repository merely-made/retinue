//! Construction, configuration and counters.

use alloc::vec::Vec;

use heapless::Vec as BoundedVec;

#[cfg(doc)]
use super::Action;
use super::tables::HashGenerations;
use super::{
    AppDataTooLarge, DEFAULT_ANNOUNCE_INTERVAL, FreshnessPolicy, LINK_MTU, LogicalMtuError,
    MIN_LOGICAL_MTU, Node, PayloadLimits, TransportConfig, TransportCounters,
};
use crate::address_book::AddressBook;
use crate::announce_freshness::{AnnounceFreshness, AnnounceFreshnessConfig};
use crate::hash::{AddressHash, NameHash};
use crate::identity::PrivateIdentity;

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
            transit_filter: HashGenerations::new(),
            path_request_tags: HashGenerations::new(),
            reverse: BoundedVec::new(),
            sent_link_data: BoundedVec::new(),
            received_link_data: BoundedVec::new(),
            last_announce: None,
            announce_interval: DEFAULT_ANNOUNCE_INTERVAL,
            announced_blob: None,
            links: BoundedVec::new(),
            pending: BoundedVec::new(),
            first_hop_airtime: BoundedVec::new(),
            interface_modes: BoundedVec::new(),
            receivers: BoundedVec::new(),
            max_inbound_resource: crate::resource_transfer::DEFAULT_MAX_RESOURCE_SIZE,
            senders: BoundedVec::new(),
            resource_proofs: BoundedVec::new(),
            iv_counter: 0,
            refused_links: 0,
            expired_links: 0,
            expired_link_requests: 0,
            refused_peers: 0,
            refused_offers: 0,
            dropped_metadata: 0,
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

    /// Transport activity and bounded-state pressure since boot.
    pub fn transport_counters(&self) -> TransportCounters {
        self.transport_counters
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
    /// demand than slots, while expiries climbing is a node whose peers keep vanishing.
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

    /// Resources delivered without the metadata their sender attached. A Node delivers
    /// only a resource's data ([`Action::Resource`]), so a climbing count means a peer is
    /// sending metadata this application never sees.
    pub fn dropped_metadata(&self) -> u16 {
        self.dropped_metadata
    }
}
