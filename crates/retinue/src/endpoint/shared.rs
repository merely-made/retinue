//! `Shared`, the state the router, the link drivers and the handles share.

use alloc::vec::Vec;

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use tokio::sync::{mpsc, oneshot};

use crate::address_book::AddressBook;
use crate::announce::{TimebaseGenerator, VerifiedAnnounces};
use crate::announce_admission::AnnounceAdmission;
use crate::hash::AddressHash;
use crate::identity::PrivateIdentity;
use crate::link::{self, Link};
use crate::link_liveness::Liveness;
use crate::packet::Packet;
use crate::resource::Advertisement;
use crate::resource_transfer::PROOF_CACHE_ANSWERS;

use super::announces::{AnnounceFreshnessState, HeldAnnounce};
use super::dedup::{HashList, LinkPacketMemory, VERIFIED_ANNOUNCES};
use super::facts::{LinkDirection, LinkFactKind, LinkRemoteFact, PeerAnnounce};
use super::inbound::{Accepted, AcceptedResource, InboundLinks};
use super::interface::{Iface, InterfaceId, QueueAdmission};
use super::known_destinations::BookPersistence;
use super::paths::PathEntry;
use super::queue::TrafficClass;
use super::registration::{RatchetPersistence, Registered};
use super::resource_session::RESOURCE_PROOF_CACHE_TTL;
use super::routing::{RoutingPolicy, RoutingStats};
use super::single::{PendingReceipt, ReceivedSingle};
use super::stream::StreamFault;
use super::transit::{LinkBridge, ReverseEntry};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Lifecycle {
    Running,
    Quiescing,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Quiesce {
    Started,
    InProgress,
    Closed,
}

pub(super) struct LinkEntry {
    pub(super) link: Link,
    /// How inbound traffic for this link is handled: best-effort delivers decrypted bytes
    /// straight to the stream; reliable hands raw channel and proof packets to a driver.
    pub(super) kind: LinkKind,
    /// The interface this link's traffic goes out on.
    pub(super) iface: InterfaceId,
    pub(super) direction: LinkDirection,
    pub(super) remote: LinkRemoteFact,
    /// Keepalive, staleness and handshake timers, advanced by the link watchdog.
    pub(super) liveness: Liveness,
    /// Set when the watchdog drops the link, so its stream reports a timeout, not an end.
    pub(super) lost: Arc<AtomicBool>,
}

/// The delivery discipline of a link's stream, chosen when the stream is registered.
pub(super) enum LinkKind {
    /// The router decrypts each data packet and forwards the plaintext (right for TCP).
    /// `fault` is the stream's receive error, set if its queue overflows.
    BestEffort {
        inbound: mpsc::Sender<Vec<u8>>,
        fault: Arc<Mutex<Option<StreamFault>>>,
    },
    /// The router forwards raw channel-data and proof packets to the reliable driver task,
    /// which orders them, proves receipts, and drives retransmission (for lossy media).
    Reliable { packets: mpsc::Sender<Packet> },
    /// Raw resource control, part, and proof packets are handed to a resource session.
    Resource { packets: mpsc::Sender<Packet> },
}

impl LinkKind {
    pub(super) fn fact_kind(&self) -> LinkFactKind {
        match self {
            Self::BestEffort { .. } => LinkFactKind::BestEffort,
            Self::Reliable { .. } => LinkFactKind::Reliable,
            Self::Resource { .. } => LinkFactKind::Resource,
        }
    }
}

type Links = Arc<Mutex<HashMap<AddressHash, LinkEntry>>>;

/// Shared router state.
pub(super) struct Shared {
    pub(super) lifecycle: Mutex<Lifecycle>,
    pub(super) closed_notify: tokio::sync::Notify,
    pub(super) identity: PrivateIdentity,
    pub(super) address_book: Mutex<AddressBook>,
    /// The host's address-book persistence hook.
    pub(super) book_persistence: Mutex<Option<BookPersistence>>,
    pub(super) links: Links,
    pub(super) registered: Mutex<Vec<Registered>>,
    /// The host's ratchet persistence hook. Its lock also serializes ratchet rotation, so
    /// two announces cannot rotate one store twice or persist out of order.
    pub(super) ratchet_persistence: Mutex<Option<RatchetPersistence>>,
    /// Per-destination announce ordinals: receivers retain each destination's signed blob,
    /// so each needs its own strictly increasing timebase.
    pub(super) announce_timebases: Mutex<HashMap<AddressHash, TimebaseGenerator>>,
    /// Every attached interface. Announces broadcast to all; link traffic targets one.
    pub(super) interfaces: Mutex<Vec<Iface>>,
    /// The router's inbound channel: every interface's reader feeds `(interface, packet)`.
    pub(super) router_tx: mpsc::Sender<(InterfaceId, Packet)>,
    /// Inbound accepted links (stream + destination), surfaced to `accept`.
    pub(super) accepted_tx: mpsc::UnboundedSender<Accepted>,
    /// Inbound accepted reliable links, surfaced to `accept_reliable_on_any`.
    pub(super) reliable_accepted_tx: mpsc::UnboundedSender<Accepted>,
    /// Inbound resource links, surfaced to `accept_resource`.
    pub(super) resource_accepted_tx: mpsc::UnboundedSender<AcceptedResource>,
    /// Validated announces, surfaced to `announcements`.
    pub(super) announce_tx: mpsc::UnboundedSender<PeerAnnounce>,
    /// Monotonic order assigned only after an announce passes validation and admission.
    pub(super) announce_sequence: AtomicU64,
    /// Decrypted link-less single packets, surfaced to `accept_single`.
    pub(super) single_tx: mpsc::UnboundedSender<ReceivedSingle>,
    /// Pending outbound links awaiting a proof, keyed by destination: the waiter to wake
    /// (with the interface the proof came in on), and the half-open link that verifies it.
    pub(super) pending: Mutex<HashMap<AddressHash, oneshot::Sender<(Link, InterfaceId)>>>,
    pub(super) pending_links: Mutex<HashMap<AddressHash, link::PendingLink>>,
    pub(super) next_iface_id: AtomicU32,
    /// Whether this endpoint acts as a transport node (forwards announces and packets).
    pub(super) routing: Mutex<RoutingPolicy>,
    /// What routing has actually done, for diagnostics and policy proof.
    pub(super) routing_stats: RoutingStats,
    /// Revision of topology and observation facts consumed by host management projections.
    pub(super) diagnostic_generation: AtomicU64,
    /// Serializes diagnostic fact mutation against a multi-table diagnostic capture.
    pub(super) diagnostic_barrier: RwLock<()>,
    /// Upper bound, in milliseconds, of the random delay before relaying an announce. See
    /// [`Endpoint::set_relay_jitter`].
    pub(super) relay_jitter_ms: AtomicU64,
    /// First reliable-channel RTT estimate. Proofs adapt it after traffic starts.
    pub(super) reliable_initial_rtt_ms: AtomicU64,
    /// Maximum unproved reliable frames allowed in flight on subsequently opened links.
    pub(super) reliable_max_window: AtomicU32,
    /// Decoded output ceiling for subsequently opened reliable streams.
    pub(super) reliable_decoded_frame_limit: AtomicUsize,
    /// Link-request retry interval for subsequently opened links.
    pub(super) link_setup_retry_ms: AtomicU64,
    /// Per-interface first-hop airtime allowances, in milliseconds, for link setup deadlines.
    pub(super) first_hop_airtime_ms: Mutex<HashMap<InterfaceId, u64>>,
    /// MTU requested and offered by subsequently established links.
    pub(super) link_mtu: AtomicU32,
    /// Proofs for recently accepted link requests, keyed by link id, replayed when only the
    /// proof was lost rather than creating a second stream.
    pub(super) inbound_link_proofs: Mutex<HashMap<AddressHash, (Packet, Instant)>>,
    /// Live inbound links counted against their caps, and the accept backlog.
    pub(super) inbound: Mutex<InboundLinks>,
    /// The last resource proof sent on each link, when, and how often it was re-sent: it
    /// answers a cache request for [`RESOURCE_PROOF_CACHE_TTL`], at most
    /// [`PROOF_CACHE_ANSWERS`] times.
    pub(super) resource_proofs: Mutex<HashMap<AddressHash, (Packet, Instant, u8)>>,
    /// Learned routes, from announces.
    pub(super) path_table: Mutex<HashMap<AddressHash, PathEntry>>,
    /// The last [`SEEN_ANNOUNCES`] announce packet hashes, for de-duplication.
    pub(super) seen_announces: Mutex<(HashSet<AddressHash>, VecDeque<AddressHash>)>,
    /// Announces that verified recently, so a copy skips its signature check.
    pub(super) verified_announces: Mutex<VerifiedAnnounces<VERIFIED_ANNOUNCES>>,
    /// Our own link packets heard back, and the far end's heard twice. See [`LinkPacketMemory`].
    pub(super) link_packets: Mutex<LinkPacketMemory>,
    /// Transit and single packets already seen, so a loop or a second copy is dropped.
    pub(super) packet_filter: Mutex<HashList>,
    /// Path requests already seen, by target and tag, so each is answered once.
    pub(super) path_request_tags: Mutex<HashList>,
    /// Bounded freshness admission. Its lock spans the complete announce-effect bundle.
    pub(super) announce_freshness: Mutex<AnnounceFreshnessState>,
    /// The freshness policy's route TTL, readable without the freshness lock.
    pub(super) route_ttl_ms: AtomicU64,
    /// Announce admission, on a clock relative to this endpoint so verdicts are deterministic
    /// under a supplied time.
    pub(super) announce_admission: Mutex<AnnounceAdmission>,
    pub(super) announce_admission_started: tokio::time::Instant,
    /// Verified unknown-route announces held until their ingress burst has subsided.
    pub(super) held_announces: Mutex<VecDeque<HeldAnnounce>>,
    /// At most one release task runs for each interface, however many announces it is holding.
    pub(super) held_release_tasks: Mutex<HashSet<InterfaceId>>,
    /// Wakes release tasks when a carrier is detached or the policy changes.
    pub(super) held_release_wake: tokio::sync::Notify,
    /// Last time a path request went out per destination: see [`PATH_REQUEST_MIN_INTERVAL`].
    pub(super) path_request_budget: Mutex<HashMap<AddressHash, Instant>>,
    /// When the path requests in the current window went out, oldest first, for the global
    /// cap ([`PATH_REQUEST_GLOBAL_MAX`]). Never longer than the cap.
    pub(super) path_request_stamps: Mutex<VecDeque<Instant>>,
    /// Links carried through this node, by link id.
    pub(super) link_transport: Mutex<HashMap<AddressHash, LinkBridge>>,
    /// Return paths for the proofs of other packets we carried, keyed by truncated packet
    /// hash. Bounded, expired after [`REVERSE_TIMEOUT`], and consumed by the proof.
    pub(super) reverse_table: Mutex<HashMap<AddressHash, ReverseEntry>>,
    /// Our sent single packets awaiting proof, keyed by the truncated packet hash a proof is
    /// addressed to. Bounded by [`SINGLE_RECEIPTS`].
    pub(super) single_receipts: Mutex<HashMap<AddressHash, PendingReceipt>>,
    /// Whether our proofs carry the signature alone (RNS's default) or the hash too.
    pub(super) implicit_proofs: AtomicBool,
    /// Abort handles for every task the endpoint spawned. Aborting them on close is what
    /// breaks the router<->`Shared` reference cycle and releases every socket.
    pub(super) tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    /// Tasks that finish on their own once their input goes away (best-effort relays and
    /// reliable drivers), which [`Endpoint::shutdown`] awaits before stopping anything.
    pub(super) drainable: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Caller-driven resource sessions must finish or be dropped before their close packet
    /// can be included in an orderly shutdown.
    pub(super) active_resources: AtomicUsize,
    pub(super) resource_notify: tokio::sync::Notify,
}

impl Shared {
    /// Mutate diagnostic source state under the one required lock order: revision barrier,
    /// then the owned fact lock(s). The revision advances before the barrier is released.
    pub(super) fn write_diagnostic<T>(&self, change: impl FnOnce() -> (T, bool)) -> T {
        let _barrier = self.diagnostic_barrier.write().unwrap();
        let (value, changed) = change();
        if changed {
            self.diagnostic_generation.fetch_add(1, Ordering::AcqRel);
        }
        value
    }

    /// Capture a value against one stable diagnostic revision. The repeated revision read is
    /// defensive: writers advance it before releasing the barrier.
    pub(super) fn capture_diagnostic<T>(&self, mut capture: impl FnMut() -> T) -> (u64, T) {
        loop {
            let _barrier = self.diagnostic_barrier.read().unwrap();
            let before = self.diagnostic_generation.load(Ordering::Acquire);
            let value = capture();
            let after = self.diagnostic_generation.load(Ordering::Acquire);
            if before == after {
                return (after, value);
            }
        }
    }

    pub(super) fn remove_link(&self, id: AddressHash) {
        self.write_diagnostic(|| {
            let removed = self.links.lock().unwrap().remove(&id).is_some();
            ((), removed)
        });
        self.inbound.lock().unwrap().slots.remove(&id);
        self.resource_proofs.lock().unwrap().remove(&id);
    }

    /// Hand a raw link packet to its driver, counting it if the driver's queue is full.
    pub(super) fn queue_link_packet(&self, packets: &mpsc::Sender<Packet>, pkt: Packet) {
        if let Err(mpsc::error::TrySendError::Full(_)) = packets.try_send(pkt) {
            self.routing_stats
                .link_queue_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Keep the resource proof just sent on `link`, replacing any earlier one.
    pub(super) fn keep_resource_proof(&self, link: AddressHash, proof: Packet) {
        let now = Instant::now();
        let mut proofs = self.resource_proofs.lock().unwrap();
        proofs.retain(|_, (_, kept, _)| now.duration_since(*kept) < RESOURCE_PROOF_CACHE_TTL);
        proofs.insert(link, (proof, now, 0));
    }

    /// The resource proof kept for `link`, to re-send, if `matches` it, it has not expired,
    /// and it has been re-sent fewer than [`PROOF_CACHE_ANSWERS`] times. The cap bounds what
    /// anyone who heard the cleartext proof can make this endpoint transmit.
    pub(super) fn resend_resource_proof(
        &self,
        link: AddressHash,
        matches: impl FnOnce(&Packet) -> bool,
    ) -> Option<Packet> {
        let mut proofs = self.resource_proofs.lock().unwrap();
        let (proof, kept, answers) = proofs.get_mut(&link)?;
        if kept.elapsed() >= RESOURCE_PROOF_CACHE_TTL
            || *answers >= PROOF_CACHE_ANSWERS
            || !matches(proof)
        {
            return None;
        }
        *answers += 1;
        Some(proof.clone())
    }

    /// The kept proof to re-send for a resource advertisement on `link` naming the
    /// resource it proves: an older retinue sender that lost the proof offers again
    /// rather than asking with a cache request.
    pub(super) fn proof_for_advertisement(
        &self,
        link: AddressHash,
        advertisement: &Packet,
    ) -> Option<Packet> {
        if !self.resource_proofs.lock().unwrap().contains_key(&link) {
            return None;
        }
        let entry = self
            .links
            .lock()
            .unwrap()
            .get(&link)
            .map(|e| e.link.clone())?;
        let plain = entry.decrypt(advertisement).ok()?;
        let advertised = Advertisement::parse(&plain).ok()?;
        self.resend_resource_proof(link, |proof| {
            crate::resource::parse_proof(&proof.payload)
                .is_some_and(|(hash, _)| hash[..] == advertised.resource_hash[..])
        })
    }

    pub(super) fn is_running(&self) -> bool {
        *self.lifecycle.lock().unwrap() == Lifecycle::Running
    }

    pub(super) fn is_closed(&self) -> bool {
        *self.lifecycle.lock().unwrap() == Lifecycle::Closed
    }

    pub(super) fn begin_quiesce(&self) -> Quiesce {
        let mut state = self.lifecycle.lock().unwrap();
        match *state {
            Lifecycle::Running => {
                *state = Lifecycle::Quiescing;
                Quiesce::Started
            }
            Lifecycle::Quiescing => Quiesce::InProgress,
            Lifecycle::Closed => Quiesce::Closed,
        }
    }

    pub(super) fn mark_closed(&self) -> bool {
        let mut state = self.lifecycle.lock().unwrap();
        if *state == Lifecycle::Closed {
            false
        } else {
            *state = Lifecycle::Closed;
            true
        }
    }

    pub(super) fn begin_resource(&self) -> bool {
        let state = self.lifecycle.lock().unwrap();
        if *state != Lifecycle::Running {
            return false;
        }
        self.active_resources.fetch_add(1, Ordering::AcqRel);
        true
    }

    pub(super) fn end_resource(&self) {
        let previous = self.active_resources.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        self.resource_notify.notify_waiters();
    }

    /// Send our own upkeep (announces, path requests) out every interface, as control.
    pub(super) fn broadcast(&self, pkt: Packet) {
        for i in self.interfaces.lock().unwrap().iter() {
            let _ = i.push(pkt.clone(), TrafficClass::Control);
        }
    }

    /// Send a packet out one interface, addressed through that interface's transport node if
    /// it has one (header-type-2 `[transport][dest]`), so a transport node forwards it.
    pub(super) fn send_on(&self, iface: InterfaceId, pkt: Packet) {
        self.send_on_class(iface, pkt, TrafficClass::Interactive);
    }

    /// Send a packet out one interface in a chosen class. Local link traffic defaults to
    /// interactive; setup, proofs, and keepalives are control; carried traffic is transit.
    pub(super) fn send_on_class(&self, iface: InterfaceId, pkt: Packet, class: TrafficClass) {
        let _ = self.try_send_on_class(iface, pkt, class);
    }

    pub(super) fn try_send_on_class(
        &self,
        iface: InterfaceId,
        pkt: Packet,
        class: TrafficClass,
    ) -> bool {
        // Copies of carried traffic coming back are its owner's to judge.
        if class != TrafficClass::Transit {
            self.link_packets.lock().unwrap().note_sent(&pkt);
        }
        let addressed = self.address_for(iface, pkt);
        if let Some(i) = self
            .interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.id == iface)
        {
            return matches!(i.push(addressed, class), QueueAdmission::Queued);
        }
        false
    }

    /// Address a packet through its destination's transport node, if its route has one
    /// (header-type-2), so the node forwards it.
    pub(super) fn address_for(&self, iface: InterfaceId, mut pkt: Packet) -> Packet {
        // By destination, not interface: one radio reaches A via X and B via Y.
        let via = self
            .path_table
            .lock()
            .unwrap()
            .get(&pkt.destination)
            .and_then(|entry| entry.transport);
        if via.is_some() {
            self.touch_path(pkt.destination);
        }
        pkt.address_via(via);
        let _ = iface;
        pkt
    }
}
