//! `Endpoint` construction, task tracking, and shutdown.

use alloc::vec::Vec;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::address_book::AddressBook;
use crate::announce::VerifiedAnnounces;
use crate::announce_admission::{AnnounceAdmission, AnnounceIngressPolicy};
use crate::announce_freshness::AnnounceFreshnessConfigError;
use crate::channel::DEFAULT_DECODED_FRAME_LIMIT;
use crate::hash::AddressHash;
use crate::identity::{Identity, PrivateIdentity};
use crate::packet::Packet;

use super::announces::{AnnounceFreshnessPolicy, AnnounceFreshnessState};
use super::config::{
    DEFAULT_LINK_MTU, DEFAULT_LINK_SETUP_RETRY_MS, DEFAULT_RELIABLE_INITIAL_RTT_MS,
    DEFAULT_RELIABLE_MAX_WINDOW,
};
use super::dedup::{HashList, LinkPacketMemory, PACKET_HASHES, PATH_REQUEST_TAGS};
use super::facts::PeerAnnounce;
use super::inbound::{Accepted, AcceptedResource, InboundLinks};
use super::interface::InterfaceId;
use super::known_destinations::KNOWN_DESTINATIONS_INTERVAL;
use super::rebroadcast::{Rebroadcasting, start_rebroadcast_driver};
use super::router::route;
use super::routing::{RoutingPolicy, RoutingStats};
use super::shared::{Lifecycle, Quiesce, Shared};
use super::single::ReceivedSingle;
use super::watchdog::{LINK_WATCHDOG_TICK, watch_links};

/// Depth of the router's inbound queue. When it is full a TCP reader awaits, back-pressuring
/// the peer, and [`InterfaceSink::deliver`], which cannot await, drops.
const ROUTER_QUEUE: usize = 1024;

/// A Reticulum endpoint over any number of interfaces.
///
/// All methods take `&self` (the receivers are behind async mutexes), so an endpoint can be
/// wrapped in an `Arc` and shared: a host transport can call `open`/`announce` from one task
/// while another drives `accept`/`next_announcement`.
pub struct Endpoint {
    pub(super) shared: Arc<Shared>,
    pub(super) accepted_rx: AsyncMutex<mpsc::UnboundedReceiver<Accepted>>,
    pub(super) reliable_accepted_rx: AsyncMutex<mpsc::UnboundedReceiver<Accepted>>,
    pub(super) resource_accepted_rx: AsyncMutex<mpsc::UnboundedReceiver<AcceptedResource>>,
    pub(super) announce_rx: AsyncMutex<mpsc::UnboundedReceiver<PeerAnnounce>>,
    pub(super) single_rx: AsyncMutex<mpsc::UnboundedReceiver<ReceivedSingle>>,
}

pub(super) fn endpoint_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "endpoint closed")
}

pub(super) async fn recv_until_closed<T>(
    shared: &Arc<Shared>,
    receiver: &AsyncMutex<mpsc::UnboundedReceiver<T>>,
) -> io::Result<T> {
    let closed = shared.closed_notify.notified();
    if shared.is_closed() {
        return Err(endpoint_closed());
    }
    tokio::select! {
        value = async {
            receiver.lock().await.recv().await
        } => {
            if shared.is_closed() {
                Err(endpoint_closed())
            } else {
                value.ok_or_else(endpoint_closed)
            }
        },
        _ = closed => Err(endpoint_closed()),
    }
}

impl Endpoint {
    /// Create an endpoint with no interfaces yet, and start its router.
    pub fn new(identity: PrivateIdentity) -> Self {
        Self::with_announce_freshness_policy(identity, AnnounceFreshnessPolicy::default())
            .expect("default announce freshness policy is valid")
    }

