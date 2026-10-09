//! `Endpoint` construction, task tracking, and shutdown.

use alloc::vec::Vec;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use tokio::sync::{Mutex as AsyncMutex, mpsc};

use crate::address_book::AddressBook;
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
use super::router::route;
use super::routing::{RoutingPolicy, RoutingStats};
use super::shared::{Lifecycle, Quiesce, Shared};
use super::single::ReceivedSingle;
use super::watchdog::{LINK_WATCHDOG_TICK, watch_links};

/// Depth of the router's inbound queue. Bounded so a flooding peer cannot make the endpoint
/// buffer packets without limit: a TCP reader awaits when it is full (back-pressuring the
/// socket, so the flow control reaches the peer), and the [`InterfaceSink::deliver`] seam,
/// which cannot await, drops instead.
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
            relay_jitter_ms: AtomicU64::new(0),
            reliable_initial_rtt_ms: AtomicU64::new(DEFAULT_RELIABLE_INITIAL_RTT_MS),
            reliable_max_window: AtomicU32::new(DEFAULT_RELIABLE_MAX_WINDOW),
            reliable_decoded_frame_limit: AtomicUsize::new(DEFAULT_DECODED_FRAME_LIMIT),
            link_setup_retry_ms: AtomicU64::new(DEFAULT_LINK_SETUP_RETRY_MS),
            first_hop_airtime_ms: Mutex::new(HashMap::new()),
            link_mtu: AtomicU32::new(DEFAULT_LINK_MTU),
            inbound_link_proofs: Mutex::new(HashMap::new()),
            inbound: Mutex::new(InboundLinks::default()),
            resource_proofs: Mutex::new(HashMap::new()),
            path_table: Mutex::new(HashMap::new()),
            seen_announces: Mutex::new((HashSet::new(), VecDeque::new())),
            link_packets: Mutex::new(LinkPacketMemory::new()),
            packet_filter: Mutex::new(HashList::new(PACKET_HASHES)),
            path_request_tags: Mutex::new(HashList::new(PATH_REQUEST_TAGS)),
            announce_freshness: Mutex::new(AnnounceFreshnessState::new(freshness_policy)?),
            route_ttl_ms: AtomicU64::new(freshness_policy.route_ttl_ticks()),
            announce_admission: Mutex::new(
                AnnounceAdmission::new(AnnounceIngressPolicy::default()),
            ),
            announce_admission_started: tokio::time::Instant::now(),
            held_announces: Mutex::new(VecDeque::new()),
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
        let watchdog = Arc::clone(&shared);
        track(&shared, async move {
            let mut tick = tokio::time::interval(LINK_WATCHDOG_TICK);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                watch_links(&watchdog);
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

    /// The address book, for resolving learned peers.
    pub fn resolve(&self, dest: AddressHash) -> Option<Identity> {
        self.shared
            .address_book
            .lock()
            .unwrap()
            .resolve(dest)
            .map(|p| p.identity)
    }

    /// Stop the endpoint: abort the router, every interface reader and writer, any TCP
    /// listeners, and every link relay, closing their sockets. [`Drop`](Self::drop) calls
    /// this too; use it to release everything at a chosen point. Streams handed out earlier
    /// will see their connection end. Idempotent.
    pub fn close(&self) {
        if !self.shared.mark_closed() {
            return;
        }
        for handle in self.shared.tasks.lock().unwrap().drain(..) {
            handle.abort();
        }
        // Drop every link sender after aborting its driver. This releases best-effort,
        // reliable, and resource receivers even when the Endpoint itself remains alive.
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
        // Close every interface's outbound scheduler so a caller-driven pump parked in
        // `next_outbound` wakes and sees the end, rather than waiting on a sender that will
        // never come. (The channel this replaced ended implicitly when its sender dropped.)
        for i in self.shared.interfaces.lock().unwrap().iter() {
            i.outbound.close();
        }
        self.shared.closed_notify.notify_waiters();
    }

    /// Stop the endpoint, giving work already queued for the interfaces a
    /// bounded chance to reach the wire first.
    ///
    /// [`close`](Self::close) and [`Drop`](Self::drop) are abrupt by design:
    /// they abort every tracked task, including the interface writers, so a
    /// packet sitting in an outbound queue dies with them. That is fine for a
    /// hard stop and wrong for an orderly one, and the difference is not
    /// visible from the caller's side — `AsyncWrite::flush` on a link stream
    /// returns once the bytes reach the relay's duplex, long before they are
    /// framed, queued, and written.
    ///
    /// Finish or drop streams and resource sessions first, then await this. It
    /// waits for best-effort relays, reliable channel proofs, resource-session
    /// release, and both queued and in-flight interface packets. The grace
    /// deadline bounds the whole sequence, after which remaining work is aborted.
    ///
    /// A stream whose write side remains open, or a resource session still held
    /// by its caller, cannot finish itself. The deadline bounds those cases.
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

        // First the link drivers. Best-effort relays finish once their stream is
        // dropped; reliable drivers finish after both EOFs and all proofs. Waiting
        // on queues alone would confuse finished with not-started-yet.
        let relays: Vec<_> = self.shared.drainable.lock().unwrap().drain(..).collect();
        for relay in relays {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            // A relay whose stream a caller still holds never reaches EOF; the
            // deadline is what bounds that case.
            let _ = tokio::time::timeout(remaining, relay).await;
        }

        // Resource sessions are caller-driven rather than spawned tasks. Their Drop queues
        // the link-close packet, so give active sessions the same bounded opportunity to
        // finish or be released before checking the wire.
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
            // Short enough that an orderly close stays prompt, long enough not
            // to spin: the writers only need to be scheduled.
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        self.close();
    }
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        // Abort every spawned task. This releases the router's `Arc<Shared>` — breaking the
        // router<->`Shared` cycle that would otherwise keep the whole runtime alive — and
        // stops all interface tasks, listeners, and relays so their sockets close.
        self.close();
    }
}

/// Spawn a task and record its abort handle on `shared`, so the endpoint's drop can cancel
/// every task it started. Every `tokio::spawn` in this module goes through here; a task that
/// is not tracked would outlive the endpoint.
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
    // Forget the ones that have already ended. Handles were only ever appended, so a
    // process that connects and disconnects repeatedly grew this vector with the ghosts of
    // every finished task, and the abort-them-all on shutdown walked all of them.
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