    /// Create an endpoint with an explicit bounded receive-freshness policy.
    ///
    /// Invalid zero capacities are refused before any router task is started.
    pub fn with_announce_freshness_policy(
        identity: PrivateIdentity,
        freshness_policy: AnnounceFreshnessPolicy,
    ) -> Result<Self, AnnounceFreshnessConfigError> {
        let (router_tx, mut router_rx) = mpsc::channel::<(InterfaceId, Packet)>(ROUTER_QUEUE);
        let (accepted_tx, accepted_rx) = mpsc::unbounded_channel::<Accepted>();
        let (reliable_accepted_tx, reliable_accepted_rx) = mpsc::unbounded_channel::<Accepted>();
        let (resource_accepted_tx, resource_accepted_rx) =
            mpsc::unbounded_channel::<AcceptedResource>();
        let (announce_tx, announce_rx) = mpsc::unbounded_channel::<PeerAnnounce>();
        let (single_tx, single_rx) = mpsc::unbounded_channel::<ReceivedSingle>();

        let shared = Arc::new(Shared {
            lifecycle: Mutex::new(Lifecycle::Running),
            closed_notify: tokio::sync::Notify::new(),
            identity,
            address_book: Mutex::new(AddressBook::new()),
            book_persistence: Mutex::new(None),
            links: Arc::new(Mutex::new(HashMap::new())),
            registered: Mutex::new(Vec::new()),
            ratchet_persistence: Mutex::new(None),
            announce_timebases: Mutex::new(HashMap::new()),
            interfaces: Mutex::new(Vec::new()),
            router_tx,
            accepted_tx,
            reliable_accepted_tx,
            resource_accepted_tx,
            announce_tx,
            announce_sequence: AtomicU64::new(0),
            single_tx,
            pending: Mutex::new(HashMap::new()),
            pending_links: Mutex::new(HashMap::new()),
            next_iface_id: AtomicU32::new(0),
            routing: Mutex::new(RoutingPolicy::none()),
            routing_stats: RoutingStats::default(),
            diagnostic_generation: AtomicU64::new(0),
            diagnostic_barrier: RwLock::new(()),
            relay_jitter_ms: AtomicU64::new(crate::node::REBROADCAST_WINDOW),
            rebroadcasts: Mutex::new(Rebroadcasting::new()),
            rebroadcast_wake: tokio::sync::Notify::new(),
            reliable_initial_rtt_ms: AtomicU64::new(DEFAULT_RELIABLE_INITIAL_RTT_MS),
            reliable_max_window: AtomicU32::new(DEFAULT_RELIABLE_MAX_WINDOW),
            reliable_decoded_frame_limit: AtomicUsize::new(DEFAULT_DECODED_FRAME_LIMIT),
            link_setup_retry_ms: AtomicU64::new(DEFAULT_LINK_SETUP_RETRY_MS),
            first_hop_airtime_ms: Mutex::new(HashMap::new()),
            iface_policies: Mutex::new(HashMap::new()),
            link_mtu: AtomicU32::new(DEFAULT_LINK_MTU),
            inbound_link_proofs: Mutex::new(HashMap::new()),
            inbound: Mutex::new(InboundLinks::default()),
            resource_proofs: Mutex::new(HashMap::new()),
            path_table: Mutex::new(HashMap::new()),
            seen_announces: Mutex::new((HashSet::new(), VecDeque::new())),
            verified_announces: Mutex::new(VerifiedAnnounces::new()),
            link_packets: Mutex::new(LinkPacketMemory::new()),
            packet_filter: Mutex::new(HashList::new(PACKET_HASHES)),
            path_request_tags: Mutex::new(HashList::new(PATH_REQUEST_TAGS)),
            announce_freshness: Mutex::new(AnnounceFreshnessState::new(freshness_policy)?),
            route_ttl_ms: AtomicU64::new(freshness_policy.route_ttl_ticks()),
            announce_admission: Mutex::new(
                AnnounceAdmission::new(AnnounceIngressPolicy::default()),
            ),
            announce_admission_started: tokio::time::Instant::now(),
            held_announces: Mutex::default(),
            held_release_tasks: Mutex::new(HashSet::new()),
            held_release_wake: tokio::sync::Notify::new(),
            path_request_budget: Mutex::new(HashMap::new()),
            path_request_stamps: Mutex::new(VecDeque::new()),
            link_transport: Mutex::new(HashMap::new()),
            reverse_table: Mutex::new(HashMap::new()),
            single_receipts: Mutex::new(HashMap::new()),
            implicit_proofs: AtomicBool::new(true),
            tasks: Mutex::new(Vec::new()),
            drainable: Mutex::new(Vec::new()),
            active_resources: AtomicUsize::new(0),
            resource_notify: tokio::sync::Notify::new(),
        });

        let router = Arc::clone(&shared);
        track(&shared, async move {
            while let Some((iface, pkt)) = router_rx.recv().await {
                // Every ingress path (TCP, `deliver`, `deliver_frame`) funnels through here,
                // so this is the one place a flagged frame from a plain interface is refused.
                if pkt.ifac {
                    router
                        .routing_stats
                        .ifac_flag_rejected
                        .fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                route(&router, iface, pkt);
            }
        });
        start_rebroadcast_driver(&shared);
        let watchdog = Arc::clone(&shared);
        track(&shared, async move {
            let mut tick = tokio::time::interval(LINK_WATCHDOG_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                watch_links(&watchdog);
            }
        });
        let cleaner = Arc::clone(&shared);
        track(&shared, async move {
            let mut tick = tokio::time::interval(KNOWN_DESTINATIONS_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            tick.tick().await;
            loop {
                tick.tick().await;
                // A failed write is retried at the next interval, as RNS's next clean does.
                let _ = cleaner.clean_and_persist_book();
            }
        });

        Ok(Self {
            shared,
            accepted_rx: AsyncMutex::new(accepted_rx),
            reliable_accepted_rx: AsyncMutex::new(reliable_accepted_rx),
            resource_accepted_rx: AsyncMutex::new(resource_accepted_rx),
            announce_rx: AsyncMutex::new(announce_rx),
            single_rx: AsyncMutex::new(single_rx),
        })
    }

    /// This endpoint's public identity.
    pub fn identity(&self) -> &Identity {
        self.shared.identity.public()
    }

    /// The address book, for resolving learned peers. A resolve counts as a use, as RNS's
    /// `Identity.recall` does, so the peer is kept through cleaning.
    pub fn resolve(&self, dest: AddressHash) -> Option<Identity> {
        self.shared.mark_destination_used(dest);
        self.shared
            .address_book
            .lock()
            .unwrap()
            .resolve(dest)
            .map(|p| p.identity)
    }

    /// Stop the endpoint at once: abort every task it spawned, closing their sockets, so
    /// streams handed out see their connection end. [`Drop`](Self::drop) calls this too.
    /// Idempotent.
    pub fn close(&self) {
        if !self.shared.mark_closed() {
            return;
        }
        for handle in self.shared.tasks.lock().unwrap().drain(..) {
            handle.abort();
        }
        // Release every link's receiver even while the Endpoint itself lives on.
        self.shared.write_diagnostic(|| {
            let mut links = self.shared.links.lock().unwrap();
            let had_links = !links.is_empty();
            links.clear();
            ((), had_links)
        });
        {
            let mut inbound = self.shared.inbound.lock().unwrap();
            inbound.slots.clear();
            inbound.backlog.clear();
        }
        // Wake any caller-driven pump parked in `next_outbound`, to see the end.
        for i in self.shared.interfaces.lock().unwrap().iter() {
            i.outbound.close();
        }
        self.shared.closed_notify.notify_waiters();
    }

    /// Stop the endpoint, giving work already queued a bounded chance to reach the wire.
    ///
    /// [`close`](Self::close) aborts the interface writers with everything else, and a
    /// stream's `flush` returns long before its bytes are written. Finish or drop streams and
    /// resource sessions first, then await this: it waits for relays, reliable proofs,
    /// resource sessions, and queued and in-flight packets, for at most `grace`.
    pub async fn shutdown(&self, grace: Duration) {
        let closed = self.shared.closed_notify.notified();
        match self.shared.begin_quiesce() {
            Quiesce::Closed => return,
            Quiesce::InProgress => {
                if !self.shared.is_closed() {
                    let _ = tokio::time::timeout(grace, closed).await;
                }
                if !self.shared.is_closed() {
                    self.close();
                }
                return;
            }
            Quiesce::Started => {}
        }
        let deadline = Instant::now() + grace;

        // First the link drivers: waiting on queues alone would mistake not started yet
        // for finished.
        let relays: Vec<_> = self.shared.drainable.lock().unwrap().drain(..).collect();
        for relay in relays {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = tokio::time::timeout(remaining, relay).await;
        }

        // Then caller-driven resource sessions, whose Drop queues their link close.
        loop {
            if self.shared.active_resources.load(Ordering::Acquire) == 0
                || Instant::now() >= deadline
            {
                break;
            }
            let notified = self.shared.resource_notify.notified();
            if self.shared.active_resources.load(Ordering::Acquire) == 0 {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let _ = tokio::time::timeout(remaining, notified).await;
        }

        // Then the wire: let the interface writers drain what the relays queued.
        loop {
            let drained = {
                let interfaces = self.shared.interfaces.lock().unwrap();
                interfaces.iter().all(|i| i.outbound.is_drained())
            };
            if drained || Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.close();
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // Aborting the tasks breaks the router<->`Shared` cycle and closes every socket.
        self.close();
    }
}

/// Spawn a task and record its abort handle on `shared`, so closing the endpoint cancels it.
/// Every endpoint task is spawned through here; an untracked one would outlive the endpoint.
pub(super) fn track<F>(shared: &Arc<Shared>, fut: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let state = shared.lifecycle.lock().unwrap();
    if *state != Lifecycle::Running {
        return false;
    }
    let handle = tokio::spawn(fut);
    let mut tasks = shared.tasks.lock().unwrap();
    // Forget finished tasks, so reconnect churn cannot grow this without bound.
    tasks.retain(|handle| !handle.is_finished());
    tasks.push(handle.abort_handle());
    true
}

/// Track a task that ends by itself once its input goes away, keeping the join
/// handle so [`Endpoint::shutdown`] can wait for it to finish draining. Still
/// abortable, so [`Endpoint::close`] remains an immediate stop.
pub(super) fn track_drainable<F>(shared: &Arc<Shared>, fut: F) -> bool
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    let state = shared.lifecycle.lock().unwrap();
    if *state != Lifecycle::Running {
        return false;
    }
    let handle = tokio::spawn(fut);
    shared.tasks.lock().unwrap().push(handle.abort_handle());
    shared.drainable.lock().unwrap().push(handle);
    true
}
