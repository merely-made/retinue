//! The endpoint runtime: the tokio shell that turns the R0–R4 primitives into a working
//! peer.
//!
//! An [`Endpoint`] holds an identity, an [`AddressBook`], and any number of **interfaces**
//! (TCP connections, dialed or accepted). A background router reads packets from every
//! interface, tagged with the interface they arrived on, and dispatches them: announces
//! populate the address book, inbound link requests are proved and surfaced as connections,
//! and link data is routed to the [`LinkStream`] for its link. Announces are broadcast on
//! every interface; a link's traffic goes back out the interface it came in on. Links are
//! exposed as [`LinkStream`]s (`AsyncRead` + `AsyncWrite`), an ordinary bidirectional byte
//! stream.
//!
//! Multiple interfaces are the substrate for routing (a node that forwards between them) and
//! for a host transport reaching many peers. This is the seam a host implements its own
//! transport trait against; see the crate root.

use alloc::format;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;

use std::collections::{HashMap, HashSet, VecDeque};
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

use crate::address_book::AddressBook;
use crate::announce::{self, ANNOUNCE_NONCE_LEN, Announce, AnnounceBlob, TimebaseGenerator};
use crate::announce_admission::{
    AnnounceAdmission, AnnounceIngressCounters, AnnounceIngressPolicy, DestinationVerdict,
    InterfaceVerdict,
};
use crate::announce_freshness::{
    AnnounceFreshness, AnnounceFreshnessCandidate, AnnounceFreshnessConfig,
    AnnounceFreshnessConfigError, AnnounceFreshnessDecision, AnnounceFreshnessReject,
};
use crate::channel::{DEFAULT_DECODED_FRAME_LIMIT, StreamDecodeError, StreamDecodeLimitError};
use crate::destination::DestinationName;
use crate::hash::{AddressHash, NameHash};
use crate::identity::{Identity, KEY_LEN, PrivateIdentity};
use crate::ifac::Ifac;
use crate::iface::hdlc::{Deframer, frame};
use crate::link::{
    self, CTX_CHANNEL, CTX_LINKCLOSE, CTX_LINKIDENTIFY, Inbound, Link, LinkMode, LinkTrailer,
};
use crate::link_liveness::{self, Due, Liveness};
use crate::node::InterfaceMode;
use crate::packet::{DestinationType, Packet, PacketType};
use crate::ratchet::RatchetStore;
use crate::reliable::ReliableChannel;
use crate::request::{Request, Response};
use crate::resource::RANDOM_HASH_LEN;
use crate::resource_transfer::{ResourceReceiver, ResourceSender};
use crate::token::{IV_LEN, TOKEN_OVERHEAD};

/// RNS's link MDU (`Link.py` 73, 512-514): the largest plaintext whose CBC-padded token and
/// minimal header fit `mtu` with RNS's one-byte IFAC reserve. 431 at MTU 500. A longer
/// access code is accounted for by the interface's frame limit, which counts it.
const fn link_mdu(mtu: usize) -> usize {
    const IFAC_MIN_SIZE: usize = 1;
    let room = mtu.saturating_sub(IFAC_MIN_SIZE + crate::packet::HEADER_MIN_LEN + TOKEN_OVERHEAD);
    (room / 16 * 16).saturating_sub(1)
}

/// Largest plaintext per link data packet at Reticulum's MTU.
const WRITE_CHUNK: usize = link_mdu(crate::packet::MTU);

/// Largest single-packet link plaintext (data, request or response) at the MTU negotiated
/// for this link, so the interface driver never has to reject a packet after `AsyncWrite`
/// already accepted its bytes.
fn write_chunk_for_mtu(mtu: u32) -> usize {
    link_mdu(mtu as usize).clamp(1, WRITE_CHUNK)
}

/// In-memory buffer for a stream's inbound side.
const DUPLEX_BUF: usize = 64 * 1024;

/// The reliable driver's clock period. Each period advances the channel's millisecond clock
/// by this much, which drives retransmission of unproven channel packets once their
/// RTT-derived timeout passes. One timer per active reliable link; a production build would
/// pause it when the link is fully idle.
const RELIABLE_TICK_MS: u64 = 50;

/// Fast interfaces start here; radio callers can raise it before opening links.
const DEFAULT_RELIABLE_INITIAL_RTT_MS: u64 = 750;

/// Default dynamic Channel ceiling, matching RNS. Strict half-duplex callers
/// can lower it without changing the wire format.
const DEFAULT_RELIABLE_MAX_WINDOW: u32 = crate::channel::WINDOW_MAX;

/// Packets or chunks queued for one link's driver or stream relay. Past this the router
/// drops further link traffic and counts it, instead of buffering without bound: a reliable
/// driver recovers by retransmission, and a best-effort stream fails with an error.
const LINK_QUEUE: usize = 256;

/// Accepted links waiting for the application, per destination. A link request that would
/// overflow its destination's backlog is refused before any link state is created.
const ACCEPT_BACKLOG: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Lifecycle {
    Running,
    Quiescing,
    Closed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Quiesce {
    Started,
    InProgress,
    Closed,
}

/// Default link MTU advertised by Reticulum. Radio callers can lower it to
/// keep encrypted Channel frames and resource parts inside a proven RF size.
const DEFAULT_LINK_MTU: u32 = crate::packet::MTU as u32;
/// Smallest link MTU currently exercised by the direct-PHY Data and Resource
/// paths. It leaves room for an eight-byte IFAC on a 255-byte packet radio.
const MIN_LINK_MTU: u32 = 247;

/// How many times an initiator sends its IDENTIFY over the opening retransmit ticks. RNS
/// sends it once; on a lossy medium a single drop leaves the responder unable to validate our
/// proofs of the data it sends us, stalling that direction with no way to recover. The wire
/// protocol has no IDENTIFY ack, so we simply re-send it a bounded few times, which survives
/// realistic early loss without ever spinning.
const IDENTIFY_MAX_SENDS: u32 = 4;

/// How many copies of a completed Resource receipt are queued before the receiving call
/// returns. Resource proofs have no acknowledgement of their own, so a single lost proof
/// otherwise leaves the publisher waiting after the receiver has already recovered the data.
const RESOURCE_PROOF_MAX_SENDS: u32 = 4;

/// How often the link watchdog advances every link's keepalive, staleness and handshake
/// timers (see [`crate::link_liveness`]). The shortest keepalive interval is 5 s.
const LINK_WATCHDOG_TICK: Duration = Duration::from_millis(500);

/// Default interval between identical link-request transmissions while setup is pending.
const DEFAULT_LINK_SETUP_RETRY_MS: u64 = 2_000;

/// Recent accepted requests whose proof can be replayed idempotently when the initiator
/// retries after losing a proof.
const LINK_REQUEST_CACHE: usize = 1_024;
const LINK_REQUEST_CACHE_TTL: Duration = Duration::from_secs(30);

/// Runtime policy for an endpoint-driven resource transfer.
#[derive(Clone, Copy, Debug)]
pub struct ResourceTransferConfig {
    /// Maximum time allowed for the complete transfer.
    pub timeout: Duration,
    /// Interval between advertisement or request retransmissions.
    pub retry_interval: Duration,
    /// Maximum resource parts requested in one half-duplex turn.
    pub request_window: usize,
}

impl Default for ResourceTransferConfig {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            retry_interval: Duration::from_millis(500),
            request_window: crate::resource::HASHMAP_MAX_PARTS,
        }
    }
}

/// Depth of the router's inbound queue. Bounded so a flooding peer cannot make the endpoint
/// buffer packets without limit: a TCP reader awaits when it is full (back-pressuring the
/// socket, so the flow control reaches the peer), and the [`InterfaceSink::deliver`] seam,
/// which cannot await, drops instead.
const ROUTER_QUEUE: usize = 1024;

/// How long a validated bridged link's interface pair is remembered after its last packet.
///
/// A transport node records which two interfaces a forwarded link joins, so a proof or link
/// data arriving on one goes out the other. An hour is far longer than any live link goes
/// quiet, and short enough that a node carrying strangers' traffic all day does not
/// accumulate the day. A link not yet proved lapses sooner, at its proof deadline, and the
/// table is bounded by [`LINK_TRANSPORT_CAPACITY`].
const LINK_TRANSPORT_TTL: Duration = Duration::from_secs(3600);

/// How long a validated link may go unheard before a new request may take its place in a full
/// table: RNS's `Transport.LINK_TIMEOUT`, as [`crate::node::LINK_TRANSPORT_IDLE`].
const LINK_TRANSPORT_IDLE: Duration = Duration::from_millis(crate::node::LINK_TRANSPORT_IDLE);

/// The most links a transport node carries at once. A request arriving at the bound displaces
/// the stalest link still awaiting its proof, or else a validated one unheard for
/// [`LINK_TRANSPORT_IDLE`]; a recently heard validated link is never displaced, so forged
/// requests cannot evict carried links or grow the table.
#[cfg(not(test))]
const LINK_TRANSPORT_CAPACITY: usize = 4096;
#[cfg(test)]
const LINK_TRANSPORT_CAPACITY: usize = 4;

/// A link carried through this node: its two interfaces, and whether its destination has
/// proved it yet.
#[derive(Clone, Copy, Debug)]
struct LinkBridge {
    from: InterfaceId,
    out: InterfaceId,
    /// The destination the request named, whose identity signs the proof.
    destination: AddressHash,
    seen: Instant,
    /// When an unproved link lapses; `None` once the destination's proof has passed.
    proof_deadline: Option<Instant>,
}

impl LinkBridge {
    fn lapsed(&self, now: Instant) -> bool {
        self.proof_deadline.map_or(
            now.duration_since(self.seen) >= LINK_TRANSPORT_TTL,
            |deadline| now >= deadline,
        )
    }

    /// Whether `pkt`, heard on `iface`, may cross: `None` if not, else whether it is the
    /// proof that validates the bridge once carried. Only the bridge's two sides may use it,
    /// nothing crosses before the destination's proof, and the proof must come from the
    /// destination's side under its signature (the side check stands alone when its identity
    /// is unknown).
    fn admit(&self, iface: InterfaceId, pkt: &Packet, book: &Mutex<AddressBook>) -> Option<bool> {
        if iface != self.from && iface != self.out {
            return None;
        }
        if pkt.packet_type != PacketType::Proof || pkt.context != link::CTX_LRPROOF {
            return self.proof_deadline.is_none().then_some(false);
        }
        let signed = iface == self.out
            && book
                .lock()
                .unwrap()
                .resolve(self.destination)
                .is_none_or(|peer| link::proof_is_signed_by(pkt, &peer.identity));
        signed.then_some(true)
    }
}

/// Maximum hops an announce or packet may travel before a transport node drops it. RNS's
/// default `m` (`PATHFINDER_M`).
const MAX_HOPS: u8 = 128;

/// How many recent announce packet-hashes to remember for de-duplication.
const SEEN_ANNOUNCES: usize = 4096;

/// Recent link packet hashes, both ways, across every link this endpoint holds.
///
/// On a shared medium a relay's retransmission of our own link packet reaches us under the
/// shared link key, with hops+1 and the same hash: `sent` marks it as ours rather than the
/// far end's. That covers Channel too, where taking our own sequence for the far end's would
/// also drop the far end's real one as a repeat. The same medium hands us the far end's packet
/// twice, directly and from a relay: `received` delivers it once.
struct LinkPacketMemory {
    sent: HashWindow,
    received: HashWindow,
}

impl LinkPacketMemory {
    fn new() -> Self {
        Self {
            sent: HashWindow::new(crate::capacity::desktop::OWN_ECHO_HASHES),
            received: HashWindow::new(crate::capacity::desktop::DUPLICATE_HASHES),
        }
    }

    /// Note a link packet we transmit.
    fn note_sent(&mut self, pkt: &Packet) {
        if is_remembered_link_packet(pkt) {
            self.sent.insert(pkt.hash());
        }
    }

    /// Whether an inbound packet on one of our links is new, our own, or a repeat.
    fn admit(&mut self, pkt: &Packet) -> LinkPacketAdmission {
        if !is_remembered_link_packet(pkt) {
            return LinkPacketAdmission::New;
        }
        let hash = pkt.hash();
        if self.sent.contains(&hash) {
            LinkPacketAdmission::OwnEcho
        } else if self.received.insert(hash) {
            LinkPacketAdmission::New
        } else {
            LinkPacketAdmission::Duplicate
        }
    }
}

/// What [`LinkPacketMemory::admit`] made of an inbound link packet.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinkPacketAdmission {
    New,
    OwnEcho,
    Duplicate,
}

fn is_remembered_link_packet(pkt: &Packet) -> bool {
    pkt.packet_type == PacketType::Data
        && pkt.destination_type == DestinationType::Link
        && crate::node::is_deduplicated_link_context(pkt.context)
}

/// A bounded set of packet hashes that forgets the oldest first. A burst can outrun it; that
/// only lets a late copy through, never drops a new packet.
struct HashWindow {
    set: HashSet<AddressHash>,
    order: VecDeque<AddressHash>,
    capacity: usize,
}

impl HashWindow {
    fn new(capacity: usize) -> Self {
        Self {
            set: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    fn contains(&self, hash: &AddressHash) -> bool {
        self.set.contains(hash)
    }

    /// Record a hash; false if it was already there.
    fn insert(&mut self, hash: AddressHash) -> bool {
        if !self.set.insert(hash) {
            return false;
        }
        self.order.push_back(hash);
        if self.order.len() > self.capacity
            && let Some(oldest) = self.order.pop_front()
        {
            self.set.remove(&oldest);
        }
        true
    }
}

/// Hashes remembered in two generations, as RNS keeps its packet hash list and the one before
/// it (`Transport.py` 832-834): once the current generation holds `capacity`, it becomes the
/// previous one, so a burst forgets the older half rather than everything.
struct HashList {
    current: HashSet<AddressHash>,
    previous: HashSet<AddressHash>,
    capacity: usize,
}

impl HashList {
    fn new(capacity: usize) -> Self {
        Self {
            current: HashSet::new(),
            previous: HashSet::new(),
            capacity,
        }
    }

    /// Record a hash; false if it was already remembered.
    fn insert(&mut self, hash: AddressHash) -> bool {
        if self.previous.contains(&hash) || !self.current.insert(hash) {
            return false;
        }
        if self.current.len() >= self.capacity {
            self.previous = core::mem::take(&mut self.current);
        }
        true
    }
}

/// Packet hashes held per generation by the endpoint's packet filter.
const PACKET_HASHES: usize = 32_768;

/// Path request tags held per generation, RNS's `max_pr_tags` (`Transport.py` 195).
const PATH_REQUEST_TAGS: usize = 16_000;

/// Host-owned policy for receive-side announce freshness.
///
/// This is deliberately independent of the packet-loop cache: it bounds receiver memory and
/// decides whether a verified announce may mutate peer, path, publication, or relay state. A
/// destination's freshness lives exactly as long as its route, as RNS keeps announce blobs on
/// the path-table row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnnounceFreshnessPolicy {
    /// How long a learned route, and with it the destination's freshness, stays live. One
    /// week by default, as in RNS ([`crate::node::DEFAULT_ROUTE_TTL`]); a route carrying
    /// traffic is refreshed, and an access-point or roaming interface shortens it (see
    /// [`InterfaceMode`]).
    pub route_ttl: Duration,
    /// Maximum destinations retained in the freshness ledger. Evicting one drops its route.
    pub destination_capacity: usize,
    /// Maximum full announce blobs retained for one destination.
    pub blob_capacity: usize,
}

impl Default for AnnounceFreshnessPolicy {
    fn default() -> Self {
        Self {
            route_ttl: Duration::from_millis(crate::node::DEFAULT_ROUTE_TTL),
            destination_capacity: 4_096,
            blob_capacity: 16,
        }
    }
}

impl AnnounceFreshnessPolicy {
    fn config(self) -> AnnounceFreshnessConfig {
        AnnounceFreshnessConfig {
            destination_capacity: self.destination_capacity,
            blob_capacity: self.blob_capacity,
        }
    }

    fn route_ttl_ticks(self) -> u64 {
        duration_ticks(self.route_ttl)
    }
}

fn duration_ticks(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

/// The most destinations a path table will hold.
///
/// The table was unbounded, which made it the last place a stranger could grow this
/// process's memory for free: every announce that survived the address book's cap put an
/// entry here and nothing ever took one out except expiry.
///
/// The eviction policy falls out of what feeds the table. Routes are learned from announces
/// and their time is refreshed by re-announces, so the entry with the oldest `learned` is
/// exactly the peer that has gone quietest, and the one whose route is least likely to still
/// be true. Evicting it costs a path request if that peer comes back; keeping it costs a
/// route we would have had to a peer that is still talking. Expired entries go first, so a
/// table full of the dead never evicts the living.
///
/// Sized for a transport node with a real neighbourhood rather than a bench: four thousand
/// destinations is far more than a LoRa mesh sees, and a bound that is never reached in
/// practice is the point.
#[cfg(not(test))]
const PATH_TABLE_CAPACITY: usize = 4096;
#[cfg(test)]
const PATH_TABLE_CAPACITY: usize = 4;

/// The least time between path requests we will broadcast for the same destination.
///
/// A path request is a broadcast, and the things that provoke one are usually inbound: a
/// message from somebody we cannot identify, a retry from a peer that has gone stale. Without
/// a floor, a peer sending traffic we cannot verify would make us broadcast once per packet,
/// which on a shared band is a stranger deciding how much of it we use.
// The test values keep the suite fast; note that integration tests in dependent crates
// (outrider's, for instance) compile this crate WITHOUT cfg(test) and therefore run against
// the real 20-second floor. A test over there that needs two requests for one destination
// will hang on the second, mysteriously, unless it knows this.
#[cfg(not(test))]
const PATH_REQUEST_MIN_INTERVAL: Duration = Duration::from_secs(20);
#[cfg(test)]
const PATH_REQUEST_MIN_INTERVAL: Duration = Duration::from_millis(60);

/// The most path requests that may be broadcast in any [`PATH_REQUEST_MIN_INTERVAL`] window,
/// across ALL destinations.
///
/// The per-destination floor alone is not a rate limit, because the peer that provokes a path
/// request also chooses the destination: a flood of unverifiable packets with fabricated,
/// unique sources would get one broadcast each, and the floor would never engage since no key
/// repeats. This cap bounds the aggregate, and — because a refused request records nothing —
/// it also bounds how fast the budget table can be made to grow.
const PATH_REQUEST_GLOBAL_MAX: usize = 8;

/// A bidirectional byte stream over a link.
///
/// Delegates [`AsyncRead`]/[`AsyncWrite`] to an internal duplex; a relay task chunks writes
/// into encrypted link data packets and the endpoint router feeds decrypted inbound data
/// back in. Dropping the stream ends its relay.
pub struct LinkStream {
    inner: DuplexStream,
    /// Set by the reliable driver before it drops its duplex half on a terminal failure.
    receive_error: Option<Arc<Mutex<Option<StreamFault>>>>,
    /// Set when the link watchdog dropped the link: its end is then a timeout.
    lost: Arc<AtomicBool>,
    /// The link id, exposed for diagnostics.
    link_id: AddressHash,
    /// The interface this link arrived on (inbound) or was opened over (outbound).
    iface: InterfaceId,
}

impl LinkStream {
    /// The id of the link carrying this stream.
    pub fn link_id(&self) -> AddressHash {
        self.link_id
    }

    /// The interface this link arrived on.
    ///
    /// Ingress is a *fact about the session*, so it lives on the stream rather
    /// than only on the accepted-session wrappers: the reliable accept path
    /// surfaces a bare `LinkStream`, and it must report the same ingress as the
    /// best-effort and Resource paths instead of diverging.
    pub fn interface(&self) -> InterfaceId {
        self.iface
    }
}

/// Why a reliable stream ended badly. Its driver records this before dropping its duplex
/// half, so the reader sees an error instead of a clean end of stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StreamFault {
    /// A received frame could not be decoded.
    Decode(StreamDecodeError),
    /// A sent packet went unproved through every try, so the link was closed (RNS tears a
    /// link down when its channel times out).
    Unacknowledged,
    /// A best-effort link's reader fell so far behind that received bytes were dropped,
    /// so the link was closed rather than leave a hole in the stream.
    Overrun,
}

impl From<StreamDecodeError> for StreamFault {
    fn from(error: StreamDecodeError) -> Self {
        Self::Decode(error)
    }
}

impl AsyncRead for LinkStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) if buf.filled().len() == before => {
                if let Some(error) = self
                    .receive_error
                    .as_ref()
                    .and_then(|state| *state.lock().unwrap())
                {
                    let (kind, message) = match error {
                        StreamFault::Decode(StreamDecodeError::UnsupportedCompression) => (
                            io::ErrorKind::InvalidData,
                            "compressed stream frame unsupported",
                        ),
                        StreamFault::Decode(StreamDecodeError::InvalidCompression) => (
                            io::ErrorKind::InvalidData,
                            "invalid compressed stream frame",
                        ),
                        StreamFault::Decode(StreamDecodeError::DecodedFrameLimitExceeded {
                            ..
                        }) => (
                            io::ErrorKind::InvalidData,
                            "decoded stream frame limit exceeded",
                        ),
                        StreamFault::Unacknowledged => (
                            io::ErrorKind::TimedOut,
                            "reliable link closed: a packet went unproved through every retry",
                        ),
                        StreamFault::Overrun => (
                            io::ErrorKind::Other,
                            "link closed: the reader fell behind and received bytes were lost",
                        ),
                    };
                    Poll::Ready(Err(io::Error::new(kind, message)))
                } else if self.lost.load(Ordering::Acquire) {
                    Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "link timed out: the peer stopped answering",
                    )))
                } else {
                    Poll::Ready(Ok(()))
                }
            }
            result => result,
        }
    }
}

impl AsyncWrite for LinkStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A live link whose raw packets are driven by the resource transfer state machines.
///
/// One session carries one transfer at a time. A peer may either publish to this session or
/// fetch from it; the other side performs the complementary operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReceivedPayload {
    /// One decrypted best-effort link packet.
    Data(Vec<u8>),
    /// One fully received, verified Resource.
    Resource(Vec<u8>),
}

/// The wire form selected for one payload on a link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PayloadMode {
    /// One encrypted link data packet.
    Data,
    /// A segmented, proved Resource transfer.
    Resource,
}

/// One request received over a resource-capable link.
#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedRequest {
    /// The decoded request body.
    pub request: Request,
    /// Hash of the encrypted request packet, echoed by the response.
    pub request_id: AddressHash,
    /// Identity proven by a preceding link IDENTIFY, when present.
    pub peer: Option<Identity>,
}

/// One decrypted request packet before an application interprets its
/// MessagePack value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedRawRequest {
    /// Complete decrypted request structure.
    pub packed: Vec<u8>,
    /// Hash of the encrypted request packet, echoed by the response.
    pub request_id: AddressHash,
    /// Identity proven by a preceding link IDENTIFY, when present.
    pub peer: Option<Identity>,
}

/// One decrypted response before an application interprets its MessagePack
/// value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReceivedRawResponse {
    /// Complete decrypted response structure.
    pub packed: Vec<u8>,
    /// Request id read from the first response item.
    pub request_id: AddressHash,
}

pub struct ResourceSession {
    shared: Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    packets: mpsc::Receiver<Packet>,
    config: ResourceTransferConfig,
    identified_peer: Option<Identity>,
}

impl ResourceSession {
    /// The id of the link carrying this resource session.
    pub fn link_id(&self) -> AddressHash {
        self.link.id()
    }

    /// The interface this resource link arrived on.
    pub fn interface(&self) -> InterfaceId {
        self.iface
    }

    fn retain_identified_peer(&self, identity: Identity) {
        self.shared.write_diagnostic(|| {
            let mut links = self.shared.links.lock().unwrap();
            let Some(entry) = links.get_mut(&self.link.id()) else {
                return ((), false);
            };
            let changed = if entry.remote.identity == Some(identity) {
                false
            } else {
                entry.remote.identity = Some(identity);
                true
            };
            ((), changed)
        });
    }

    /// Replace the retry and overall timeout policy for subsequent transfer work.
    pub fn set_config(&mut self, config: ResourceTransferConfig) {
        self.config = config;
    }

    /// Publish one payload and wait until the receiver proves complete receipt.
    pub async fn publish(&mut self, data: &[u8]) -> io::Result<()> {
        let mut random_hash = [0_u8; RANDOM_HASH_LEN];
        fill_random(&mut random_hash);
        let sender = ResourceSender::publish(self.link.clone(), data, random_hash, &next_iv());
        self.publish_sender(sender).await
    }

    async fn publish_response_value(
        &mut self,
        request_id: AddressHash,
        packed_value: &[u8],
    ) -> io::Result<()> {
        let mut random_hash = [0_u8; RANDOM_HASH_LEN];
        fill_random(&mut random_hash);
        let sender = ResourceSender::respond(
            self.link.clone(),
            packed_value,
            *request_id.as_bytes(),
            random_hash,
            &next_iv(),
        );
        self.publish_sender(sender).await
    }

    async fn publish_sender(&mut self, mut sender: ResourceSender) -> io::Result<()> {
        self.shared
            .send_on(self.iface, sender.advertisement(&next_iv()));

        let shared = Arc::clone(&self.shared);
        let iface = self.iface;
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let transfer = async {
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                        })?;
                        for outbound in sender.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        if sender.is_done() {
                            return Ok(());
                        } else if sender.is_canceled() {
                            return Err(io::Error::new(
                                io::ErrorKind::ConnectionAborted,
                                "resource publish canceled by receiver",
                            ));
                        }
                    }
                    _ = interval.tick() => {
                        if !sender.has_started() {
                            shared.send_on(iface, sender.advertisement(&next_iv()));
                        }
                    }
                }
            }
        };
        match tokio::time::timeout(self.config.timeout, transfer).await {
            Ok(result) => result,
            Err(_) => {
                let message = if sender.served_parts() > 0 {
                    format!(
                        "resource publish timed out after serving {} requested part(s)",
                        sender.served_parts()
                    )
                } else if sender.has_started() {
                    "resource publish timed out after receiver request matched no parts".to_string()
                } else {
                    "resource publish timed out before receiver request".to_string()
                };
                Err(io::Error::new(io::ErrorKind::TimedOut, message))
            }
        }
    }

    /// Fetch one payload published by the peer, returning after verification and proof.
    pub async fn fetch(&mut self) -> io::Result<Vec<u8>> {
        let mut receiver =
            ResourceReceiver::with_request_window(self.link.clone(), self.config.request_window);
        let shared = Arc::clone(&self.shared);
        let iface = self.iface;
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let transfer = async move {
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                        })?;
                        for outbound in receiver.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        if let Some(data) = receiver.data().map(|data| data.to_vec()) {
                            queue_resource_proof_replays(&shared, iface, &mut receiver);
                            return Ok(data);
                        } else if let Some(error) = receiver.failure() {
                            return Err(resource_receive_failure(error));
                        } else if receiver.is_canceled() {
                            return Err(io::Error::new(
                                io::ErrorKind::ConnectionAborted,
                                "resource fetch canceled by sender",
                            ));
                        }
                    }
                    _ = interval.tick() => {
                        for outbound in receiver.retransmit(next_iv) {
                            shared.send_on(iface, outbound);
                        }
                    }
                }
            }
        };
        tokio::time::timeout(self.config.timeout, transfer)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "resource fetch timed out"))?
    }

    /// Receive either one best-effort data packet or one complete Resource.
    ///
    /// Protocols such as LXMF use both delivery forms on the same destination.
    /// Register that destination with [`Endpoint::register_resource`], then use
    /// this method instead of deciding the inbound form before the link arrives.
    pub async fn receive(&mut self) -> io::Result<ReceivedPayload> {
        let mut receiver =
            ResourceReceiver::with_request_window(self.link.clone(), self.config.request_window);
        let shared = Arc::clone(&self.shared);
        let link = self.link.clone();
        let iface = self.iface;
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let mut identified = self.identified_peer;
        let transfer = async move {
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                        })?;
                        // An IDENTIFY on a resource link is the sender telling us who it is,
                        // signed under the link. Reading it here is what lets a receiver
                        // authenticate a first message from somebody it has never heard
                        // announce: the request path already did this, and dropping it here
                        // meant the strongest evidence available was thrown away in favour of
                        // an address-book lookup that could only fail.
                        if let Some(identity) = link.read_identify(&packet) {
                            identified = Some(identity);
                            continue;
                        }
                        match link.receive(&packet) {
                            Some(Inbound::Data(data)) => {
                                return Ok((identified, ReceivedPayload::Data(data)));
                            }
                            Some(Inbound::Close) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    "resource link closed",
                                ));
                            }
                            _ => {}
                        }
                        for outbound in receiver.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        if let Some(data) = receiver.data().map(|data| data.to_vec()) {
                            queue_resource_proof_replays(&shared, iface, &mut receiver);
                            return Ok((identified, ReceivedPayload::Resource(data)));
                        } else if let Some(error) = receiver.failure() {
                            return Err(resource_receive_failure(error));
                        } else if receiver.is_canceled() {
                            return Err(io::Error::new(
                                io::ErrorKind::ConnectionAborted,
                                "resource fetch canceled by sender",
                            ));
                        }
                    }
                    _ = interval.tick() => {
                        for outbound in receiver.retransmit(next_iv) {
                            shared.send_on(iface, outbound);
                        }
                    }
                }
            }
        };
        let (identified, payload) = tokio::time::timeout(self.config.timeout, transfer)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "payload receive timed out"))??;
        self.identified_peer = identified;
        if let Some(identity) = identified {
            self.retain_identified_peer(identity);
        }
        Ok(payload)
    }

    /// The peer identity proven by an IDENTIFY on this link, if the sender sent one.
    ///
    /// Stronger evidence than an announce: an announce says a destination exists somewhere,
    /// while this is the peer on the other end of *this* link signing that it is that
    /// identity. A caller still has to check that the identity is the one its payload claims
    /// as the source, because IDENTIFY proves who the peer is and says nothing about who the
    /// payload says it is from.
    pub fn identified_peer(&self) -> Option<Identity> {
        self.identified_peer
    }

    /// Wait for one request packet on this link.
    pub async fn receive_request(&mut self) -> io::Result<ReceivedRequest> {
        let raw = self.receive_raw_request().await?;
        let request = Request::unpack(&raw.packed).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid byte request payload")
        })?;
        Ok(ReceivedRequest {
            request,
            request_id: raw.request_id,
            peer: raw.peer,
        })
    }

    /// Wait for one request and retain its complete decrypted MessagePack.
    ///
    /// RNS permits the request's third item to be an application value rather
    /// than a binary blob. Consumers with their own grammar use this method;
    /// byte-oriented requests can use [`receive_request`](Self::receive_request).
    pub async fn receive_raw_request(&mut self) -> io::Result<ReceivedRawRequest> {
        let link = self.link.clone();
        let packets = &mut self.packets;
        let mut peer = self.identified_peer;
        let receive = async move {
            loop {
                let packet = packets.recv().await.ok_or_else(|| {
                    io::Error::new(io::ErrorKind::BrokenPipe, "request link closed")
                })?;
                if let Some(identity) = link.read_identify(&packet) {
                    peer = Some(identity);
                    continue;
                }
                match link.receive(&packet) {
                    Some(Inbound::Request(bytes)) => {
                        return Ok(ReceivedRawRequest {
                            packed: bytes,
                            request_id: packet.hash(),
                            peer,
                        });
                    }
                    Some(Inbound::Close) => {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "request link closed",
                        ));
                    }
                    _ => {}
                }
            }
        };
        let received = tokio::time::timeout(self.config.timeout, receive)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "request receive timed out"))??;
        self.identified_peer = received.peer;
        if let Some(identity) = received.peer {
            self.retain_identified_peer(identity);
        }
        Ok(received)
    }

    /// Send one response to a request received on this session.
    pub fn respond(&self, request_id: AddressHash, data: Vec<u8>) {
        let response = Response::new(request_id, data);
        self.shared.send_on(
            self.iface,
            self.link.response_packet(&response.pack(), &next_iv()),
        );
    }

    /// Send a response whose data is already one MessagePack value.
    pub fn respond_value(&self, request_id: AddressHash, packed_value: &[u8]) {
        let packed = Response::pack_value(request_id, packed_value);
        self.shared
            .send_on(self.iface, self.link.response_packet(&packed, &next_iv()));
    }

    /// Respond with opaque bytes, degrading to a Resource when the complete
    /// response envelope does not fit one encrypted link packet.
    pub async fn respond_auto(
        &mut self,
        request_id: AddressHash,
        data: Vec<u8>,
    ) -> io::Result<PayloadMode> {
        let packed_value = Response::pack_binary_value(&data);
        self.respond_value_auto(request_id, &packed_value).await
    }

    /// Respond with one already-packed MessagePack value, degrading to a
    /// Resource when the complete response envelope does not fit one encrypted
    /// link packet.
    pub async fn respond_value_auto(
        &mut self,
        request_id: AddressHash,
        packed_value: &[u8],
    ) -> io::Result<PayloadMode> {
        let packed = Response::pack_value(request_id, packed_value);
        if packed.len() <= write_chunk_for_mtu(self.link.mtu()) {
            self.shared
                .send_on(self.iface, self.link.response_packet(&packed, &next_iv()));
            Ok(PayloadMode::Data)
        } else {
            self.publish_response_value(request_id, &packed).await?;
            Ok(PayloadMode::Resource)
        }
    }

    /// Identify this endpoint's local identity to the remote link.
    pub fn identify(&self) {
        self.shared.send_on(
            self.iface,
            self.link.identify_packet(&self.shared.identity, &next_iv()),
        );
    }

    /// Send one request and wait for its matching response.
    pub async fn request(&mut self, request: &Request) -> io::Result<Response> {
        let raw = self.request_raw(&request.pack()).await?;
        Response::unpack(&raw.packed).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidData, "invalid byte response payload")
        })
    }

    /// Send one already-packed request and retain the raw matching response.
    pub async fn request_raw(&mut self, packed_request: &[u8]) -> io::Result<ReceivedRawResponse> {
        // Outgoing request Resources are not implemented. Refuse a value that
        // cannot fit this link instead of transmitting an oversized packet.
        if packed_request.len() > write_chunk_for_mtu(self.link.mtu()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "request exceeds link packet capacity",
            ));
        }
        let packet = self.link.request_packet(packed_request, &next_iv());
        let request_id = packet.hash();
        self.shared.send_on(self.iface, packet);

        let link = self.link.clone();
        let shared = Arc::clone(&self.shared);
        let iface = self.iface;
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let request_window = self.config.request_window;
        let receive = async move {
            let mut receiver = ResourceReceiver::with_request_window(link.clone(), request_window);
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "request link closed")
                        })?;
                        match link.receive(&packet) {
                            Some(Inbound::Response(bytes)) => {
                                let response_id = Response::request_id(&bytes).map_err(|_| {
                                    io::Error::new(io::ErrorKind::InvalidData, "invalid response envelope")
                                })?;
                                if response_id == request_id {
                                    return Ok(ReceivedRawResponse {
                                        packed: bytes,
                                        request_id: response_id,
                                    });
                                }
                            }
                            Some(Inbound::Close) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    "request link closed",
                                ));
                            }
                            _ => {}
                        }
                        for outbound in receiver.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        if let Some(packed) = receiver.data().map(|bytes| bytes.to_vec()) {
                            queue_resource_proof_replays(&shared, iface, &mut receiver);
                            let response_id = Response::request_id(&packed).map_err(|_| {
                                io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "invalid response resource",
                                )
                            })?;
                            if let Some(advertised_id) = receiver.response_request_id()
                                && AddressHash::from_bytes(advertised_id) != response_id
                            {
                                return Err(io::Error::new(
                                    io::ErrorKind::InvalidData,
                                    "response Resource request id mismatch",
                                ));
                            }
                            if response_id == request_id {
                                return Ok(ReceivedRawResponse {
                                    packed,
                                    request_id: response_id,
                                });
                            }
                            receiver =
                                ResourceReceiver::with_request_window(link.clone(), request_window);
                        } else if let Some(error) = receiver.failure() {
                            return Err(resource_receive_failure(error));
                        } else if receiver.is_canceled() {
                            return Err(io::Error::new(
                                io::ErrorKind::ConnectionAborted,
                                "response resource canceled by sender",
                            ));
                        }
                    }
                    _ = interval.tick() => {
                        for outbound in receiver.retransmit(next_iv) {
                            shared.send_on(iface, outbound);
                        }
                    }
                }
            }
        };
        tokio::time::timeout(self.config.timeout, receive)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "response receive timed out"))?
    }
}

impl Drop for ResourceSession {
    fn drop(&mut self) {
        self.shared.remove_link(self.link.id());
        self.shared
            .send_on(self.iface, self.link.close_packet(&next_iv()));
        self.shared.end_resource();
    }
}

/// The I/O error for a resource this endpoint refused to receive: one that needs segment
/// accumulation, or whose body inflated past the decompression limit. The receiver has
/// already sent the sender its cancel.
fn resource_receive_failure(error: crate::Error) -> io::Error {
    let kind = match error {
        crate::Error::MultiSegmentResource => io::ErrorKind::Unsupported,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, error)
}

/// Queue bounded duplicate receipts while the resource link is still registered.
///
/// The first receipt was emitted by [`ResourceReceiver::on_packet`]. These copies are
/// deliberately queued rather than awaited: the interface owns physical pacing, while the
/// application can persist and surface the already-verified payload without an artificial
/// retry-delay pause.
fn queue_resource_proof_replays(
    shared: &Shared,
    iface: InterfaceId,
    receiver: &mut ResourceReceiver,
) {
    for _ in 1..RESOURCE_PROOF_MAX_SENDS {
        for proof in receiver.retransmit(next_iv) {
            shared.send_on(iface, proof);
        }
    }
}

/// A validated announce observation, surfaced without exposing the endpoint's mutable
/// address book or path table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnnounceFact {
    /// The destination hash announced.
    pub destination: AddressHash,
    /// The announcing identity.
    pub identity: Identity,
    /// The app data the announce carried (a host binds its own peer id here).
    pub app_data: Vec<u8>,
    /// The interface on which this observation arrived.
    pub interface: InterfaceId,
    /// Hop count carried by this observation.
    pub hops: u8,
    /// Transport node named by a header-type-2 observation, when present.
    pub transport: Option<AddressHash>,
    /// Endpoint-local monotonic observation order.
    pub sequence: u64,
}

/// Compatibility name for consumers that already treat an announce as a peer record.
pub type PeerAnnounce = AnnounceFact;

/// A current learned route captured at one caller-supplied instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteFact {
    pub destination: AddressHash,
    pub interface: InterfaceId,
    pub transport: Option<AddressHash>,
    pub hops: u8,
    pub age: Duration,
}

/// Which side initiated a live link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkDirection {
    Inbound,
    Outbound,
}

/// The endpoint discipline currently driving a live link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkFactKind {
    BestEffort,
    Reliable,
    Resource,
}

/// Caps on live inbound links, so a flood of requests cannot make the endpoint spawn tasks
/// and buffers without bound. At a cap, a new request displaces the oldest link that has
/// not yet activated (no packet from the initiator has decrypted on it, so it never answered
/// our proof), and is refused only when every counted link has. Slots free when a link
/// closes or its stream is dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboundLinkLimits {
    /// Live inbound links across every destination this endpoint serves.
    pub total: usize,
    /// Live inbound links to any one destination.
    pub per_destination: usize,
}

impl Default for InboundLinkLimits {
    fn default() -> Self {
        Self {
            total: 256,
            per_destination: 64,
        }
    }
}

/// Live inbound links, counted against [`InboundLinkLimits`], and the per-destination
/// backlog of links queued for `accept`.
#[derive(Default)]
struct InboundLinks {
    limits: InboundLinkLimits,
    /// Link id to its slot. Entries leave with the link (`remove_link`).
    slots: HashMap<AddressHash, InboundSlot>,
    /// Admission order, so eviction can find the oldest.
    next_order: u64,
    /// Links handed to an accept queue and not yet taken, per destination.
    backlog: HashMap<AddressHash, usize>,
}

struct InboundSlot {
    destination: AddressHash,
    order: u64,
    /// Whether a packet from the initiator has decrypted on this link.
    active: bool,
}

/// What to do with a new inbound link request.
enum Admission {
    Admit,
    /// Admit after evicting this link, which never activated.
    Evict(AddressHash),
    Refuse,
}

impl InboundLinks {
    fn admission(&self, destination: AddressHash) -> Admission {
        if self.backlog.get(&destination).copied().unwrap_or(0) >= ACCEPT_BACKLOG {
            return Admission::Refuse;
        }
        let here = |slot: &InboundSlot| slot.destination == destination;
        let count = self.slots.values().filter(|slot| here(slot)).count();
        let destination_full = count >= self.limits.per_destination;
        if !destination_full && self.slots.len() < self.limits.total {
            return Admission::Admit;
        }
        let oldest_waiting = |same: bool| {
            self.slots
                .iter()
                .filter(|(_, slot)| !slot.active && (!same || here(slot)))
                .min_by_key(|(_, slot)| slot.order)
                .map(|(id, slot)| (*id, here(slot)))
        };
        // Prefer a stale request to the same destination, so a flood displaces its own.
        let victim =
            oldest_waiting(true).or_else(|| oldest_waiting(false).filter(|_| !destination_full));
        match victim {
            Some((id, same))
                if count - usize::from(same) < self.limits.per_destination
                    && self.slots.len() - 1 < self.limits.total =>
            {
                Admission::Evict(id)
            }
            _ => Admission::Refuse,
        }
    }

    fn admit(&mut self, id: AddressHash, destination: AddressHash) {
        self.next_order += 1;
        self.slots.insert(
            id,
            InboundSlot {
                destination,
                order: self.next_order,
                active: false,
            },
        );
    }

    fn queued(&mut self, destination: AddressHash) {
        *self.backlog.entry(destination).or_default() += 1;
    }

    fn taken(&mut self, destination: AddressHash) {
        if let Some(n) = self.backlog.get_mut(&destination) {
            *n -= 1;
            if *n == 0 {
                self.backlog.remove(&destination);
            }
        }
    }
}

/// Remote facts authenticated by link setup or a later IDENTIFY.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LinkRemoteFact {
    /// The remote application destination, known for an outbound link request.
    pub destination: Option<AddressHash>,
    /// The remote public identity, known outbound and after a valid inbound IDENTIFY.
    pub identity: Option<Identity>,
}

/// A read-only live-link observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkFact {
    pub id: AddressHash,
    pub interface: InterfaceId,
    pub kind: LinkFactKind,
    pub direction: LinkDirection,
    pub remote: LinkRemoteFact,
}

/// Route and link facts captured against one stable interface-id set.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointFacts {
    /// Endpoint topology revision stable across this fact capture.
    pub generation: u64,
    pub interfaces: Vec<InterfaceId>,
    pub routes: Vec<RouteFact>,
    pub links: Vec<LinkFact>,
    /// Routes omitted because their captured age reached the route lifetime.
    pub expired_routes: u64,
}

/// One authenticated link-less asymmetric packet received by a registered destination.
#[derive(Clone, Debug)]
pub struct ReceivedSingle {
    pub destination: AddressHash,
    pub interface: InterfaceId,
    pub data: Vec<u8>,
    /// The retained receive ratchet that authenticated it. `None` means the destination was
    /// registered without ratchets and the long-term identity key authenticated the token.
    pub ratchet_id: Option<NameHash>,
}

/// Evidence that a link-less packet was encrypted and accepted by local interface queues.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinglePacketReceipt {
    pub destination: AddressHash,
    pub ratchet_id: NameHash,
    /// One for a learned route; possibly several when an expired route requires broadcast.
    pub queued_interfaces: usize,
}

/// An accepted inbound link and the destination it arrived on.
pub struct Accepted {
    /// The stream carrying the link.
    pub stream: LinkStream,
    /// The destination hash the link request targeted (an ALPN maps to one).
    pub destination: AddressHash,
    /// The interface the link request arrived on.
    ///
    /// A transport fact, not a claim: it is the interface the router actually
    /// received the packet on, so a policy layer above can distinguish a peer
    /// reaching a service over the local mesh from one arriving over TCP.
    pub interface: InterfaceId,
}

/// An accepted resource link and the destination it arrived on.
pub struct AcceptedResource {
    /// The session that publishes or fetches one resource over the link.
    pub session: ResourceSession,
    /// The destination hash the link request targeted.
    pub destination: AddressHash,
    /// The interface the link request arrived on.
    pub interface: InterfaceId,
}

/// A live link and the channel that feeds its stream inbound bytes.
/// Identifies one attached interface (one TCP connection).
pub type InterfaceId = u32;

/// A raw packet interface: the seam every transport plugs into.
///
/// The endpoint sends outbound [`Packet`]s to it (drain [`next_outbound`]) and
/// receives inbound packets from it (via its [`InterfaceSink`]). Nothing here does
/// I/O or framing — the caller owns how bytes move. TCP's interface is exactly this
/// seam plus HDLC framing over a socket; a serial line, or a test loss-oracle that
/// drops/delays/reorders packets, is the same seam with a different pump.
///
/// [`next_outbound`]: Interface::next_outbound
pub struct Interface {
    id: InterfaceId,
    outbound: OutboundPackets,
    router_tx: mpsc::Sender<(InterfaceId, Packet)>,
    frame_limit: Arc<AtomicUsize>,
    ifac: Option<Ifac>,
    /// Inbound packets dropped because the router's queue was full. Shared with every
    /// [`InterfaceSink`] split off this interface.
    dropped: Arc<AtomicU64>,
}

/// Which interfaces a routing rule applies to.
///
/// Transit is directional: an endpoint may accept forwarded traffic from one interface and
/// emit it on another without the reverse being true. A node bridging a public radio to a
/// private wired segment, for instance, can carry the radio's traffic outward while refusing
/// to inject anything from the wire back onto the air.
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
/// Transit is not one switch: a node may relay announces so its neighbours stay discoverable
/// while refusing to carry their data, may accept transit from one interface only, or may cap
/// how far it will propagate traffic. Every axis is independent, and the default
/// ([`RoutingPolicy::none`]) carries nothing — an endpoint moves its own traffic until its
/// owner opts in.
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
    /// Hop ceiling for forwarded traffic. A packet at or above this is dropped rather than
    /// relayed, bounding how far this node will carry anything.
    pub max_hops: u8,
    /// Each class's share of a contended interface. This is where transit is bounded against
    /// local traffic, so it lives with the transit policy even though it governs both.
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
    fn accepts_transit_from(&self, iface: InterfaceId) -> bool {
        self.forward_packets && self.allowed_ingress.allows(iface)
    }

    /// Whether an announce arriving on `iface` may be re-broadcast under this policy.
    fn relays_announce_from(&self, iface: InterfaceId) -> bool {
        self.forward_announces && self.allowed_ingress.allows(iface)
    }
}

/// A per-class tally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClassCounters {
    pub control: u64,
    pub interactive: u64,
    pub background: u64,
    pub transit: u64,
}

impl ClassCounters {
    fn from_array(a: [u64; TrafficClass::COUNT]) -> Self {
        Self {
            control: a[TrafficClass::Control.index()],
            interactive: a[TrafficClass::Interactive.index()],
            background: a[TrafficClass::Background.index()],
            transit: a[TrafficClass::Transit.index()],
        }
    }

    fn add(&mut self, other: Self) {
        self.control += other.control;
        self.interactive += other.interactive;
        self.background += other.background;
        self.transit += other.transit;
    }
}

/// What the outbound schedule has done, summed over every interface.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct QueueCounters {
    /// Packets the schedule released to the wire, by class.
    pub sent: ClassCounters,
    /// Packets dropped because their class's queue was full, by class. A non-zero transit
    /// count on an otherwise healthy node is the expected sign of a neighbour offering more
    /// than this node agreed to carry.
    pub dropped: ClassCounters,
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
    /// Packets dropped for carrying the IFAC flag into an interface without IFAC. An IFAC
    /// interface strips the flag when it verifies a frame, so any flag that reaches the
    /// router came off a plain interface, where RNS drops it too.
    pub ifac_flag_rejected: u64,
    /// Packets RNS's packet filter drops: header-type-2 packets for another transport, PLAIN
    /// or GROUP packets past their first hop, repeated transit or single packets, and tagless
    /// or repeated path requests.
    pub filtered_packets: u64,
    /// Announces turned away by a full address book, and therefore not routed, relayed, or
    /// published either. Climbing means the book is at capacity, which is worth knowing:
    /// past that point this endpoint is deaf to peers it has not already met.
    pub refused_announces: u64,
    /// Verified unknown-route announces retained during an ingress interface burst.
    pub held_announces: u64,
    /// Verified announces dropped because the bounded ingress hold queue was full.
    pub held_announces_dropped: u64,
    /// Valid announces learned locally but not relayed because their destination was rate
    /// blocked on this incoming interface.
    pub relay_rate_limited_announces: u64,
    /// Routes dropped to make room in a full path table. Climbing means this endpoint knows
    /// more destinations than it can hold, and is forgetting the quietest to keep the rest.
    pub paths_evicted: u64,
    /// Announces rejected because their exact freshness blob is in this destination's live
    /// route history. Packet-loop de-duplication is deliberately separate and runs later.
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
}

/// The live counter cells behind [`RoutingCounters`].
#[derive(Debug, Default)]
struct RoutingStats {
    forwarded_packets: AtomicU64,
    forwarded_announces: AtomicU64,
    policy_rejected: AtomicU64,
    hop_limit_dropped: AtomicU64,
    ifac_flag_rejected: AtomicU64,
    filtered_packets: AtomicU64,
    refused_announces: AtomicU64,
    held_announces: AtomicU64,
    held_announces_dropped: AtomicU64,
    relay_rate_limited_announces: AtomicU64,
    paths_evicted: AtomicU64,
    freshness_replays_rejected: AtomicU64,
    freshness_stale_rejected: AtomicU64,
    freshness_rows_evicted: AtomicU64,
    freshness_blobs_evicted: AtomicU64,
    own_echo_dropped: AtomicU64,
    duplicate_dropped: AtomicU64,
    inbound_links_refused: AtomicU64,
    inbound_links_evicted: AtomicU64,
    link_queue_dropped: AtomicU64,
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
        }
    }
}

impl Interface {
    /// This interface's id.
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// Maximum complete Reticulum packet this interface currently admits.
    ///
    /// Raw interface owners can set an initial cap through
    /// [`Endpoint::attach_interface_with_frame_limit`]. Tulle also constrains
    /// this value synchronously when its driver is constructed.
    pub fn frame_limit(&self) -> usize {
        self.frame_limit.load(Ordering::Acquire)
    }

    /// Lower this interface's admission limit to a carrier-discovered cap.
    ///
    /// This is monotonic and should be called before the endpoint can queue
    /// traffic. Prefer [`Endpoint::attach_interface_with_frame_limit`] when the
    /// limit is already known.
    pub fn constrain_frame_limit(&self, max_frame_len: usize) {
        self.frame_limit.fetch_min(max_frame_len, Ordering::AcqRel);
    }

    /// The next packet the endpoint wants to send out this interface, chosen by the
    /// per-class schedule. `None` once the endpoint is dropped.
    pub async fn next_outbound(&mut self) -> Option<Packet> {
        self.outbound.recv().await
    }

    /// A cloneable handle for delivering packets received on this interface into
    /// the endpoint's router.
    pub fn sink(&self) -> InterfaceSink {
        InterfaceSink {
            id: self.id,
            router_tx: self.router_tx.clone(),
            ifac: self.ifac.clone(),
            dropped: self.dropped.clone(),
        }
    }

    /// Split into the outbound packet stream and an inbound [`InterfaceSink`], the
    /// usual shape for a bidirectional pump.
    pub fn split(self) -> (OutboundPackets, InterfaceSink) {
        let sink = InterfaceSink {
            id: self.id,
            router_tx: self.router_tx,
            ifac: self.ifac,
            dropped: self.dropped,
        };
        (self.outbound, sink)
    }
}

/// Delivers packets received on an [`Interface`] into the endpoint's router,
/// tagged with the interface they arrived on.
#[derive(Clone)]
pub struct InterfaceSink {
    id: InterfaceId,
    router_tx: mpsc::Sender<(InterfaceId, Packet)>,
    ifac: Option<Ifac>,
    /// Packets dropped because the router's queue was full, shared across clones so the
    /// figure describes the interface rather than one handle to it.
    dropped: Arc<AtomicU64>,
}

impl InterfaceSink {
    /// Deliver a received packet into the router.
    ///
    /// Returns whether the endpoint is **still there**, not whether the packet was queued,
    /// and the difference is the whole point. `try_send` fails both when the router's
    /// bounded queue is momentarily full and when the endpoint has been dropped. Collapsing
    /// those into one `false` made every caller treat a burst as a dead endpoint, so a
    /// thousand packets arriving faster than the router drained them detached a working
    /// radio permanently, with nothing to bring it back but a restart.
    ///
    /// A full queue is backpressure, and dropping is the correct response: Reticulum is a
    /// datagram network whose upper layers already retransmit, so a lost packet costs a
    /// retry while a lost interface costs the carrier. The drop is counted rather than
    /// silent; see [`Self::dropped`].
    pub fn deliver(&self, pkt: Packet) -> bool {
        match self.router_tx.try_send((self.id, pkt)) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                true
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        }
    }

    /// Packets this interface dropped because the router could not keep up.
    ///
    /// Nonzero means the endpoint is being offered more than it can route, which is a
    /// capacity fact worth surfacing: it is invisible from the wire and indistinguishable,
    /// from the outside, from a peer that never transmitted.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Authenticate and decode one complete carrier frame, then deliver it.
    ///
    /// An IFAC-configured interface rejects open, incorrectly keyed, and
    /// modified frames before they reach the endpoint router. An interface
    /// without IFAC hands a frame carrying the IFAC flag on, and the router
    /// drops it and counts it in [`RoutingCounters::ifac_flag_rejected`].
    pub fn deliver_frame(&self, frame: &[u8]) -> crate::Result<bool> {
        let packet = match &self.ifac {
            Some(ifac) => Packet::decode(&ifac.open(frame)?)?,
            None => Packet::decode(frame)?,
        };
        Ok(self.deliver(packet))
    }
}

/// What a packet is for, which decides how it shares a busy interface.
///
/// A radio is slow enough that packets genuinely queue in the endpoint, so the order they
/// leave in is a policy choice rather than an accident of arrival. Classifying at the send
/// site is deliberate: the bytes cannot say whether they are someone's chat message or a bulk
/// sync, but the code putting them on the wire knows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TrafficClass {
    /// Protocol upkeep: announces, path responses, link setup, proofs. Small and infrequent;
    /// starving it costs the network its ability to repair itself, so it is served first.
    Control,
    /// Local traffic someone is waiting on.
    Interactive,
    /// Local traffic that can wait: bulk transfer, replication.
    Background,
    /// Someone else's traffic, carried as a courtesy. Served last and capped, so a busy
    /// public mesh cannot consume the capacity its host reserved for itself.
    Transit,
}

impl TrafficClass {
    const COUNT: usize = 4;

    const fn index(self) -> usize {
        match self {
            Self::Control => 0,
            Self::Interactive => 1,
            Self::Background => 2,
            Self::Transit => 3,
        }
    }

    const ALL: [Self; Self::COUNT] = [
        Self::Control,
        Self::Interactive,
        Self::Background,
        Self::Transit,
    ];
}

/// Each class's share of a busy interface, as a deficit-round-robin quantum multiplier.
///
/// These are *relative shares of a contended interface*, not rate limits: an idle interface
/// sends whatever it has. They only bind when more traffic is offered than the medium can
/// carry, which is exactly when a host's own traffic must not lose to transit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueWeights {
    pub control: u32,
    pub interactive: u32,
    pub background: u32,
    pub transit: u32,
}

impl Default for QueueWeights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl QueueWeights {
    /// Control first, then interactive, background, and transit last. The ratios matter more
    /// than the values: transit gets a share, never priority.
    pub const DEFAULT: Self = Self {
        control: 8,
        interactive: 4,
        background: 2,
        transit: 1,
    };

    fn for_class(&self, class: TrafficClass) -> u32 {
        match class {
            TrafficClass::Control => self.control,
            TrafficClass::Interactive => self.interactive,
            TrafficClass::Background => self.background,
            TrafficClass::Transit => self.transit,
        }
    }
}

/// How many packets each class may hold on one interface before its next packet is dropped.
///
/// Bounded on purpose: an unbounded queue in front of a slow radio converts memory into
/// latency and hides the loss instead of reporting it. Transit is held shallowest, so a
/// flooding neighbour's backlog cannot grow without limit inside a node that is doing it a
/// favour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QueueDepths {
    pub control: usize,
    pub interactive: usize,
    pub background: usize,
    pub transit: usize,
}

impl Default for QueueDepths {
    fn default() -> Self {
        Self::DEFAULT
    }
}

impl QueueDepths {
    /// Transit is held shallowest: a neighbour's backlog is their problem to retry, not this
    /// node's to store.
    pub const DEFAULT: Self = Self {
        control: 64,
        interactive: 256,
        background: 256,
        transit: 64,
    };

    fn for_class(&self, class: TrafficClass) -> usize {
        match class {
            TrafficClass::Control => self.control,
            TrafficClass::Interactive => self.interactive,
            TrafficClass::Background => self.background,
            TrafficClass::Transit => self.transit,
        }
    }
}

/// The scheduling half of an interface's outbound path: per-class bounded queues drained by
/// deficit round robin.
///
/// The quantum is in bytes, so the shares are of *airtime* rather than packet count — on a
/// LoRa link a 500-byte packet costs far more than a 20-byte one, and counting packets would
/// let a class of large frames quietly take more than its share.
const QUANTUM_UNIT: u64 = 128;

#[derive(Default)]
struct QueueState {
    queues: [VecDeque<Packet>; TrafficClass::COUNT],
    deficit: [u64; TrafficClass::COUNT],
    dropped: [u64; TrafficClass::COUNT],
    sent: [u64; TrafficClass::COUNT],
    cursor: usize,
    /// Whether the class at `cursor` has already been credited its quantum for this visit.
    /// Without this the cursor is re-credited on every `pop`, and a class with a standing
    /// backlog never has to yield — which starves everything below it.
    credited: bool,
    /// Packets handed to the interface pump whose delivery has not completed yet.
    in_flight: usize,
    closed: bool,
}

/// One interface's outbound scheduler: bounded per-class queues plus the parking spot for
/// whoever is draining them.
struct OutboundQueues {
    state: Mutex<QueueState>,
    ready: tokio::sync::Notify,
    weights: Mutex<(QueueWeights, QueueDepths)>,
}

impl OutboundQueues {
    fn new(weights: QueueWeights, depths: QueueDepths) -> Self {
        Self {
            state: Mutex::new(QueueState::default()),
            ready: tokio::sync::Notify::new(),
            weights: Mutex::new((weights, depths)),
        }
    }

    /// Queue a packet in its class. Returns `false` if that class is full and the packet was
    /// dropped, which is reported rather than hidden.
    fn push(&self, pkt: Packet, class: TrafficClass) -> bool {
        let depth = self.weights.lock().unwrap().1.for_class(class);
        let mut state = self.state.lock().unwrap();
        if state.closed {
            return false;
        }
        let i = class.index();
        if state.queues[i].len() >= depth {
            state.dropped[i] += 1;
            return false;
        }
        state.queues[i].push_back(pkt);
        drop(state);
        self.ready.notify_one();
        true
    }

    /// Whether every class is empty and the interface pump has completed the packet it
    /// most recently took.
    fn is_drained(&self) -> bool {
        let state = self.state.lock().unwrap();
        state.in_flight == 0 && state.queues.iter().all(VecDeque::is_empty)
    }

    /// Take the next packet the schedule allows, or `None` if every queue is empty.
    fn pop(&self) -> Option<Packet> {
        let weights = self.weights.lock().unwrap().0;
        let mut state = self.state.lock().unwrap();
        if state.queues.iter().all(VecDeque::is_empty) {
            return None;
        }
        // Deficit round robin. A class is credited its quantum once per *visit*, not once per
        // call: it then spends that credit over consecutive calls until its head packet costs
        // more than it has banked, at which point the cursor moves on. A class that empties
        // forfeits its credit, so it cannot bank capacity while idle and burst later.
        //
        // Progress is guaranteed: every full cycle credits each non-empty class at least
        // QUANTUM_UNIT (weights are clamped to 1 below), and a packet costs at most the MTU,
        // so a class becomes affordable within a few cycles.
        for _ in 0..(TrafficClass::COUNT * 8) {
            let i = state.cursor;
            let class = TrafficClass::ALL[i];
            if state.queues[i].is_empty() {
                state.deficit[i] = 0;
                state.credited = false;
                state.cursor = (i + 1) % TrafficClass::COUNT;
                continue;
            }
            if !state.credited {
                // Clamped to 1: a zero weight would credit nothing and spin forever.
                state.deficit[i] += u64::from(weights.for_class(class).max(1)) * QUANTUM_UNIT;
                state.credited = true;
            }
            let cost = state.queues[i]
                .front()
                .map_or(1, |p| p.encoded_len() as u64)
                .max(1);
            if state.deficit[i] >= cost {
                state.deficit[i] -= cost;
                let pkt = state.queues[i].pop_front();
                if state.queues[i].is_empty() {
                    state.deficit[i] = 0;
                    state.credited = false;
                    state.cursor = (i + 1) % TrafficClass::COUNT;
                }
                state.sent[i] += 1;
                state.in_flight += 1;
                return pkt;
            }
            state.credited = false;
            state.cursor = (i + 1) % TrafficClass::COUNT;
        }
        // Unreachable given the progress argument above, but a scheduler that returns None
        // with packets still queued would park the drain forever, so fall back to strict
        // order rather than risk a stall.
        for i in 0..TrafficClass::COUNT {
            if let Some(pkt) = state.queues[i].pop_front() {
                state.sent[i] += 1;
                state.in_flight += 1;
                return Some(pkt);
            }
        }
        None
    }

    fn delivery_complete(&self) {
        let mut state = self.state.lock().unwrap();
        debug_assert!(state.in_flight > 0);
        state.in_flight = state.in_flight.saturating_sub(1);
    }

    fn set_policy(&self, weights: QueueWeights, depths: QueueDepths) {
        *self.weights.lock().unwrap() = (weights, depths);
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
        self.ready.notify_waiters();
    }

    fn counters(&self) -> ([u64; TrafficClass::COUNT], [u64; TrafficClass::COUNT]) {
        let state = self.state.lock().unwrap();
        (state.sent, state.dropped)
    }

    fn depth(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.in_flight + state.queues.iter().map(VecDeque::len).sum::<usize>()
    }
}

/// The draining half of an interface's outbound path.
///
/// Shaped like the channel receiver it replaces — `recv().await` — so every pump keeps
/// working, but the order packets arrive in is now the schedule's rather than arrival's.
pub struct OutboundPackets {
    queues: Arc<OutboundQueues>,
    delivery_in_flight: bool,
    ifac: Option<Ifac>,
}

impl OutboundPackets {
    /// The next packet to put on the wire, or `None` once the endpoint is gone.
    pub async fn recv(&mut self) -> Option<Packet> {
        self.complete_delivery();
        loop {
            if let Some(pkt) = self.queues.pop() {
                self.delivery_in_flight = true;
                return Some(pkt);
            }
            if self.queues.state.lock().unwrap().closed {
                return None;
            }
            // Register before re-checking, so a push between the pop above and the wait here
            // cannot be missed.
            let notified = self.queues.ready.notified();
            if let Some(pkt) = self.queues.pop() {
                self.delivery_in_flight = true;
                return Some(pkt);
            }
            if self.queues.state.lock().unwrap().closed {
                return None;
            }
            notified.await;
        }
    }

    /// Encode one queued packet for this interface, applying IFAC when configured.
    pub fn encode(&self, packet: &Packet) -> crate::Result<Vec<u8>> {
        let logical = packet.encode();
        match &self.ifac {
            Some(ifac) => ifac.seal(&logical),
            None => Ok(logical),
        }
    }

    fn complete_delivery(&mut self) {
        if self.delivery_in_flight {
            self.queues.delivery_complete();
            self.delivery_in_flight = false;
        }
    }
}

impl Drop for OutboundPackets {
    fn drop(&mut self) {
        self.complete_delivery();
    }
}

/// An attached interface: the scheduler its writer task drains.
struct Iface {
    id: InterfaceId,
    outbound: Arc<OutboundQueues>,
    frame_limit: Arc<AtomicUsize>,
    wire_overhead: usize,
    /// Sets the lifetime of routes learned on this interface.
    mode: InterfaceMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum QueueAdmission {
    Queued,
    Full,
    FrameLimit { actual: usize, limit: usize },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct SingleQueueResult {
    queued: usize,
    frame_capable: bool,
    frame_limit_rejection: Option<(usize, usize)>,
}

impl SingleQueueResult {
    fn observe(&mut self, admission: QueueAdmission) {
        match admission {
            QueueAdmission::Queued => {
                self.queued += 1;
                self.frame_capable = true;
            }
            QueueAdmission::Full => self.frame_capable = true,
            QueueAdmission::FrameLimit { actual, limit } => {
                if self
                    .frame_limit_rejection
                    .is_none_or(|(_, recorded_limit)| limit > recorded_limit)
                {
                    self.frame_limit_rejection = Some((actual, limit));
                }
            }
        }
    }
}

impl Iface {
    fn push(&self, packet: Packet, class: TrafficClass) -> QueueAdmission {
        let actual = packet.encoded_len() + self.wire_overhead;
        let limit = self.frame_limit.load(Ordering::Acquire);
        if actual > limit {
            return QueueAdmission::FrameLimit { actual, limit };
        }
        if self.outbound.push(packet, class) {
            QueueAdmission::Queued
        } else {
            QueueAdmission::Full
        }
    }
}

struct LinkEntry {
    link: Link,
    /// How inbound traffic for this link is handled: best-effort delivers decrypted bytes
    /// straight to the stream; reliable hands raw channel and proof packets to a driver.
    kind: LinkKind,
    /// The interface this link's traffic goes out on. Recorded for routing (R7), where a
    /// forwarded link's return traffic must go back the way it came.
    iface: InterfaceId,
    direction: LinkDirection,
    remote: LinkRemoteFact,
    /// Keepalive, staleness and handshake timers, advanced by the link watchdog.
    liveness: Liveness,
    /// Set when the watchdog drops the link, so its stream reports a timeout, not an end.
    lost: Arc<AtomicBool>,
}

/// The delivery discipline of a link's stream, chosen when the stream is registered.
enum LinkKind {
    /// The router decrypts each data packet and forwards the plaintext (right for TCP,
    /// where the medium never drops).
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
    fn fact_kind(&self) -> LinkFactKind {
        match self {
            Self::BestEffort { .. } => LinkFactKind::BestEffort,
            Self::Reliable { .. } => LinkFactKind::Reliable,
            Self::Resource { .. } => LinkFactKind::Resource,
        }
    }
}

type Links = Arc<Mutex<HashMap<AddressHash, LinkEntry>>>;

/// A destination this endpoint accepts links on.
struct Registered {
    dest: AddressHash,
    kind: RegistrationKind,
    /// The name and app data this destination announced with, retained so a path request for
    /// it can be answered by re-announcing it as a path response.
    name: DestinationName,
    app_data: Vec<u8>,
    /// Receive ratchets supplied and persisted by the host. `Some` also means identity-key
    /// fallback is refused for single packets, preventing an advertised ratchet from being
    /// silently downgraded.
    ratchets: Option<RatchetStore>,
}

#[derive(Clone, Copy)]
enum RegistrationKind {
    BestEffort,
    Reliable,
    Resource,
}

/// A verified announce deferred by a noisy interface. It retains the ingress fact through
/// release so a later route cannot be accidentally attributed to a different bearer.
struct HeldAnnounce {
    interface: InterfaceId,
    packet: Packet,
    announce: Announce,
}

/// The freshness ledger and its host policy share one lock. Keeping this guard across address
/// admission, freshness commit, route replacement, observation publication, and relay
/// scheduling makes a held-release task indistinguishable from direct router ingress.
struct AnnounceFreshnessState {
    policy: AnnounceFreshnessPolicy,
    table: AnnounceFreshness,
}

impl AnnounceFreshnessState {
    fn new(policy: AnnounceFreshnessPolicy) -> Result<Self, AnnounceFreshnessConfigError> {
        Ok(Self {
            policy,
            table: AnnounceFreshness::new(policy.config())?,
        })
    }
}

/// Shared router state.
struct Shared {
    lifecycle: Mutex<Lifecycle>,
    closed_notify: tokio::sync::Notify,
    identity: PrivateIdentity,
    address_book: Mutex<AddressBook>,
    links: Links,
    registered: Mutex<Vec<Registered>>,
    /// Per-destination host announce ordinals. A destination needs its own strictly increasing
    /// timebase, because it is the destination's signed blob that receivers retain.
    announce_timebases: Mutex<HashMap<AddressHash, TimebaseGenerator>>,
    /// Every attached interface. Announces broadcast to all; link traffic targets one.
    interfaces: Mutex<Vec<Iface>>,
    /// The router's inbound channel: every interface's reader feeds `(interface, packet)`.
    router_tx: mpsc::Sender<(InterfaceId, Packet)>,
    /// Inbound accepted links (stream + destination), surfaced to `accept`.
    accepted_tx: mpsc::UnboundedSender<Accepted>,
    /// Inbound accepted reliable links, surfaced to `accept_reliable_on_any`. Registered
    /// eagerly (the peer identity is learned from the initiator's IDENTIFY, not needed up
    /// front).
    reliable_accepted_tx: mpsc::UnboundedSender<Accepted>,
    /// Inbound resource links, surfaced to `accept_resource`.
    resource_accepted_tx: mpsc::UnboundedSender<AcceptedResource>,
    /// Validated announces, surfaced to `announcements`.
    announce_tx: mpsc::UnboundedSender<PeerAnnounce>,
    /// Monotonic order assigned only after an announce passes validation and admission.
    announce_sequence: AtomicU64,
    /// Decrypted link-less single packets, surfaced to `accept_single`.
    single_tx: mpsc::UnboundedSender<ReceivedSingle>,
    /// Pending outbound links awaiting a proof, keyed by destination: the waiter to wake
    /// (with the interface the proof came in on), and the half-open link that verifies it.
    pending: Mutex<HashMap<AddressHash, oneshot::Sender<(Link, InterfaceId)>>>,
    pending_links: Mutex<HashMap<AddressHash, link::PendingLink>>,
    next_iface_id: AtomicU32,
    /// Whether this endpoint acts as a transport node (forwards announces and packets).
    routing: Mutex<RoutingPolicy>,
    /// What routing has actually done, for diagnostics and policy proof.
    routing_stats: RoutingStats,
    /// Revision of topology and observation facts consumed by host management projections.
    diagnostic_generation: AtomicU64,
    /// Serializes diagnostic fact mutation against a multi-table diagnostic capture.
    diagnostic_barrier: RwLock<()>,
    /// Upper bound, in milliseconds, of the random delay before relaying an announce. Zero
    /// (the default) relays immediately. See [`Endpoint::set_relay_jitter`].
    relay_jitter_ms: AtomicU64,
    /// First reliable-channel RTT estimate. Proofs adapt it after traffic starts.
    reliable_initial_rtt_ms: AtomicU64,
    /// Maximum unproved reliable frames allowed in flight on subsequently opened links.
    reliable_max_window: AtomicU32,
    /// Decoded output ceiling for subsequently opened reliable streams.
    reliable_decoded_frame_limit: AtomicUsize,
    /// Link-request retry interval for subsequently opened links.
    link_setup_retry_ms: AtomicU64,
    /// Per-interface first-hop airtime allowances, in milliseconds, for link setup deadlines.
    first_hop_airtime_ms: Mutex<HashMap<InterfaceId, u64>>,
    /// MTU requested and offered by subsequently established links.
    link_mtu: AtomicU32,
    /// Proofs for recently accepted link requests, keyed by link id. Replaying the same
    /// proof avoids creating a second stream when only the first proof was lost.
    inbound_link_proofs: Mutex<HashMap<AddressHash, (Packet, Instant)>>,
    /// Live inbound links counted against their caps, and the accept backlog.
    inbound: Mutex<InboundLinks>,
    /// Learned routes: destination → the interface to reach it and its hop count. Populated
    /// from announces.
    path_table: Mutex<HashMap<AddressHash, PathEntry>>,
    /// Recently-seen announce packet hashes, for de-duplication (a ring of the last
    /// [`SEEN_ANNOUNCES`]).
    seen_announces: Mutex<(HashSet<AddressHash>, VecDeque<AddressHash>)>,
    /// Our own link packets heard back, and the far end's heard twice. See [`LinkPacketMemory`].
    link_packets: Mutex<LinkPacketMemory>,
    /// Transit and single packets already seen, so a loop or a second copy is dropped.
    packet_filter: Mutex<HashList>,
    /// Path requests already seen, by target and tag, so each is answered once.
    path_request_tags: Mutex<HashList>,
    /// Bounded freshness admission. Its lock spans the complete announce-effect bundle.
    announce_freshness: Mutex<AnnounceFreshnessState>,
    /// Route expiry follows the host freshness policy without needing to acquire the freshness
    /// bundle lock during ordinary packet routing.
    route_ttl_ms: AtomicU64,
    /// The bounded interface and destination announce-admission state machines. Their clock
    /// is relative to this endpoint so the verdicts are deterministic under a supplied time.
    announce_admission: Mutex<AnnounceAdmission>,
    announce_admission_started: tokio::time::Instant,
    /// Verified unknown-route announces held until their ingress burst has subsided.
    held_announces: Mutex<VecDeque<HeldAnnounce>>,
    /// At most one release task runs for each interface, however many announces it is holding.
    held_release_tasks: Mutex<HashSet<InterfaceId>>,
    /// Wakes deferred-release tasks when a carrier is detached, so a removed interface never
    /// leaves a full burst penalty's worth of sleeping tasks behind.
    held_release_wake: tokio::sync::Notify,
    /// Last time a path request went out per destination, for the same reason: see
    /// [`PATH_REQUEST_MIN_INTERVAL`].
    path_request_budget: Mutex<HashMap<AddressHash, Instant>>,
    /// When the path requests in the current window went out, oldest first, for the global
    /// cap ([`PATH_REQUEST_GLOBAL_MAX`]). Never longer than the cap.
    path_request_stamps: Mutex<VecDeque<Instant>>,
    /// Links being forwarded through us (this node is a transport hop): a link id maps to the
    /// two interfaces it bridges, so a proof or link data arriving on one goes out the other.
    link_transport: Mutex<HashMap<AddressHash, LinkBridge>>,
    /// Abort handles for every task the endpoint spawned (the router, interface readers and
    /// writers, TCP listeners, and link relays). [`Endpoint`]'s drop aborts them all, which is
    /// what lets the router's `Arc<Shared>` — and thus `Shared` and every socket — be released
    /// rather than kept alive forever by the router<->`Shared` reference cycle.
    tasks: Mutex<Vec<tokio::task::AbortHandle>>,
    /// Tasks that *finish on their own* once the thing feeding them goes away,
    /// as opposed to the perpetual ones (router, interface readers/writers,
    /// listeners) that only ever stop by being aborted. These are the best-effort
    /// outbound relays and reliable channel drivers. [`Endpoint::shutdown`] awaits
    /// them before it stops anything, which lets written and proven replies reach
    /// the wire.
    drainable: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Caller-driven resource sessions must finish or be dropped before their close packet
    /// can be included in an orderly shutdown.
    active_resources: AtomicUsize,
    resource_notify: tokio::sync::Notify,
}

/// A learned route to a destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PathEntry {
    iface: InterfaceId,
    /// The transport node this destination is reached through, from the `transport` field of
    /// the header-type-2 announce that taught us the route. Per destination, because an
    /// interface can reach many destinations through different nodes: one radio hearing A
    /// via X and B via Y is the ordinary case, not an exotic one.
    transport: Option<AddressHash>,
    hops: u8,
    /// When this route was last (re)learned from an announce, or last carried traffic. Routes
    /// older than their lifetime ([`Self::lifetime`]) are treated as stale and evicted on lookup.
    learned: Instant,
    /// The learning interface's mode when the route was learned.
    mode: InterfaceMode,
}

impl PathEntry {
    /// How long this route lives, given the policy's full-mode route TTL.
    fn lifetime(&self, route_ttl: Duration) -> Duration {
        Duration::from_millis(self.mode.route_ttl(duration_ticks(route_ttl)))
    }

    fn live_at(&self, now: Instant, route_ttl: Duration) -> bool {
        now.saturating_duration_since(self.learned) < self.lifetime(route_ttl)
    }
}

impl Shared {
    /// Mutate diagnostic source state under the one required lock order: revision barrier,
    /// then the owned fact lock(s). The revision advances before the barrier is released.
    fn write_diagnostic<T>(&self, change: impl FnOnce() -> (T, bool)) -> T {
        let _barrier = self.diagnostic_barrier.write().unwrap();
        let (value, changed) = change();
        if changed {
            self.diagnostic_generation.fetch_add(1, Ordering::AcqRel);
        }
        value
    }

    /// Capture a value against one stable diagnostic revision. Writers take the barrier
    /// first, mutate their owned fact state, and advance the revision before releasing it.
    /// The repeated revision read is defensive and keeps the stamped-value contract explicit.
    fn capture_diagnostic<T>(&self, mut capture: impl FnMut() -> T) -> (u64, T) {
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

    fn remove_link(&self, id: AddressHash) {
        self.write_diagnostic(|| {
            let removed = self.links.lock().unwrap().remove(&id).is_some();
            ((), removed)
        });
        self.inbound.lock().unwrap().slots.remove(&id);
    }

    /// Mark an inbound link active once a packet on it decrypts under its keys: the
    /// initiator holds the keys, so it answered our proof (RNS activates on the RTT
    /// packet). Only a link still waiting for that pays the decrypt.
    fn note_inbound_traffic(&self, link: &Link, pkt: &Packet) {
        let waiting = self
            .inbound
            .lock()
            .unwrap()
            .slots
            .get(&pkt.destination)
            .is_some_and(|slot| !slot.active);
        // Decrypt with the lock released; the router is the only writer of `active`.
        if waiting && link.decrypt(pkt).is_ok() {
            let mut inbound = self.inbound.lock().unwrap();
            if let Some(slot) = inbound.slots.get_mut(&pkt.destination) {
                slot.active = true;
            }
        }
    }

    /// Drop an inbound link that never activated, to make room for a newer request: close
    /// it toward the initiator and forget its cached proof.
    fn evict_inbound(&self, id: AddressHash) {
        let entry = self
            .links
            .lock()
            .unwrap()
            .get(&id)
            .map(|e| (e.link.clone(), e.iface));
        if let Some((link, iface)) = entry {
            self.send_on(iface, link.close_packet(&next_iv()));
        }
        self.inbound_link_proofs.lock().unwrap().remove(&id);
        self.remove_link(id);
        self.routing_stats
            .inbound_links_evicted
            .fetch_add(1, Ordering::Relaxed);
    }

    /// Hand a raw link packet to its driver, counting it if the driver's queue is full.
    fn queue_link_packet(&self, packets: &mpsc::Sender<Packet>, pkt: Packet) {
        if let Err(mpsc::error::TrySendError::Full(_)) = packets.try_send(pkt) {
            self.routing_stats
                .link_queue_dropped
                .fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The monotonic millisecond clock link liveness runs on.
    fn link_clock_ms(&self) -> u64 {
        self.announce_admission_now_ms()
    }

    fn is_running(&self) -> bool {
        *self.lifecycle.lock().unwrap() == Lifecycle::Running
    }

    fn is_closed(&self) -> bool {
        *self.lifecycle.lock().unwrap() == Lifecycle::Closed
    }

    fn begin_quiesce(&self) -> Quiesce {
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

    fn mark_closed(&self) -> bool {
        let mut state = self.lifecycle.lock().unwrap();
        if *state == Lifecycle::Closed {
            false
        } else {
            *state = Lifecycle::Closed;
            true
        }
    }

    fn register_interface(&self, iface: Iface) -> bool {
        let state = self.lifecycle.lock().unwrap();
        if *state != Lifecycle::Running {
            return false;
        }
        let id = iface.id;
        self.write_diagnostic(|| {
            self.interfaces.lock().unwrap().push(iface);
            self.announce_admission
                .lock()
                .unwrap()
                .attach_interface(id, self.announce_admission_now_ms());
            (true, true)
        })
    }

    /// Forget an interface, closing its outbound queues.
    ///
    /// Attaching was one-way: a transport that connects, drops, and reconnects left its old
    /// record, its queues, and anything scheduling against them in place forever. Anything
    /// still holding the matching [`Interface`] will see its queues closed and stop, which
    /// is the intended way to end a carrier.
    fn forget_interface(&self, id: InterfaceId) {
        self.write_diagnostic(|| {
            let mut interfaces = self.interfaces.lock().unwrap();
            let removed = if let Some(index) = interfaces.iter().position(|iface| iface.id == id) {
                let iface = interfaces.swap_remove(index);
                iface.outbound.close();
                true
            } else {
                false
            };
            drop(interfaces);
            self.first_hop_airtime_ms.lock().unwrap().remove(&id);
            // Routes and bridges through a gone interface would send into nothing until they
            // expired. RNS culls them with the interface (`Transport.py` 880-881, 975-978);
            // a destination without a route is then reached by broadcast.
            let routes_removed = {
                let mut paths = self.path_table.lock().unwrap();
                let before = paths.len();
                paths.retain(|_, entry| entry.iface != id);
                paths.len() != before
            };
            self.link_transport
                .lock()
                .unwrap()
                .retain(|_, bridge| bridge.from != id && bridge.out != id);
            self.announce_admission.lock().unwrap().forget_interface(id);
            self.held_announces
                .lock()
                .unwrap()
                .retain(|announce| announce.interface != id);
            self.held_release_wake.notify_waiters();
            ((), removed || routes_removed)
        });
    }

    /// The mode of an attached interface; [`InterfaceMode::Full`] if it is not attached.
    fn interface_mode(&self, id: InterfaceId) -> InterfaceMode {
        self.interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|iface| iface.id == id)
            .map_or(InterfaceMode::Full, |iface| iface.mode)
    }

    /// Mark `dest`'s live route used now. RNS refreshes a path's timestamp whenever it
    /// carries a packet (`Transport.py` 1406, 1426, 2113), so a route in use does not lapse.
    fn touch_path(&self, dest: AddressHash) {
        let now = Instant::now();
        let route_ttl = self.route_ttl();
        self.write_diagnostic(|| {
            let mut paths = self.path_table.lock().unwrap();
            match paths.get_mut(&dest) {
                Some(entry) if entry.live_at(now, route_ttl) => {
                    entry.learned = now;
                    ((), true)
                }
                _ => ((), false),
            }
        });
    }

    /// Record a transit or single packet's hash; false for a copy already seen. Resource parts
    /// and keepalives legitimately repeat their hash and are always new, as RNS exempts them
    /// (`Transport.py` 1635-1640). Channel is filtered, unlike RNS: see N10.
    fn packet_is_new(&self, pkt: &Packet) -> bool {
        if !crate::node::is_deduplicated_link_context(pkt.context)
            || self.packet_filter.lock().unwrap().insert(pkt.hash())
        {
            return true;
        }
        self.routing_stats
            .filtered_packets
            .fetch_add(1, Ordering::Relaxed);
        false
    }

    fn announce_admission_now_ms(&self) -> u64 {
        self.announce_admission_started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
    }

    fn route_ttl(&self) -> Duration {
        Duration::from_millis(self.route_ttl_ms.load(Ordering::Relaxed))
    }

    fn hold_announce(&self, held: HeldAnnounce) -> bool {
        let capacity = self
            .announce_admission
            .lock()
            .unwrap()
            .policy()
            .held_capacity;
        let mut queue = self.held_announces.lock().unwrap();
        if let Some(existing) = queue.iter_mut().find(|existing| {
            existing.interface == held.interface
                && existing.announce.destination == held.announce.destination
        }) {
            *existing = held;
            return true;
        }
        if queue.len() >= capacity {
            return false;
        }
        queue.push_back(held);
        true
    }

    fn begin_resource(&self) -> bool {
        let state = self.lifecycle.lock().unwrap();
        if *state != Lifecycle::Running {
            return false;
        }
        self.active_resources.fetch_add(1, Ordering::AcqRel);
        true
    }

    fn end_resource(&self) {
        let previous = self.active_resources.fetch_sub(1, Ordering::AcqRel);
        debug_assert!(previous > 0);
        self.resource_notify.notify_waiters();
    }

    /// Send a packet out every interface (announces, path requests). These are our own
    /// protocol upkeep, so they ride the control class.
    fn broadcast(&self, pkt: Packet) {
        for i in self.interfaces.lock().unwrap().iter() {
            let _ = i.push(pkt.clone(), TrafficClass::Control);
        }
    }

    /// A fresh random delay in `0..=relay_jitter_ms`, or zero when jitter is off.
    fn relay_jitter(&self) -> Duration {
        let max = self.relay_jitter_ms.load(Ordering::Relaxed);
        if max == 0 {
            return Duration::ZERO;
        }
        let mut b = [0u8; 8];
        fill_random(&mut b);
        Duration::from_millis(u64::from_le_bytes(b) % (max + 1))
    }

    /// The queue weights currently configured (from the routing policy).
    fn queue_weights(&self) -> QueueWeights {
        self.routing.lock().unwrap().queue_weights
    }

    /// The queue depths currently configured (from the routing policy).
    fn queue_depths(&self) -> QueueDepths {
        self.routing.lock().unwrap().queue_depths
    }

    /// Relay a packet out every interface except the one it arrived on, restricted to those
    /// the egress selector permits. Returns how many interfaces it went out on, so a caller
    /// can tell a real relay from a policy that permitted nothing.
    fn broadcast_transit(
        &self,
        except: InterfaceId,
        pkt: Packet,
        egress: &InterfaceSelector,
    ) -> usize {
        let mut sent = 0;
        for i in self.interfaces.lock().unwrap().iter() {
            // Relayed announces are someone else's upkeep: useful, but not at the expense of
            // this node's own traffic, so they queue as transit.
            if i.id != except
                && egress.allows(i.id)
                && matches!(
                    i.push(pkt.clone(), TrafficClass::Transit),
                    QueueAdmission::Queued
                )
            {
                sent += 1;
            }
        }
        sent
    }

    /// The largest packet an attached interface carries: its frame limit less its access code.
    fn link_mtu_on(&self, iface: InterfaceId) -> Option<u32> {
        self.interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|i| i.id == iface)
            .map(|i| {
                let limit = i.frame_limit.load(Ordering::Acquire);
                u32::try_from(limit.saturating_sub(i.wire_overhead)).unwrap_or(u32::MAX)
            })
    }

    /// Send a packet out one interface, addressed through that interface's transport node if
    /// it has one (header-type-2 `[transport][dest]`), so a transport node forwards it.
    fn send_on(&self, iface: InterfaceId, pkt: Packet) {
        self.send_on_class(iface, pkt, TrafficClass::Interactive);
    }

    /// Send a packet out one interface in a chosen class. Local link traffic defaults to
    /// interactive; setup, proofs, and keepalives are control; carried traffic is transit.
    fn send_on_class(&self, iface: InterfaceId, pkt: Packet, class: TrafficClass) {
        let _ = self.try_send_on_class(iface, pkt, class);
    }

    fn try_send_on_class(&self, iface: InterfaceId, pkt: Packet, class: TrafficClass) -> bool {
        // Carried traffic is someone else's, and its copies coming back are theirs to judge.
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

    fn queue_single_on(&self, iface: InterfaceId, pkt: Packet) -> QueueAdmission {
        let addressed = self.address_for(iface, pkt);
        self.interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|candidate| candidate.id == iface)
            .map_or(QueueAdmission::Full, |candidate| {
                candidate.push(addressed, TrafficClass::Interactive)
            })
    }

    /// Queue a local single packet on its learned route, or broadcast when the cached route
    /// has expired. Records whether a candidate queue could carry the complete encoded frame,
    /// so the caller can distinguish carrier refusal from temporary queue pressure.
    fn queue_single(&self, dest: AddressHash, pkt: Packet) -> SingleQueueResult {
        let mut result = SingleQueueResult::default();
        if let Some(iface) = self.path_iface(dest) {
            result.observe(self.queue_single_on(iface, pkt));
            return result;
        }
        let interfaces: Vec<_> = self
            .interfaces
            .lock()
            .unwrap()
            .iter()
            .map(|interface| interface.id)
            .collect();
        for interface in interfaces {
            result.observe(self.queue_single_on(interface, pkt.clone()));
        }
        result
    }

    /// Wrap a packet for the interface it will go out on: if that interface reaches a
    /// transport node, make it header-type-2 with the node's id in the transport field so the
    /// node forwards it toward `destination`. A directly-connected interface leaves it as is.
    fn address_for(&self, iface: InterfaceId, mut pkt: Packet) -> Packet {
        // Looked up by destination, not by interface. Keyed by interface, a second
        // destination learned through a different transport node overwrote the first, and
        // every packet for the first was then addressed to the wrong node: announce A via X
        // and B via Y on one radio, and A silently routes through Y.
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

    /// Build a path response for `target` if it is one of our registered destinations: an
    /// announce for it carrying context [`crate::path::CTX_PATH_RESPONSE`]. Returns `None` if
    /// we do not own `target` — we hold no announce cache, so we cannot answer for others and
    /// stay silent rather than guess.
    fn path_response(&self, target: AddressHash) -> Option<Packet> {
        self.path_response_at(target, host_announce_seconds())
    }

    /// The deterministic half of [`Self::path_response`]. Keeping the clock source at this
    /// seam lets the production path and its boundary cases share the same blob minting rule.
    fn path_response_at(&self, target: AddressHash, source_seconds: u64) -> Option<Packet> {
        let reg = self.registered.lock().unwrap();
        let r = reg.iter().find(|r| r.dest == target)?;
        let ratchet = r.ratchets.as_ref().and_then(RatchetStore::current_public);
        let mut pkt =
            self.build_announce_at(&r.name, ratchet.as_ref(), &r.app_data, source_seconds);
        pkt.context = crate::path::CTX_PATH_RESPONSE;
        Some(pkt)
    }

    /// Build one locally owned announce from a typed blob. The generator is keyed by the
    /// derived destination rather than the endpoint identity, so two registered names do not
    /// consume one another's ordinal space.
    fn build_announce_at(
        &self,
        name: &DestinationName,
        ratchet: Option<&[u8; crate::announce::RATCHET_LEN]>,
        app_data: &[u8],
        source_seconds: u64,
    ) -> Packet {
        let destination = name.destination_hash(self.identity.public());
        let blob = self.next_announce_blob(destination, source_seconds);
        announce::build(&self.identity, name.name_hash(), &blob, ratchet, app_data)
    }

    fn next_announce_blob(&self, destination: AddressHash, source_seconds: u64) -> AnnounceBlob {
        let ordinal = self
            .announce_timebases
            .lock()
            .unwrap()
            .entry(destination)
            .or_insert_with(|| {
                TimebaseGenerator::host(0)
                    .expect("the host announce timebase starts within its wire range")
            })
            .next(source_seconds)
            .expect("host announce timebase is representable and not exhausted");
        let mut nonce = [0_u8; ANNOUNCE_NONCE_LEN];
        fill_random(&mut nonce);
        AnnounceBlob::mint(nonce, ordinal)
            .expect("TimebaseGenerator only returns timebases representable on the announce wire")
    }

    /// Whether `dest` has an unexpired route, without evicting anything.
    fn has_live_route(&self, dest: AddressHash) -> bool {
        let route_ttl = self.route_ttl();
        self.path_table
            .lock()
            .unwrap()
            .get(&dest)
            .is_some_and(|entry| entry.live_at(Instant::now(), route_ttl))
    }

    /// Drop `dest`'s route, if any.
    fn forget_path(&self, dest: AddressHash) {
        self.write_diagnostic(|| ((), self.path_table.lock().unwrap().remove(&dest).is_some()));
    }

    /// Record that `dest` is reachable via `iface` at `hops`.
    ///
    /// Freshness admission has already admitted this announce. Its route is therefore the
    /// incumbent, irrespective of whether its hop count is better, equal, or worse than a
    /// formerly live route. Selecting the shortest live route here would let an
    /// older announce override the newer route decision made by the freshness ledger.
    fn learn_path(
        &self,
        dest: AddressHash,
        iface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
    ) {
        self.learn_path_at(dest, iface, hops, transport, Instant::now());
    }

    /// As [`Self::learn_path`], at a supplied monotonic instant. This keeps route-capacity and
    /// expiry tests independent of scheduler timing.
    fn learn_path_at(
        &self,
        dest: AddressHash,
        iface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
        now: Instant,
    ) {
        let mode = self.interface_mode(iface);
        self.write_diagnostic(|| {
            let mut t = self.path_table.lock().unwrap();
            let route_ttl = self.route_ttl();
            if t.len() >= PATH_TABLE_CAPACITY && !t.contains_key(&dest) {
                // The dead first: a table full of expired routes must never evict a live one.
                t.retain(|_, e| e.live_at(now, route_ttl));
                // Still full means every route is live, so the quietest peer loses. Its
                // `learned` is oldest precisely because it has stopped re-announcing.
                if t.len() >= PATH_TABLE_CAPACITY
                    && let Some(stalest) = t
                        .iter()
                        .min_by_key(|(_, e)| e.learned)
                        .map(|(dest, _)| *dest)
                {
                    t.remove(&stalest);
                    self.routing_stats
                        .paths_evicted
                        .fetch_add(1, Ordering::Relaxed);
                }
            }
            let next = PathEntry {
                iface,
                transport,
                hops,
                learned: now,
                mode,
            };
            let changed = t.get(&dest) != Some(&next);
            t.insert(dest, next);
            ((), changed)
        });
    }

    /// Destinations the address book must keep while it is full: those with an unexpired path
    /// or a live link. Each table is locked in turn, never two at once.
    fn destinations_in_use(&self) -> HashSet<AddressHash> {
        let route_ttl = self.route_ttl();
        let now = Instant::now();
        let mut in_use: HashSet<AddressHash> = self
            .path_table
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, entry)| entry.live_at(now, route_ttl))
            .map(|(destination, _)| *destination)
            .collect();
        in_use.extend(
            self.links
                .lock()
                .unwrap()
                .values()
                .filter_map(|entry| entry.remote.destination),
        );
        in_use
    }

    /// The interface to reach `dest`, if a route is known and unexpired. Evicts an expired
    /// route as a side effect, so a stale path never lingers past a lookup.
    fn path_iface(&self, dest: AddressHash) -> Option<InterfaceId> {
        let route_ttl = self.route_ttl();
        self.write_diagnostic(|| {
            let mut t = self.path_table.lock().unwrap();
            match t.get(&dest) {
                Some(e) if e.live_at(Instant::now(), route_ttl) => (Some(e.iface), false),
                Some(_) => {
                    t.remove(&dest);
                    (None, true)
                }
                None => (None, false),
            }
        })
    }

    /// Whether a path request for `dest` may go out now. Same shape and same reasoning as
    /// announce-admission destination ledger on the outbound side, plus the global cap: the
    /// per-destination floor cannot be the whole answer, because the peer that provokes a
    /// path request also chooses the destination it names.
    ///
    /// Ordering is load-bearing: both checks pass before either records anything, so a
    /// request refused by the global cap does not burn the destination's own budget, and a
    /// per-destination repeat does not spend a slot in the global window.
    fn path_request_within_budget(&self, dest: AddressHash) -> bool {
        let mut budget = self.path_request_budget.lock().unwrap();
        let mut stamps = self.path_request_stamps.lock().unwrap();
        let now = Instant::now();
        if let Some(&last) = budget.get(&dest)
            && now.duration_since(last) < PATH_REQUEST_MIN_INTERVAL
        {
            return false;
        }
        while let Some(&oldest) = stamps.front()
            && now.duration_since(oldest) >= PATH_REQUEST_MIN_INTERVAL
        {
            stamps.pop_front();
        }
        if stamps.len() >= PATH_REQUEST_GLOBAL_MAX {
            return false;
        }
        if budget.len() > SEEN_ANNOUNCES {
            budget.retain(|_, t| now.duration_since(*t) < PATH_REQUEST_MIN_INTERVAL);
        }
        budget.insert(dest, now);
        stamps.push_back(now);
        true
    }

    /// Whether this announce (by packet hash) is new; records it if so.
    fn announce_is_new(&self, hash: AddressHash) -> bool {
        let mut g = self.seen_announces.lock().unwrap();
        if g.0.contains(&hash) {
            return false;
        }
        g.0.insert(hash);
        g.1.push_back(hash);
        if g.1.len() > SEEN_ANNOUNCES
            && let Some(old) = g.1.pop_front()
        {
            g.0.remove(&old);
        }
        true
    }
}

/// A Reticulum endpoint over any number of interfaces.
///
/// All methods take `&self` (the receivers are behind async mutexes), so an endpoint can be
/// wrapped in an `Arc` and shared: a host transport can call `open`/`announce` from one task
/// while another drives `accept`/`next_announcement`.
pub struct Endpoint {
    shared: Arc<Shared>,
    accepted_rx: AsyncMutex<mpsc::UnboundedReceiver<Accepted>>,
    reliable_accepted_rx: AsyncMutex<mpsc::UnboundedReceiver<Accepted>>,
    resource_accepted_rx: AsyncMutex<mpsc::UnboundedReceiver<AcceptedResource>>,
    announce_rx: AsyncMutex<mpsc::UnboundedReceiver<PeerAnnounce>>,
    single_rx: AsyncMutex<mpsc::UnboundedReceiver<ReceivedSingle>>,
}

fn endpoint_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "endpoint closed")
}

fn require_current_ratchet(ratchets: &RatchetStore) -> io::Result<()> {
    ratchets.current_public().map(|_| ()).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "ratchet store has no current epoch",
        )
    })
}

async fn recv_until_closed<T>(
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

    /// Create an endpoint and dial one TCP peer as its first interface.
    pub async fn connect(addr: SocketAddr, identity: PrivateIdentity) -> io::Result<Self> {
        let ep = Self::new(identity);
        ep.attach_tcp_client(addr).await?;
        Ok(ep)
    }

    /// Attach a connected TCP stream as an interface, and return its id.
    pub fn attach_stream(&self, stream: TcpStream) -> InterfaceId {
        attach(&self.shared, stream, None).0
    }

    /// Attach an IFAC-authenticated connected TCP stream.
    pub fn attach_stream_with_ifac(&self, stream: TcpStream, ifac: Ifac) -> InterfaceId {
        attach(&self.shared, stream, Some(ifac)).0
    }

    /// Attach a raw packet [`Interface`] and return its handle, doing no I/O or
    /// framing. The caller drives the transport: drain [`Interface::next_outbound`]
    /// to send packets, and call the [`InterfaceSink`] to deliver received ones.
    /// This is the seam a non-TCP medium (serial, or a deterministic test loss
    /// oracle) plugs into; `attach_tcp_client` / `listen_tcp` are this plus framing.
    pub fn attach_interface(&self) -> Interface {
        self.attach_interface_with_frame_limit(crate::packet::MTU)
            .expect("the Reticulum protocol MTU is a valid interface frame limit")
    }

    /// Attach a raw packet interface with an explicit complete-frame limit.
    ///
    /// The effective limit cannot exceed Reticulum's own protocol MTU. Interface
    /// drivers may lower it again if they discover a stricter carrier limit.
    /// Detach an interface, closing its queues and forgetting its record.
    ///
    /// The counterpart attaching never had. A transport that connects, drops, and reconnects
    /// -- an unreliable TCP peer, a radio replugged -- otherwise left its old record and
    /// queues behind on every cycle, and the scheduler kept visiting them.
    pub fn detach_interface(&self, id: InterfaceId) {
        self.shared.forget_interface(id);
    }

    /// Set an attached interface's mode, which bounds the lifetime of routes learned on it
    /// from now on (see [`InterfaceMode::route_ttl`]). False if no such interface is attached.
    pub fn set_interface_mode(&self, id: InterfaceId, mode: InterfaceMode) -> bool {
        self.shared
            .interfaces
            .lock()
            .unwrap()
            .iter_mut()
            .find(|iface| iface.id == id)
            .map(|iface| iface.mode = mode)
            .is_some()
    }

    pub fn attach_interface_with_frame_limit(&self, max_frame_len: usize) -> io::Result<Interface> {
        self.attach_interface_access(max_frame_len, None)
    }

    /// Attach an IFAC-authenticated raw packet interface.
    ///
    /// `max_frame_len` includes the access code, so an eight-byte IFAC leaves
    /// eight fewer bytes for the logical packet on a fixed-size radio frame.
    pub fn attach_interface_with_ifac(
        &self,
        max_frame_len: usize,
        ifac: Ifac,
    ) -> io::Result<Interface> {
        self.attach_interface_access(max_frame_len, Some(ifac))
    }

    fn attach_interface_access(
        &self,
        max_frame_len: usize,
        ifac: Option<Ifac>,
    ) -> io::Result<Interface> {
        let wire_overhead = ifac.as_ref().map_or(0, Ifac::size);
        if max_frame_len < crate::packet::HEADER_MIN_LEN + wire_overhead {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "interface frame limit cannot hold a Reticulum header and access code",
            ));
        }
        let id = self.shared.next_iface_id.fetch_add(1, Ordering::Relaxed);
        let queues = Arc::new(OutboundQueues::new(
            self.shared.queue_weights(),
            self.shared.queue_depths(),
        ));
        let frame_limit = Arc::new(AtomicUsize::new(
            max_frame_len.min(crate::packet::MTU + wire_overhead),
        ));
        if !self.shared.register_interface(Iface {
            id,
            outbound: Arc::clone(&queues),
            frame_limit: Arc::clone(&frame_limit),
            wire_overhead,
            mode: InterfaceMode::Full,
        }) {
            queues.close();
        }
        Ok(Interface {
            id,
            outbound: OutboundPackets {
                queues,
                delivery_in_flight: false,
                ifac: ifac.clone(),
            },
            router_tx: self.shared.router_tx.clone(),
            frame_limit,
            ifac,
            dropped: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Dial a TCP peer and attach it as an interface.
    pub async fn attach_tcp_client(&self, addr: SocketAddr) -> io::Result<InterfaceId> {
        self.attach_tcp_client_access(addr, None).await
    }

    /// Dial an IFAC-authenticated TCP peer and attach it.
    pub async fn attach_tcp_client_with_ifac(
        &self,
        addr: SocketAddr,
        ifac: Ifac,
    ) -> io::Result<InterfaceId> {
        self.attach_tcp_client_access(addr, Some(ifac)).await
    }

    async fn attach_tcp_client_access(
        &self,
        addr: SocketAddr,
        ifac: Option<Ifac>,
    ) -> io::Result<InterfaceId> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let (id, attached) = attach(&self.shared, TcpStream::connect(addr).await?, ifac);
        attached.then_some(id).ok_or_else(endpoint_closed)
    }

    /// Listen on TCP; every accepted connection becomes an interface. Returns the bound
    /// address (pass port 0 to get an OS-assigned one).
    pub async fn listen_tcp(&self, addr: SocketAddr) -> io::Result<SocketAddr> {
        self.listen_tcp_access(addr, None).await
    }

    /// Listen for IFAC-authenticated TCP connections.
    pub async fn listen_tcp_with_ifac(
        &self,
        addr: SocketAddr,
        ifac: Ifac,
    ) -> io::Result<SocketAddr> {
        self.listen_tcp_access(addr, Some(ifac)).await
    }

    async fn listen_tcp_access(
        &self,
        addr: SocketAddr,
        ifac: Option<Ifac>,
    ) -> io::Result<SocketAddr> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let shared = Arc::clone(&self.shared);
        if !track(&self.shared, async move {
            while let Ok((stream, _)) = listener.accept().await {
                if !shared.is_running() {
                    break;
                }
                attach(&shared, stream, ifac.clone());
            }
        }) {
            return Err(endpoint_closed());
        }
        Ok(local)
    }

    /// Number of interfaces currently attached.
    pub fn interface_count(&self) -> usize {
        self.shared.interfaces.lock().unwrap().len()
    }

    /// Stable ordered identifiers for interfaces attached at capture time.
    pub fn interface_ids(&self) -> Vec<InterfaceId> {
        let mut interfaces: Vec<_> = self
            .shared
            .interfaces
            .lock()
            .unwrap()
            .iter()
            .map(|interface| interface.id)
            .collect();
        interfaces.sort_unstable();
        interfaces
    }

    /// Configure bounded announce ingress control for subsequently observed packets.
    /// Rate debt resets; retained interface counters and in-flight cooldowns survive.
    /// Rows are trimmed to capacity and release tasks are woken to reconsider deadlines.
    pub fn set_announce_ingress_policy(&self, policy: AnnounceIngressPolicy) {
        self.shared
            .announce_admission
            .lock()
            .unwrap()
            .set_policy(policy);
        self.shared.held_release_wake.notify_waiters();
    }

    /// The active announce-ingress policy.
    pub fn announce_ingress_policy(&self) -> AnnounceIngressPolicy {
        self.shared.announce_admission.lock().unwrap().policy()
    }

    /// Per-interface ingress accounting. This is carrier attribution, not an on-air receipt.
    pub fn announce_ingress_counters(&self, interface: InterfaceId) -> AnnounceIngressCounters {
        self.shared
            .announce_admission
            .lock()
            .unwrap()
            .counters(interface)
    }

    /// Replace the host receive-freshness policy without discarding retained replay state.
    ///
    /// Shrinking a bound deterministically trims the oldest retained rows or blobs, and drops
    /// the routes of evicted rows. Those removals are reflected in [`RoutingCounters`].
    pub fn set_announce_freshness_policy(
        &self,
        policy: AnnounceFreshnessPolicy,
    ) -> Result<(), AnnounceFreshnessConfigError> {
        let mut freshness = self.shared.announce_freshness.lock().unwrap();
        let changed = freshness.table.reconfigure(policy.config())?;
        freshness.policy = policy;
        self.shared
            .route_ttl_ms
            .store(policy.route_ttl_ticks(), Ordering::Relaxed);
        for destination in &changed.evicted_destinations {
            self.shared.forget_path(*destination);
        }
        self.shared
            .routing_stats
            .freshness_rows_evicted
            .fetch_add(changed.evicted_destinations.len() as u64, Ordering::Relaxed);
        self.shared
            .routing_stats
            .freshness_blobs_evicted
            .fetch_add(changed.evicted_blobs as u64, Ordering::Relaxed);
        Ok(())
    }

    /// The active host receive-freshness policy.
    pub fn announce_freshness_policy(&self) -> AnnounceFreshnessPolicy {
        self.shared.announce_freshness.lock().unwrap().policy
    }

    /// Act as a transport node: forward announces (hops+1, de-duplicated, never back the way
    /// they came) and forward packets toward learned destinations. Off by default, since an
    /// endpoint carries only its own traffic unless it opts in.
    ///
    /// Shorthand for [`RoutingPolicy::transit`]; use
    /// [`set_routing_policy`](Self::set_routing_policy) for anything narrower.
    pub fn enable_routing(&self) {
        self.set_routing_policy(RoutingPolicy::transit());
    }

    /// Install the transit policy: what this endpoint carries for others, from and to which
    /// interfaces, and how far. Takes effect for packets routed after it returns.
    ///
    /// Transit and local service are independent. Carrying nothing
    /// ([`RoutingPolicy::none`], the default) does not affect this endpoint's own links,
    /// announces, or registered destinations.
    pub fn set_routing_policy(&self, policy: RoutingPolicy) {
        let (weights, depths) = (policy.queue_weights, policy.queue_depths);
        *self.shared.routing.lock().unwrap() = policy;
        // Apply the queue policy to interfaces already attached, so a policy change is not
        // silently limited to interfaces attached afterwards.
        for i in self.shared.interfaces.lock().unwrap().iter() {
            i.outbound.set_policy(weights, depths);
        }
    }

    /// What the outbound schedule has done across every interface: released and dropped, by
    /// class. Dropped transit is the visible sign of a bound being enforced rather than a
    /// backlog quietly growing.
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
    /// This is a point-in-time host observation. It does not alter scheduling
    /// and must not be used as a delivery receipt.
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

    /// Cap live inbound links, in total and per destination. Links already up are kept;
    /// the caps govern new requests.
    pub fn set_inbound_link_limits(&self, limits: InboundLinkLimits) {
        self.shared.inbound.lock().unwrap().limits = limits;
    }

    /// The current inbound link caps.
    pub fn inbound_link_limits(&self) -> InboundLinkLimits {
        self.shared.inbound.lock().unwrap().limits
    }

    /// Spread announce relays over a random delay of `0..=max`, instead of relaying the
    /// instant the router hands the announce over.
    ///
    /// Every neighbour that heard an announce is about to relay it. On a shared medium,
    /// relaying immediately means relaying *simultaneously*, so a flood partly destroys
    /// itself. Jitter is the cheapest fix: local timing only, nothing on the wire, nothing
    /// asked of the radio.
    ///
    /// Off by default, since it costs latency and buys nothing on a point-to-point link.
    /// Set it on a shared radio, to something near the air time of an announce (hundreds of
    /// milliseconds on slow spreading factors, tens on fast ones).
    pub fn set_relay_jitter(&self, max: Duration) {
        let ms = max.as_millis().min(u128::from(u64::MAX)) as u64;
        self.shared.relay_jitter_ms.store(ms, Ordering::Relaxed);
    }

    /// Set the first reliable-channel RTT estimate for subsequently opened links. It is a
    /// floor under the RTT an initiator measures at setup, and a responder's whole estimate.
    /// Slow half-duplex radios should include their queue and proof turnaround time.
    pub fn set_reliable_initial_rtt(&self, rtt: Duration) {
        let millis = rtt.as_millis().clamp(1, u128::from(u64::MAX)) as u64;
        self.shared
            .reliable_initial_rtt_ms
            .store(millis, Ordering::Relaxed);
    }

    /// Cap the reliable Channel send window for subsequently opened links.
    ///
    /// Set this to one on strict half-duplex media so each data frame is proved
    /// before another transmission begins. The default is RNS's dynamic maximum.
    pub fn set_reliable_max_window(&self, frames: u32) {
        self.shared.reliable_max_window.store(
            frames.clamp(1, crate::channel::WINDOW_MAX),
            Ordering::Relaxed,
        );
    }

    /// Set the decoded output ceiling for each compressed frame on subsequently
    /// opened reliable streams. This excludes the bz2 decoder's own workspace.
    pub fn set_reliable_decoded_frame_limit(
        &self,
        limit: usize,
    ) -> Result<(), StreamDecodeLimitError> {
        if limit == 0 || limit >= isize::MAX as usize {
            return Err(StreamDecodeLimitError::InvalidLimit);
        }
        self.shared
            .reliable_decoded_frame_limit
            .store(limit, Ordering::Relaxed);
        Ok(())
    }

    /// Set the retry interval for link requests sent by subsequently opened links.
    pub fn set_link_setup_retry(&self, interval: Duration) {
        let millis = interval.as_millis().clamp(1, u128::from(u64::MAX)) as u64;
        self.shared
            .link_setup_retry_ms
            .store(millis, Ordering::Relaxed);
    }

    /// Set the first-hop airtime allowance added to the setup deadline of links opened over
    /// `iface`: the time its medium needs to carry one 500-byte MTU
    /// ([`crate::node::first_hop_airtime`] computes it from a bitrate). Zero, the default,
    /// suits an unbounded medium such as TCP.
    pub fn set_first_hop_airtime(&self, iface: InterfaceId, allowance: Duration) {
        let millis = allowance.as_millis().min(u128::from(u64::MAX)) as u64;
        let mut airtime = self.shared.first_hop_airtime_ms.lock().unwrap();
        if millis == 0 {
            airtime.remove(&iface);
        } else {
            airtime.insert(iface, millis);
        }
    }

    /// Set the MTU requested and offered by subsequently established links.
    ///
    /// The lower bound keeps link setup, identify, and resource control packets
    /// representable while allowing the standard eight-byte IFAC on a 255-byte
    /// packet radio. The default remains Reticulum's 500-byte MTU.
    pub fn set_link_mtu(&self, mtu: u32) {
        self.shared.link_mtu.store(
            mtu.clamp(MIN_LINK_MTU, crate::packet::MTU as u32),
            Ordering::Relaxed,
        );
    }

    /// The interface a learned destination is reachable over, and its hop count. An expired
    /// route is not returned (and is evicted).
    pub fn route_to(&self, dest: AddressHash) -> Option<(InterfaceId, u8)> {
        self.route_to_at(dest, Instant::now())
    }

    /// As [`Self::route_to`], against a supplied monotonic instant. Kept private because a
    /// host captures route observations through [`Self::route_facts_at`], while endpoint tests
    /// need deterministic expiry without sleeping.
    fn route_to_at(&self, dest: AddressHash, now: Instant) -> Option<(InterfaceId, u8)> {
        let route_ttl = self.shared.route_ttl();
        self.shared.write_diagnostic(|| {
            let mut t = self.shared.path_table.lock().unwrap();
            match t.get(&dest) {
                Some(e) if e.live_at(now, route_ttl) => (Some((e.iface, e.hops)), false),
                Some(_) => {
                    t.remove(&dest);
                    (None, true)
                }
                None => (None, false),
            }
        })
    }

    /// Current routes in deterministic destination order, aged against one instant supplied
    /// by the caller. Unlike [`route_to`](Self::route_to), observation never evicts state.
    pub fn route_facts_at(&self, captured_at: Instant) -> Vec<RouteFact> {
        let interfaces = self.interface_ids();
        self.route_facts_for_interfaces_at(captured_at, &interfaces)
            .0
    }

    fn route_facts_for_interfaces_at(
        &self,
        captured_at: Instant,
        interfaces: &[InterfaceId],
    ) -> (Vec<RouteFact>, u64) {
        let interfaces: HashSet<_> = interfaces.iter().copied().collect();
        let route_ttl = self.shared.route_ttl();
        let table = self.shared.path_table.lock().unwrap();
        let mut expired_routes = 0_u64;
        let mut facts = Vec::new();
        for (destination, entry) in table.iter() {
            if !interfaces.contains(&entry.iface) {
                continue;
            }
            let age = captured_at
                .checked_duration_since(entry.learned)
                .unwrap_or_default();
            if age >= entry.lifetime(route_ttl) {
                expired_routes = expired_routes.saturating_add(1);
                continue;
            }
            facts.push(RouteFact {
                destination: *destination,
                interface: entry.iface,
                transport: entry.transport,
                hops: entry.hops,
                age,
            });
        }
        facts.sort_unstable_by_key(|fact| fact.destination);
        (facts, expired_routes)
    }

    /// Live links in deterministic id order. Entries whose carrier was detached are omitted,
    /// because a snapshot must never name an interface absent from the same capture.
    pub fn link_facts(&self) -> Vec<LinkFact> {
        let interfaces = self.interface_ids();
        self.link_facts_for_interfaces(&interfaces)
    }

    fn link_facts_for_interfaces(&self, interfaces: &[InterfaceId]) -> Vec<LinkFact> {
        let interfaces: HashSet<_> = interfaces.iter().copied().collect();
        let links = self.shared.links.lock().unwrap();
        let mut facts: Vec<_> = links
            .iter()
            .filter_map(|(id, entry)| {
                interfaces.contains(&entry.iface).then_some(LinkFact {
                    id: *id,
                    interface: entry.iface,
                    kind: entry.kind.fact_kind(),
                    direction: entry.direction,
                    remote: entry.remote,
                })
            })
            .collect();
        facts.sort_unstable_by_key(|fact| fact.id);
        facts
    }

    /// Capture interface, route, and link facts with referential integrity. All route and
    /// link interface ids occur in the returned `interfaces` list.
    pub fn diagnostic_facts_at(&self, captured_at: Instant) -> EndpointFacts {
        let (generation, (interfaces, routes, links, expired_routes)) =
            self.shared.capture_diagnostic(|| {
                let interfaces = self.interface_ids();
                let (routes, expired_routes) =
                    self.route_facts_for_interfaces_at(captured_at, &interfaces);
                let links = self.link_facts_for_interfaces(&interfaces);
                (interfaces, routes, links, expired_routes)
            });
        EndpointFacts {
            generation,
            routes,
            links,
            interfaces,
            expired_routes,
        }
    }

    /// Monotonic source revision for interface, route, link, and announce facts.
    /// Point-in-time ages and traffic counters are sampled values, not revision sources.
    pub fn diagnostic_generation(&self) -> u64 {
        self.shared.diagnostic_generation.load(Ordering::Acquire)
    }

    /// This endpoint's public identity.
    pub fn identity(&self) -> &Identity {
        self.shared.identity.public()
    }

    /// Register a destination to accept best-effort links on, and announce it. Accept these
    /// with [`accept`](Self::accept).
    pub fn register(&self, name: DestinationName, app_data: &[u8]) {
        self.register_with(name, app_data, RegistrationKind::BestEffort, None);
    }

    /// Register a best-effort-link destination that also receives ratcheted single packets.
    pub fn register_with_ratchets(
        &self,
        name: DestinationName,
        app_data: &[u8],
        ratchets: &RatchetStore,
    ) -> io::Result<()> {
        require_current_ratchet(ratchets)?;
        self.register_with(
            name,
            app_data,
            RegistrationKind::BestEffort,
            Some(ratchets.clone()),
        );
        Ok(())
    }

    /// Register a destination to accept **reliable** links on — the Channel/Buffer path with
    /// proof acks, for lossy interfaces — and announce it. Accept these with
    /// [`accept_reliable`](Self::accept_reliable); the initiator's identity arrives over the
    /// link, so none need be supplied.
    pub fn register_reliable(&self, name: DestinationName, app_data: &[u8]) {
        self.register_with(name, app_data, RegistrationKind::Reliable, None);
    }

    /// Register a destination that accepts resource sessions, then announce it.
    pub fn register_resource(&self, name: DestinationName, app_data: &[u8]) {
        self.register_with(name, app_data, RegistrationKind::Resource, None);
    }

    /// Register a resource destination that also receives ratcheted single packets.
    pub fn register_resource_with_ratchets(
        &self,
        name: DestinationName,
        app_data: &[u8],
        ratchets: &RatchetStore,
    ) -> io::Result<()> {
        require_current_ratchet(ratchets)?;
        self.register_with(
            name,
            app_data,
            RegistrationKind::Resource,
            Some(ratchets.clone()),
        );
        Ok(())
    }

    fn register_with(
        &self,
        name: DestinationName,
        app_data: &[u8],
        kind: RegistrationKind,
        ratchets: Option<RatchetStore>,
    ) {
        let dest = name.destination_hash(self.shared.identity.public());
        self.shared.registered.lock().unwrap().push(Registered {
            dest,
            kind,
            name: name.clone(),
            app_data: app_data.to_vec(),
            ratchets,
        });
        self.announce(&name, app_data);
    }

    /// Replace a registered destination's active receive-ratchet state and announce its
    /// current public key. The caller retains the canonical store and persists its snapshot.
    pub fn update_ratchets(
        &self,
        name: &DestinationName,
        ratchets: &RatchetStore,
    ) -> io::Result<()> {
        require_current_ratchet(ratchets)?;
        let dest = name.destination_hash(self.shared.identity.public());
        let app_data = {
            let mut registered = self.shared.registered.lock().unwrap();
            let registration = registered
                .iter_mut()
                .find(|registration| registration.dest == dest)
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "destination is not registered")
                })?;
            registration.ratchets = Some(ratchets.clone());
            registration.app_data.clone()
        };
        self.announce(name, &app_data);
        Ok(())
    }

    /// Broadcast a path request for `dest`, asking the network to make it reachable. The
    /// matching path response is an announce, ingested like any other, which populates the
    /// path table *and the address book*, so this is also how an identity is learned for a
    /// destination that has only ever been named to us. Use when a route has gone stale so a
    /// subsequent link setup has an interface to go out on, or when a message arrives from a
    /// source whose keys we do not have.
    ///
    /// Rate-limited per destination (see `PATH_REQUEST_MIN_INTERVAL`) and silent when the
    /// request is suppressed, because callers ask on someone else's schedule. Returns whether
    /// a request actually went out, for tests and diagnostics.
    pub fn request_path(&self, dest: AddressHash) -> bool {
        if !self.shared.path_request_within_budget(dest) {
            return false;
        }
        let mut tag = [0u8; crate::path::TAG_LEN];
        fill_random(&mut tag);
        self.shared.broadcast(crate::path::path_request(dest, &tag));
        true
    }

    /// Emit an announce for a destination on every interface.
    pub fn announce(&self, name: &DestinationName, app_data: &[u8]) {
        let pkt = self.build_announce_at(name, app_data, host_announce_seconds());
        self.shared.broadcast(pkt);
    }

    /// The deterministic half of [`Self::announce`]. It is kept private because host callers
    /// obtain wall-clock seconds here; firmware must supply its own reservation-backed
    /// ordinal rather than inherit this unbounded host generator.
    fn build_announce_at(
        &self,
        name: &DestinationName,
        app_data: &[u8],
        source_seconds: u64,
    ) -> Packet {
        let dest = name.destination_hash(self.shared.identity.public());
        let ratchet = self
            .shared
            .registered
            .lock()
            .unwrap()
            .iter()
            .find(|registration| registration.dest == dest)
            .and_then(|registration| registration.ratchets.as_ref())
            .and_then(RatchetStore::current_public);
        self.shared
            .build_announce_at(name, ratchet.as_ref(), app_data, source_seconds)
    }

    /// Encrypt and queue one link-less packet to a destination's advertised ratchet.
    ///
    /// This is delivery, not a receipt from the peer. Success means the packet was encrypted
    /// to the latest validated announce and accepted by at least one local interface queue.
    pub fn send_single(&self, dest: AddressHash, data: &[u8]) -> io::Result<SinglePacketReceipt> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        if data.len() > crate::packet::ENCRYPTED_MDU {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "single-packet plaintext exceeds ENCRYPTED_MDU",
            ));
        }
        let (peer, ratchet) = {
            let address_book = self.shared.address_book.lock().unwrap();
            let peer = address_book.resolve(dest).ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "destination has not announced")
            })?;
            let ratchet = peer.ratchet.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "destination did not advertise a ratchet",
                )
            })?;
            (peer.identity, ratchet)
        };

        let mut ephemeral = [0u8; KEY_LEN];
        fill_random(&mut ephemeral);
        let payload =
            crate::token::encrypt_to_ratchet(&peer, &ratchet, &ephemeral, &next_iv(), data);
        let packet = Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: dest,
            context: 0,
            payload,
        };
        debug_assert!(packet.within_mtu());

        let queued = self.shared.queue_single(dest, packet);
        if queued.queued == 0 {
            if !queued.frame_capable
                && let Some((actual, limit)) = queued.frame_limit_rejection
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "single packet is {actual} bytes after encryption, interface frame limit is {limit}"
                    ),
                ));
            }
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "no interface accepted the single packet",
            ));
        }
        Ok(SinglePacketReceipt {
            destination: dest,
            ratchet_id: NameHash::of(&ratchet),
            queued_interfaces: queued.queued,
        })
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

    /// Open a best-effort link to a destination and return its stream. `peer` is the
    /// destination's identity (learned from an announce, e.g. via [`resolve`](Self::resolve)).
    pub async fn open(&self, dest: AddressHash, peer: Identity) -> io::Result<LinkStream> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        register_stream(
            &self.shared,
            link,
            iface,
            liveness,
            LinkDirection::Outbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(peer),
            },
        )
        .ok_or_else(endpoint_closed)
    }

    /// Open a **reliable** link to a destination — the Channel/Buffer path with proof acks,
    /// for lossy interfaces — and return its stream. `peer` is the destination's identity: the
    /// handshake authenticates it, and the peer's proofs of our packets are validated against
    /// it. As the initiator, the reliable driver IDENTIFYs us to the responder so it can
    /// validate our proofs in turn.
    pub async fn open_reliable(&self, dest: AddressHash, peer: Identity) -> io::Result<LinkStream> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        register_reliable_stream(
            &self.shared,
            link,
            iface,
            liveness,
            Some(peer),
            LinkDirection::Outbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(peer),
            },
        )
        .ok_or_else(endpoint_closed)
    }

    /// Open a link whose packets are driven by the resource transfer state machines.
    pub async fn open_resource(
        &self,
        dest: AddressHash,
        peer: Identity,
    ) -> io::Result<ResourceSession> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        register_resource_session(
            &self.shared,
            link,
            iface,
            liveness,
            LinkDirection::Outbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(peer),
            },
        )
        .ok_or_else(endpoint_closed)
    }

    /// Open a resource link and publish one payload over it.
    pub async fn publish_resource(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
    ) -> io::Result<()> {
        self.publish_resource_with_config(dest, peer, data, ResourceTransferConfig::default())
            .await
    }

    /// Open and publish with explicit retry and total-time policy.
    pub async fn publish_resource_with_config(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
        config: ResourceTransferConfig,
    ) -> io::Result<()> {
        let mut session = self.open_resource(dest, peer).await?;
        session.set_config(config);
        session.publish(data).await
    }

    /// Send one indivisible payload using the form that fits the negotiated link.
    ///
    /// A payload that fits one encrypted data packet takes the low-overhead path.
    /// Larger payloads use a proved Resource on the same established link. This
    /// does not split a logical message into several independent data packets.
    pub async fn send_payload(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
    ) -> io::Result<PayloadMode> {
        self.send_payload_with_config(dest, peer, data, ResourceTransferConfig::default())
            .await
    }

    /// Send one indivisible payload, with explicit policy for the Resource
    /// path used when it does not fit one encrypted data packet.
    ///
    /// The policy is ignored for a payload that takes the Data path.
    pub async fn send_payload_with_config(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
        config: ResourceTransferConfig,
    ) -> io::Result<PayloadMode> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        if data.len() <= write_chunk_for_mtu(link.mtu()) {
            let mut stream = register_stream(
                &self.shared,
                link,
                iface,
                liveness,
                LinkDirection::Outbound,
                LinkRemoteFact {
                    destination: Some(dest),
                    identity: Some(peer),
                },
            )
            .ok_or_else(endpoint_closed)?;
            stream.write_all(data).await?;
            stream.shutdown().await?;
            drop(stream);
            Ok(PayloadMode::Data)
        } else {
            let mut session = register_resource_session(
                &self.shared,
                link,
                iface,
                liveness,
                LinkDirection::Outbound,
                LinkRemoteFact {
                    destination: Some(dest),
                    identity: Some(peer),
                },
            )
            .ok_or_else(endpoint_closed)?;
            session.set_config(config);
            session.publish(data).await?;
            Ok(PayloadMode::Resource)
        }
    }

    /// Open a link, send one request, and return its matching response.
    pub async fn request(
        &self,
        dest: AddressHash,
        peer: Identity,
        request: &Request,
    ) -> io::Result<Response> {
        let mut session = self.open_resource(dest, peer).await?;
        session.request(request).await
    }

    /// Open a link, send one already-packed request, and retain the raw
    /// matching response.
    pub async fn request_raw(
        &self,
        dest: AddressHash,
        peer: Identity,
        packed_request: &[u8],
    ) -> io::Result<ReceivedRawResponse> {
        let mut session = self.open_resource(dest, peer).await?;
        session.request_raw(packed_request).await
    }

    /// Open a resource link and fetch one payload published by the peer.
    pub async fn fetch_resource(&self, dest: AddressHash, peer: Identity) -> io::Result<Vec<u8>> {
        self.fetch_resource_with_config(dest, peer, ResourceTransferConfig::default())
            .await
    }

    /// Open and fetch with explicit retry and total-time policy.
    pub async fn fetch_resource_with_config(
        &self,
        dest: AddressHash,
        peer: Identity,
        config: ResourceTransferConfig,
    ) -> io::Result<Vec<u8>> {
        let mut session = self.open_resource(dest, peer).await?;
        session.set_config(config);
        session.fetch().await
    }

    /// How long a link request to `dest` waits for its proof: Node's deadline for the
    /// route's hop count plus the first-hop airtime of the interface it leaves by, or of the
    /// slowest interface when it is broadcast for want of a route.
    fn link_setup_timeout(&self, dest: AddressHash) -> Duration {
        let route = self.route_to(dest);
        let first_hop = {
            let airtime = self.shared.first_hop_airtime_ms.lock().unwrap();
            match route {
                Some((iface, _)) => airtime.get(&iface).copied().unwrap_or(0),
                None => airtime.values().copied().max().unwrap_or(0),
            }
        };
        let hops = route.map_or(0, |(_, hops)| hops);
        Duration::from_millis(crate::node::link_request_timeout(hops).saturating_add(first_hop))
    }

    /// Establish a link to `dest` (whose identity is `peer`), returning it with the interface
    /// its proof arrived on and its liveness timers. The stream discipline is chosen by the
    /// caller.
    ///
    /// Setup waits [`crate::node::link_request_timeout`] for the route's hop count, plus the
    /// outgoing interface's first-hop airtime ([`Endpoint::set_first_hop_airtime`]). On
    /// timeout an endpoint that is not a transport forgets the route and asks for a new path,
    /// within the path-request budget, as RNS does (`Transport.py` 697-725).
    async fn establish(
        &self,
        dest: AddressHash,
        peer: Identity,
    ) -> io::Result<(Link, InterfaceId, Liveness)> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let ephemeral = ephemeral_seed();
        let link_mtu = self.shared.link_mtu.load(Ordering::Relaxed);
        let (pending, request) = link::PendingLink::open(
            dest,
            peer,
            &ephemeral,
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: link_mtu,
            },
        );

        let link_id = pending.link_id();
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(link_id, tx);
        // Stash the pending link so the router can prove it.
        self.shared
            .pending_links
            .lock()
            .unwrap()
            .insert(link_id, pending);
        // If setup does not complete — it times out below, or the caller drops this future —
        // remove both entries on the way out so a failed setup never leaks router state.
        let mut guard = PendingGuard {
            shared: Arc::clone(&self.shared),
            link_id,
            armed: true,
        };

        // Send the request toward the destination: on the interface the path table names
        // (addressed via its transport node if remote), or broadcast if we have no route yet
        // (a directly-connected peer).
        let send_request = || {
            match self.shared.path_iface(dest) {
                Some(iface) => self.shared.send_on(iface, request.clone()),
                None => self.shared.broadcast(request.clone()),
            }
            Instant::now()
        };
        let setup_timeout = self.link_setup_timeout(dest);
        // The proof's arrival measures the RTT from this first transmission. A proof after a
        // retry cannot say which copy it answers (Karn), so the measurement keeps this
        // conservative bound rather than guessing the shorter one.
        let sent_at = tokio::time::Instant::now();
        send_request();

        let retry = Duration::from_millis(self.shared.link_setup_retry_ms.load(Ordering::Relaxed));
        let mut retries = tokio::time::interval_at(tokio::time::Instant::now() + retry, retry);
        retries.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let deadline = tokio::time::sleep(setup_timeout);
        let closed = self.shared.closed_notify.notified();
        tokio::pin!(deadline);
        tokio::pin!(rx);
        tokio::pin!(closed);
        if self.shared.is_closed() {
            return Err(endpoint_closed());
        }

        loop {
            tokio::select! {
                result = &mut rx => match result {
                    Ok((link, iface)) => {
                        guard.armed = false; // router removed both entries on success
                        if self.shared.is_running() {
                            // The responder does not activate an inbound link until the
                            // initiator reports its measured RTT. Keep this ahead of any
                            // application packet emitted by the returned session.
                            let rtt = sent_at.elapsed();
                            self.shared
                                .send_on(iface, link.rtt_packet(rtt.as_secs_f32(), &next_iv()));
                            let rtt_ms = rtt.as_millis().min(u128::from(u64::MAX)) as u64;
                            let liveness =
                                Liveness::initiator(rtt_ms, self.shared.link_clock_ms());
                            return Ok((link, iface, liveness));
                        }
                        self.shared.send_on(iface, link.close_packet(&next_iv()));
                        return Err(endpoint_closed());
                    }
                    Err(_) => return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "link setup dropped",
                    )),
                },
                _ = retries.tick() => {
                    send_request();
                }
                _ = &mut deadline => {
                    if !self.shared.routing.lock().unwrap().forward_packets {
                        self.shared.forget_path(dest);
                        self.request_path(dest);
                    }
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "link setup timed out"));
                }
                _ = &mut closed => return Err(endpoint_closed()),
            }
        }
    }

    /// Wait for the next inbound link, surfaced as a stream.
    pub async fn accept(&self) -> io::Result<LinkStream> {
        Ok(self.accept_on_any().await?.stream)
    }

    /// Wait for the next inbound link, with the destination it targeted (an ALPN maps to a
    /// destination, so a host can dispatch by protocol).
    pub async fn accept_on_any(&self) -> io::Result<Accepted> {
        let accepted = recv_until_closed(&self.shared, &self.accepted_rx).await?;
        self.shared
            .inbound
            .lock()
            .unwrap()
            .taken(accepted.destination);
        Ok(accepted)
    }

    /// Wait for the next inbound **reliable** link (to a destination registered with
    /// [`register_reliable`](Self::register_reliable)) and return its stream. The initiator's
    /// identity is learned from the IDENTIFY it sends, so — unlike before — no peer identity
    /// need be supplied here; the driver validates the initiator's proofs once it arrives.
    pub async fn accept_reliable(&self) -> io::Result<LinkStream> {
        Ok(self.accept_reliable_on_any().await?.stream)
    }

    /// Wait for the next inbound reliable link, retaining the destination and
    /// physical interface on which its request arrived.
    pub async fn accept_reliable_on_any(&self) -> io::Result<Accepted> {
        let accepted = recv_until_closed(&self.shared, &self.reliable_accepted_rx).await?;
        self.shared
            .inbound
            .lock()
            .unwrap()
            .taken(accepted.destination);
        Ok(accepted)
    }

    /// Wait for an inbound resource link, including the destination it targeted.
    pub async fn accept_resource(&self) -> io::Result<AcceptedResource> {
        let accepted = recv_until_closed(&self.shared, &self.resource_accepted_rx).await?;
        self.shared
            .inbound
            .lock()
            .unwrap()
            .taken(accepted.destination);
        Ok(accepted)
    }

    /// The next validated announce, for building a host peer-id to destination map.
    pub async fn next_announcement(&self) -> io::Result<PeerAnnounce> {
        recv_until_closed(&self.shared, &self.announce_rx).await
    }

    /// Wait for the next authenticated link-less single packet.
    pub async fn accept_single(&self) -> io::Result<ReceivedSingle> {
        recv_until_closed(&self.shared, &self.single_rx).await
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
}

impl Endpoint {
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

/// Attach a connected stream as an interface: register it, and spawn its writer and reader
/// tasks (the reader feeds the shared router, tagged with the interface id).
fn attach(shared: &Arc<Shared>, stream: TcpStream, ifac: Option<Ifac>) -> (InterfaceId, bool) {
    let _ = stream.set_nodelay(true);
    let id = shared.next_iface_id.fetch_add(1, Ordering::Relaxed);
    let queues = Arc::new(OutboundQueues::new(
        shared.queue_weights(),
        shared.queue_depths(),
    ));
    let wire_overhead = ifac.as_ref().map_or(0, Ifac::size);
    if !shared.register_interface(Iface {
        id,
        outbound: Arc::clone(&queues),
        frame_limit: Arc::new(AtomicUsize::new(crate::packet::MTU + wire_overhead)),
        wire_overhead,
        mode: InterfaceMode::Full,
    }) {
        queues.close();
        return (id, false);
    }
    let (mut read_half, mut write_half) = stream.into_split();

    // Writer: frame and send this interface's outbound packets, in schedule order.
    let mut out_rx = OutboundPackets {
        queues,
        delivery_in_flight: false,
        ifac: ifac.clone(),
    };
    let writer_started = track(shared, async move {
        while let Some(pkt) = out_rx.recv().await {
            let Ok(wire) = out_rx.encode(&pkt) else {
                break;
            };
            if write_half.write_all(&frame(&wire)).await.is_err() {
                break;
            }
            let _ = write_half.flush().await;
        }
    });

    // Reader: deframe, decode, hand to the router tagged with this interface.
    let router_tx = shared.router_tx.clone();
    let owner = Arc::clone(shared);
    let reader_started = track(shared, async move {
        let mut deframer = Deframer::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = match read_half.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for raw in deframer.push(&buf[..n]) {
                let logical = match &ifac {
                    Some(ifac) => match ifac.open(&raw) {
                        Ok(logical) => logical,
                        Err(_) => continue,
                    },
                    None => raw,
                };
                // Await on a full router queue rather than dropping: this back-pressures the
                // socket read, so TCP flow control slows a flooding peer. `send` errors only
                // when the router is gone, which means the endpoint is shutting down.
                if let Ok(pkt) = Packet::decode(&logical)
                    && router_tx.send((id, pkt)).await.is_err()
                {
                    // The endpoint is going away, and it will tear its own state down.
                    return;
                }
            }
        }
        // The socket is gone, so this interface is. Attaching used to be one-way: a peer
        // that reconnected -- a flapping link, a restarted daemon -- left its record and its
        // queues behind on every cycle, and the scheduler kept visiting them. Reaching here
        // is exactly the moment there is nothing left to visit.
        owner.forget_interface(id);
    });

    let attached = writer_started && reader_started;
    if !attached
        && let Some(iface) = shared
            .interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|iface| iface.id == id)
    {
        iface.outbound.close();
    }
    (id, attached)
}

fn deliver_single(shared: &Arc<Shared>, iface: InterfaceId, pkt: &Packet) {
    if !shared.is_running() {
        return;
    }
    let registration = shared
        .registered
        .lock()
        .unwrap()
        .iter()
        .find(|registration| registration.dest == pkt.destination)
        .map(|registration| registration.ratchets.clone());
    let Some(ratchets) = registration else {
        return;
    };

    let decrypted = match ratchets {
        Some(ratchets) => ratchets
            .decrypt(&shared.identity, &pkt.payload)
            .ok()
            .map(|(data, ratchet_id)| (data, Some(ratchet_id))),
        None => crate::token::decrypt_to_identity(&shared.identity, &pkt.payload)
            .ok()
            .map(|data| (data, None)),
    };
    if let Some((data, ratchet_id)) = decrypted {
        let _ = shared.single_tx.send(ReceivedSingle {
            destination: pkt.destination,
            interface: iface,
            data,
            ratchet_id,
        });
    }
}

/// Release verified unknown-route announces one at a time after their ingress interface has
/// calmed. The task is per interface, not per packet, so a burst cannot turn into a timer
/// storm. It is tracked with the endpoint's other tasks and is aborted on close.
fn start_held_announce_release(shared: &Arc<Shared>, iface: InterfaceId, first_due_ms: u64) {
    if !shared.held_release_tasks.lock().unwrap().insert(iface) {
        return;
    }
    let owner = Arc::clone(shared);
    if !track(shared, async move {
        let mut due_ms = first_due_ms;
        loop {
            let now_ms = owner.announce_admission_now_ms();
            if due_ms > now_ms {
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_millis(due_ms - now_ms)) => {}
                    _ = owner.held_release_wake.notified() => {}
                }
            }
            if !owner.is_running() {
                break;
            }

            let now_ms = owner.announce_admission_now_ms();
            let has_held = owner
                .held_announces
                .lock()
                .unwrap()
                .iter()
                .any(|announce| announce.interface == iface);
            if !has_held {
                break;
            }
            let Some(next_due_ms) = owner
                .announce_admission
                .lock()
                .unwrap()
                .release_due(iface, now_ms)
            else {
                // The bounded ledger evicted this interface (or policy cleared it).
                // Retire its deferred work; otherwise task restart would spin forever.
                owner
                    .held_announces
                    .lock()
                    .unwrap()
                    .retain(|held| held.interface != iface);
                break;
            };
            if next_due_ms > now_ms {
                due_ms = next_due_ms;
                continue;
            }

            let held = {
                let mut queue = owner.held_announces.lock().unwrap();
                queue
                    .iter()
                    .position(|announce| announce.interface == iface)
                    .and_then(|index| queue.remove(index))
            };
            let Some(held) = held else {
                due_ms = next_due_ms;
                continue;
            };
            owner
                .announce_admission
                .lock()
                .unwrap()
                .note_released(iface);
            process_verified_announce(&owner, iface, held.packet, held.announce);
            due_ms = next_due_ms;
        }

        owner.held_release_tasks.lock().unwrap().remove(&iface);
        let next_due_ms = owner.announce_admission_now_ms();
        if owner
            .held_announces
            .lock()
            .unwrap()
            .iter()
            .any(|announce| announce.interface == iface)
        {
            start_held_announce_release(&owner, iface, next_due_ms);
        }
    }) {
        shared.held_release_tasks.lock().unwrap().remove(&iface);
    }
}

/// Continue a verified announce after interface admission. A destination-rate block keeps
/// learning and local publication intact, but stops the expensive mesh-wide rebroadcast.
fn process_verified_announce(
    shared: &Arc<Shared>,
    iface: InterfaceId,
    pkt: Packet,
    announce: Announce,
) {
    // This guard deliberately spans every announce effect. Held-release tasks run separately
    // from the packet loop; without one ordered bundle, two candidates could both evaluate as
    // admissible and publish/relay out of freshness order.
    let mut freshness = shared.announce_freshness.lock().unwrap();
    let candidate = AnnounceFreshnessCandidate {
        destination: announce.destination,
        blob: AnnounceBlob::from_wire(announce.rand_hash),
    };
    // Freshness belongs to the route. Without a live one this is a first sighting, as after
    // an RNS path cull, which is what lets a cached path response restore an expired route.
    let route_live = shared.has_live_route(announce.destination);
    let accepted = match freshness.table.evaluate(candidate, route_live) {
        AnnounceFreshnessDecision::Accept(accepted) => accepted,
        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::Replay) => {
            shared
                .routing_stats
                .freshness_replays_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        AnnounceFreshnessDecision::Reject(AnnounceFreshnessReject::StaleTimebase) => {
            shared
                .routing_stats
                .freshness_stale_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
    };

    // A full book makes room by evicting the least recently heard peer that has neither a
    // live path nor a live link. Pending link requests already hold the peer's identity, so
    // they do not need the entry. A refusal only keeps the identity out of the book (and so
    // publishes no `PeerAnnounce`): the path is still learned and the announce still relayed,
    // as RNS relays from its path table rather than its known destinations.
    // The book only needs a monotonic tick to order peers by when they were last heard.
    let now = shared.announce_admission_now_ms();
    let in_use = {
        let book = shared.address_book.lock().unwrap();
        book.is_full() && !book.knows(announce.destination)
    }
    .then(|| shared.destinations_in_use());
    let admitted = shared
        .address_book
        .lock()
        .unwrap()
        .ingest_at(&announce, now, |destination| {
            in_use
                .as_ref()
                .is_some_and(|in_use| in_use.contains(&destination))
        })
        != crate::address_book::Ingested::Refused;
    if !admitted {
        shared
            .routing_stats
            .refused_announces
            .fetch_add(1, Ordering::Relaxed);
    }
    let record = freshness.table.record_accepted(candidate, accepted);
    if let Some(evicted) = record.evicted_destination {
        // A route never outlives its freshness row.
        shared.forget_path(evicted);
        shared
            .routing_stats
            .freshness_rows_evicted
            .fetch_add(1, Ordering::Relaxed);
    }
    if record.evicted_blob.is_some() {
        shared
            .routing_stats
            .freshness_blobs_evicted
            .fetch_add(1, Ordering::Relaxed);
    }
    let destination = announce.destination;
    // A header-type-2 announce names the transport node forwarding it. It belongs to this
    // destination's route, not to the interface: the same radio routinely reaches different
    // destinations through different nodes.
    shared.learn_path(destination, iface, pkt.hops, pkt.transport);
    if admitted {
        let sequence = shared.announce_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let _ = shared.announce_tx.send(PeerAnnounce {
            destination,
            identity: announce.identity,
            app_data: announce.app_data,
            interface: iface,
            hops: pkt.hops,
            transport: pkt.transport,
            sequence,
        });
    }

    // A path response answers one requester. RNS learns from it but never queues it for
    // rebroadcast, and leaves it out of the announce rate table, so one path request cannot
    // flood the mesh.
    if pkt.context == crate::path::CTX_PATH_RESPONSE {
        return;
    }

    // As a transport node, propagate the announce onward: hops+1, stamped with our identity
    // as the transport node so downstream peers address replies through us, out every
    // permitted interface but the one it came in on, de-duplicated by packet hash.
    let policy = shared.routing.lock().unwrap().clone();
    if !policy.relays_announce_from(iface) || !shared.announce_is_new(pkt.hash()) {
        return;
    }
    if shared
        .announce_admission
        .lock()
        .unwrap()
        .observe_destination(destination, shared.announce_admission_now_ms())
        == DestinationVerdict::BlockRelay
    {
        shared
            .routing_stats
            .relay_rate_limited_announces
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    if pkt.hops >= policy.max_hops {
        shared
            .routing_stats
            .hop_limit_dropped
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    let mut fwd = pkt;
    fwd.hops += 1;
    fwd.header_type = crate::packet::HeaderType::Type2;
    fwd.transport = Some(shared.identity.public().hash());
    // Every neighbour that heard this announce is about to relay it. If they all relay the
    // instant the router hands it over, they transmit on top of each other and the flood partly
    // destroys itself. A short random delay spreads the relays out.
    let jitter = shared.relay_jitter();
    if jitter.is_zero() {
        relay_announce(shared, iface, fwd, &policy.allowed_egress);
    } else {
        let shared = Arc::clone(shared);
        let egress = policy.allowed_egress.clone();
        track(&Arc::clone(&shared), async move {
            tokio::time::sleep(jitter).await;
            relay_announce(&shared, iface, fwd, &egress);
        });
    }
}

/// Advance every link's liveness timers: send due keepalives, and drop links that went
/// stale (with a LINKCLOSE) or whose initiator never reported an RTT. A dropped link's
/// stream reads end in [`io::ErrorKind::TimedOut`], its session's in
/// [`io::ErrorKind::BrokenPipe`].
fn watch_links(shared: &Shared) {
    let now = shared.link_clock_ms();
    let sends = shared.write_diagnostic(|| {
        let mut links = shared.links.lock().unwrap();
        let mut sends = Vec::new();
        let mut lost = Vec::new();
        for (id, entry) in links.iter_mut() {
            match entry.liveness.poll(now) {
                None => {}
                Some(Due::Keepalive) => sends.push((
                    entry.iface,
                    entry.link.keepalive_packet(link::KEEPALIVE_REQUEST),
                )),
                Some(Due::Teardown) => {
                    sends.push((entry.iface, entry.link.close_packet(&next_iv())));
                    lost.push(*id);
                }
                Some(Due::HandshakeTimeout) => lost.push(*id),
            }
        }
        for id in &lost {
            if let Some(entry) = links.remove(id) {
                entry.lost.store(true, Ordering::Release);
            }
        }
        (sends, !lost.is_empty())
    });
    for (iface, packet) in sends {
        shared.send_on_class(iface, packet, TrafficClass::Control);
    }
}

/// Feed an inbound packet on one of our links to its liveness timers, answering a keepalive
/// request when due. Returns whether the packet was liveness upkeep (a keepalive or the RTT
/// packet), which nothing else consumes.
fn note_link_inbound(shared: &Shared, pkt: &Packet) -> bool {
    let now = shared.link_clock_ms();
    let reply = {
        let mut links = shared.links.lock().unwrap();
        let Some(entry) = links.get_mut(&pkt.destination) else {
            return false;
        };
        match pkt.context {
            link::CTX_KEEPALIVE if pkt.packet_type == PacketType::Data => {
                entry.liveness.on_keepalive(&pkt.payload, now).then(|| {
                    (
                        entry.iface,
                        entry.link.keepalive_packet(link::KEEPALIVE_RESPONSE),
                    )
                })
            }
            link::CTX_LRRTT if pkt.packet_type == PacketType::Data => {
                if let Some(rtt) = link_liveness::read_rtt(&entry.link, pkt) {
                    entry.liveness.on_rtt(rtt, now);
                }
                None
            }
            _ => {
                entry.liveness.on_inbound(now);
                return false;
            }
        }
    };
    if let Some((iface, packet)) = reply {
        shared.send_on_class(iface, packet, TrafficClass::Control);
    }
    true
}

/// Dispatch one inbound packet that arrived on `iface`.
fn route(shared: &Arc<Shared>, iface: InterfaceId, pkt: Packet) {
    // RNS's packet filter, ahead of any dispatch (`Transport.py` 1629-1660): a header-type-2
    // packet is carried only by the transport it names, and a PLAIN or GROUP packet never
    // travels past its first hop.
    if pkt.packet_type != PacketType::Announce
        && ((pkt.header_type == crate::packet::HeaderType::Type2
            && pkt.transport != Some(shared.identity.public().hash()))
            || (matches!(
                pkt.destination_type,
                DestinationType::Plain | DestinationType::Group
            ) && pkt.hops > 1))
    {
        shared
            .routing_stats
            .filtered_packets
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    // A path request for a destination we own: answer it with a path response (an announce
    // carrying context 0x0b) so a peer that lost its route to us can rediscover it. RNS drops
    // a tagless request and a repeated target and tag (`Transport.py` 1838-1856), and answers
    // for a local destination on the requesting interface only (`Transport.py` 3452-3456).
    // We answer only for our own destinations; with no announce cache we cannot answer for
    // others.
    if let Some(request) = crate::path::PathRequest::parse(&pkt) {
        let fresh = request
            .unique_tag()
            .is_some_and(|tag| shared.path_request_tags.lock().unwrap().insert(tag));
        if !fresh {
            shared
                .routing_stats
                .filtered_packets
                .fetch_add(1, Ordering::Relaxed);
        } else if let Some(resp) = shared.path_response(request.target) {
            shared.send_on_class(iface, resp, TrafficClass::Control);
        }
        return;
    }
    // Transport-node forwarding (announces are re-forwarded in their own arm instead, so
    // they still populate our address book).
    let policy = shared.routing.lock().unwrap().clone();
    if pkt.packet_type != PacketType::Announce {
        // A packet whose destination is a link we bridge goes to the opposite side, whatever
        // its header type: the two endpoints may address it differently (one type-2 through
        // us, one type-1 direct, e.g. a responder that never learned it is behind us).
        // Traffic from either end of a bridge is proof it is still wanted. A packet
        // arriving on a third interface cannot use or refresh that bridge.
        let bridged = {
            let mut bridges = shared.link_transport.lock().unwrap();
            let now = Instant::now();
            match bridges.get_mut(&pkt.destination) {
                Some(bridge) if !bridge.lapsed(now) => {
                    let Some(proves) = bridge.admit(iface, &pkt, &shared.address_book) else {
                        shared
                            .routing_stats
                            .policy_rejected
                            .fetch_add(1, Ordering::Relaxed);
                        return;
                    };
                    bridge.seen = now;
                    Some((bridge.from, bridge.out, proves))
                }
                _ => None,
            }
        };
        // A header-type-2 packet addressed to us as the transport hop is likewise someone
        // else's traffic asking to be carried.
        let addressed_to_us_as_hop = pkt.header_type == crate::packet::HeaderType::Type2
            && pkt.transport == Some(shared.identity.public().hash());

        if bridged.is_some() || addressed_to_us_as_hop {
            // This is transit, not ours. Policy decides whether we carry it; a refusal is
            // counted and the packet is dropped rather than falling through to local
            // handling, since we are not its destination either way.
            if !policy.accepts_transit_from(iface) {
                shared
                    .routing_stats
                    .policy_rejected
                    .fetch_add(1, Ordering::Relaxed);
                return;
            }
            match bridged {
                Some((a, b, proves)) => {
                    let link_id = pkt.destination;
                    let out = if iface == a { b } else { a };
                    // As in RNS, the bridge is validated as its proof goes out, not before.
                    if forward_on(shared, out, pkt, &policy)
                        && proves
                        && let Some(bridge) =
                            shared.link_transport.lock().unwrap().get_mut(&link_id)
                    {
                        bridge.proof_deadline = None;
                    }
                }
                None => forward(shared, iface, pkt, &policy),
            }
            return;
        }
    }
    match pkt.packet_type {
        PacketType::Announce => {
            // A relay rebroadcasting one of our own announces echoes it back. We are not our
            // own peer: drop it before it costs a signature check, or becomes a path to
            // ourselves, a `PeerAnnounce`, or a relay.
            if shared
                .registered
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.dest == pkt.destination)
            {
                return;
            }
            if let Ok(a) = Announce::decode(&pkt) {
                let route_is_known = shared
                    .path_table
                    .lock()
                    .unwrap()
                    .contains_key(&a.destination);
                let verdict = shared.announce_admission.lock().unwrap().observe_interface(
                    iface,
                    route_is_known,
                    shared.announce_admission_now_ms(),
                );
                match verdict {
                    InterfaceVerdict::Process => {
                        process_verified_announce(shared, iface, pkt, a);
                    }
                    InterfaceVerdict::Hold { release_at_ms } => {
                        let held = HeldAnnounce {
                            interface: iface,
                            packet: pkt,
                            announce: a,
                        };
                        if shared.hold_announce(held) {
                            shared.announce_admission.lock().unwrap().note_held(iface);
                            shared
                                .routing_stats
                                .held_announces
                                .fetch_add(1, Ordering::Relaxed);
                            start_held_announce_release(shared, iface, release_at_ms);
                        } else {
                            shared
                                .announce_admission
                                .lock()
                                .unwrap()
                                .note_held_dropped(iface);
                            shared
                                .routing_stats
                                .held_announces_dropped
                                .fetch_add(1, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
        PacketType::LinkRequest => {
            if !shared.is_running() {
                return;
            }
            let dest = pkt.destination;
            let kind = shared
                .registered
                .lock()
                .unwrap()
                .iter()
                .find(|r| r.dest == dest)
                .map(|r| r.kind);
            if let Some(kind) = kind {
                let request_link_id = link::link_id(&pkt).ok();
                if let Some(link_id) = request_link_id {
                    let cached = {
                        let mut cache = shared.inbound_link_proofs.lock().unwrap();
                        cache.retain(|_, (_, at)| at.elapsed() < LINK_REQUEST_CACHE_TTL);
                        cache.get(&link_id).map(|(proof, _)| proof.clone())
                    };
                    if let Some(proof) = cached {
                        shared.send_on(iface, proof);
                        return;
                    }
                }
                // Refuse a new link past the inbound caps or its destination's full accept
                // backlog before any key agreement, task, or buffer is spent on it. Only the
                // router admits links, so the room seen here is still there below.
                let admission = shared.inbound.lock().unwrap().admission(dest);
                if matches!(admission, Admission::Refuse) {
                    shared
                        .routing_stats
                        .inbound_links_refused
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
                let ephemeral = ephemeral_seed();
                // As in RNS, the link is no larger than the interface that heard its request.
                let configured_mtu = shared.link_mtu.load(Ordering::Relaxed);
                let configured_mtu = shared
                    .link_mtu_on(iface)
                    .map_or(configured_mtu, |limit| configured_mtu.min(limit));
                let requested_mtu = pkt
                    .payload
                    .get(link::LINK_KEYS_LEN..link::LINK_KEYS_LEN + link::TRAILER_LEN)
                    .and_then(|bytes| bytes.try_into().ok())
                    .and_then(|trailer| LinkTrailer::decode(trailer).ok())
                    .map(|trailer| trailer.mtu)
                    // As in RNS a signalled 0 means the default; a request below the smallest
                    // workable link is held to that floor.
                    .filter(|&mtu| mtu != 0)
                    .map_or(configured_mtu, |mtu| mtu.max(crate::node::MIN_LOGICAL_MTU));
                if let Ok((link, proof)) = link::accept(
                    &pkt,
                    &shared.identity,
                    &ephemeral,
                    LinkTrailer {
                        mode: LinkMode::Aes256Cbc,
                        mtu: configured_mtu.min(requested_mtu),
                    },
                ) {
                    {
                        let mut cache = shared.inbound_link_proofs.lock().unwrap();
                        if cache.len() >= LINK_REQUEST_CACHE {
                            cache.retain(|_, (_, at)| at.elapsed() < LINK_REQUEST_CACHE_TTL);
                        }
                        if cache.len() >= LINK_REQUEST_CACHE
                            && let Some(oldest) = cache
                                .iter()
                                .min_by_key(|(_, (_, at))| *at)
                                .map(|(id, _)| *id)
                        {
                            cache.remove(&oldest);
                        }
                        cache.insert(link.id(), (proof.clone(), Instant::now()));
                    }
                    let link_id = link.id();
                    if let Admission::Evict(stale) = admission {
                        shared.evict_inbound(stale);
                    }
                    shared.inbound.lock().unwrap().admit(link_id, dest);
                    shared.send_on(iface, proof);
                    let liveness = Liveness::responder(shared.link_clock_ms(), pkt.hops);
                    match kind {
                        RegistrationKind::Reliable => {
                            // Register eagerly with no peer yet: the driver learns the
                            // initiator's identity from the IDENTIFY it sends.
                            if let Some(stream) = register_reliable_stream(
                                shared,
                                link,
                                iface,
                                liveness,
                                None,
                                LinkDirection::Inbound,
                                LinkRemoteFact::default(),
                            ) {
                                shared.inbound.lock().unwrap().queued(dest);
                                let _ = shared.reliable_accepted_tx.send(Accepted {
                                    stream,
                                    destination: dest,
                                    interface: iface,
                                });
                            }
                        }
                        RegistrationKind::Resource => {
                            if let Some(session) = register_resource_session(
                                shared,
                                link,
                                iface,
                                liveness,
                                LinkDirection::Inbound,
                                LinkRemoteFact::default(),
                            ) {
                                shared.inbound.lock().unwrap().queued(dest);
                                let _ = shared.resource_accepted_tx.send(AcceptedResource {
                                    session,
                                    destination: dest,
                                    interface: iface,
                                });
                            } else {
                                shared.remove_link(link_id);
                            }
                        }
                        RegistrationKind::BestEffort => {
                            if let Some(stream) = register_stream(
                                shared,
                                link,
                                iface,
                                liveness,
                                LinkDirection::Inbound,
                                LinkRemoteFact::default(),
                            ) {
                                shared.inbound.lock().unwrap().queued(dest);
                                let _ = shared.accepted_tx.send(Accepted {
                                    stream,
                                    destination: dest,
                                    interface: iface,
                                });
                            }
                        }
                    }
                }
            }
        }
        PacketType::Proof => {
            // Complete a pending outbound link, binding it to the interface it came in on.
            // Validate the proof against the pending link BEFORE removing it: a forged proof
            // addressed to a real pending link id must not be able to evict it and strand the
            // genuine proof that follows. Only a proof that actually verifies removes it.
            let proved = {
                let mut pend = shared.pending_links.lock().unwrap();
                let link = pend.get(&pkt.destination).and_then(|p| p.prove(&pkt).ok());
                if link.is_some() {
                    pend.remove(&pkt.destination);
                }
                link
            };
            if let Some(link) = proved {
                if let Some(tx) = shared.pending.lock().unwrap().remove(&pkt.destination) {
                    let _ = tx.send((link, iface));
                }
            } else {
                // Otherwise a link-data proof for an established link: hand it to the
                // reliable driver, which matches its hash to an outstanding sequence.
                // Best-effort links never request proofs, so there is nothing to do.
                let packets = shared
                    .links
                    .lock()
                    .unwrap()
                    .get(&pkt.destination)
                    .and_then(|e| match &e.kind {
                        LinkKind::Reliable { packets } | LinkKind::Resource { packets } => {
                            Some(packets.clone())
                        }
                        LinkKind::BestEffort { .. } => None,
                    });
                note_link_inbound(shared, &pkt);
                if let Some(packets) = packets {
                    shared.queue_link_packet(&packets, pkt);
                }
            }
        }
        PacketType::Data => {
            // Link data: route to the matching stream by its delivery discipline. Clone the
            // sender(s) under the lock, then act on the packet once the lock is released.
            let (link, raw, best) = {
                let links = shared.links.lock().unwrap();
                match links.get(&pkt.destination) {
                    Some(e) => match &e.kind {
                        LinkKind::Reliable { packets } | LinkKind::Resource { packets } => {
                            (Some(e.link.clone()), Some(packets.clone()), None)
                        }
                        LinkKind::BestEffort { inbound, fault } => (
                            Some(e.link.clone()),
                            None,
                            Some((inbound.clone(), Arc::clone(fault), e.iface)),
                        ),
                    },
                    None => (None, None, None),
                }
            };
            // On one of our links, our own packet heard back or the far end's heard twice
            // is not new traffic, whichever discipline the link uses.
            if raw.is_some() || best.is_some() {
                let admission = shared.link_packets.lock().unwrap().admit(&pkt);
                let dropped = match admission {
                    LinkPacketAdmission::New => None,
                    LinkPacketAdmission::OwnEcho => Some(&shared.routing_stats.own_echo_dropped),
                    LinkPacketAdmission::Duplicate => Some(&shared.routing_stats.duplicate_dropped),
                };
                if let Some(counter) = dropped {
                    counter.fetch_add(1, Ordering::Relaxed);
                    return;
                }
                // Heard from the peer. Keepalives and the RTT packet end here, but the RTT
                // packet is also what activates an inbound link (RNS activates on it), so the
                // inbound cap must see it first.
                if note_link_inbound(shared, &pkt) {
                    if let Some(link) = &link {
                        shared.note_inbound_traffic(link, &pkt);
                    }
                    return;
                }
            }
            if let Some(link) = &link {
                shared.note_inbound_traffic(link, &pkt);
            }
            if let Some(packets) = raw {
                // The reliable or resource driver owns this packet; hand it over raw. A
                // reliable driver recovers a dropped one by retransmission.
                shared.queue_link_packet(&packets, pkt);
            } else if let (Some(link), Some((inbound, fault, link_iface))) = (link, best) {
                match link.receive(&pkt) {
                    Some(Inbound::Data(bytes)) => {
                        if let Err(mpsc::error::TrySendError::Full(_)) = inbound.try_send(bytes) {
                            // Nothing re-sends best-effort data, so a dropped chunk would
                            // be a silent hole in the byte stream. Fail the stream instead:
                            // its reader gets what arrived, then an error.
                            shared
                                .routing_stats
                                .link_queue_dropped
                                .fetch_add(1, Ordering::Relaxed);
                            *fault.lock().unwrap() = Some(StreamFault::Overrun);
                            shared.send_on(link_iface, link.close_packet(&next_iv()));
                            shared.remove_link(pkt.destination);
                        }
                    }
                    Some(Inbound::Close) => {
                        // The peer closed the link: drop its entry so the inbound
                        // sender is released. The stream's inbound relay then ends
                        // and the local reader sees EOF (what read-to-end needs).
                        shared.remove_link(pkt.destination);
                    }
                    _ => {}
                }
            } else if pkt.destination_type == DestinationType::Single && shared.packet_is_new(&pkt)
            {
                deliver_single(shared, iface, &pkt);
            }
        }
    }
}

/// Forward a header-type-2 packet addressed to us as a transport hop, toward its
/// destination. `from` is the interface it arrived on.
fn forward(shared: &Arc<Shared>, from: InterfaceId, pkt: Packet, policy: &RoutingPolicy) {
    if pkt.hops >= policy.max_hops {
        shared
            .routing_stats
            .hop_limit_dropped
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    // A copy already carried is a loop or a second path, never new work.
    if !shared.packet_is_new(&pkt) {
        return;
    }
    let dest = pkt.destination;

    // Route toward the destination by the path table (unexpired routes only).
    let next = shared.path_iface(dest);
    if let Some(out) = next {
        // Refuse before recording anything: a bridge recorded for a route policy will not
        // carry would strand the link's later packets in a table that never fires.
        if !policy.allowed_egress.allows(out) {
            shared
                .routing_stats
                .policy_rejected
                .fetch_add(1, Ordering::Relaxed);
            return;
        }
        // A link request establishes a bridge: record the link id's two interfaces so the
        // proof and subsequent link data forward back the way they came.
        let mut pkt = pkt;
        let mut admitted = None;
        if pkt.packet_type == PacketType::LinkRequest
            && let Ok(link_id) = link::link_id(&pkt)
        {
            match admit_link_request(shared, from, out, link_id, &mut pkt) {
                BridgeAdmission::Refused => {
                    shared
                        .routing_stats
                        .policy_rejected
                        .fetch_add(1, Ordering::Relaxed);
                    return;
                }
                BridgeAdmission::New => admitted = Some(link_id),
                BridgeAdmission::Known => {}
            }
        }
        // A route carrying transit is in use, and RNS refreshes it (`Transport.py` 2113).
        shared.touch_path(dest);
        // A request that never left must not hold the slot it was given until its deadline.
        if !forward_on(shared, out, pkt, policy)
            && let Some(link_id) = admitted
        {
            shared.link_transport.lock().unwrap().remove(&link_id);
        }
    }
}

/// What [`admit_link_request`] made of a request.
enum BridgeAdmission {
    /// Drop it.
    Refused,
    /// Carry it; its bridge is new.
    New,
    /// Carry it; it retransmits a request already bridged.
    Known,
}

/// Prepare to carry a link request from `from` to `out`: lower its signalled MTU to what both
/// interfaces carry, and record an unproved bridge with a proof deadline of the per-hop
/// allowance for each hop still ahead. Refuses a request whose MTU must be lowered under a
/// link mode this node cannot encode, or a full table of recently heard validated links.
fn admit_link_request(
    shared: &Shared,
    from: InterfaceId,
    out: InterfaceId,
    link_id: AddressHash,
    pkt: &mut Packet,
) -> BridgeAdmission {
    let limit = [from, out]
        .into_iter()
        .filter_map(|iface| shared.link_mtu_on(iface))
        .min();
    if let Some(limit) = limit
        && link::clamp_request_mtu(pkt, limit).is_err()
    {
        return BridgeAdmission::Refused;
    }
    let hops = shared
        .path_table
        .lock()
        .unwrap()
        .get(&pkt.destination)
        .map_or(0, |entry| entry.hops);
    let now = Instant::now();
    let bridge = LinkBridge {
        from,
        out,
        destination: pkt.destination,
        seen: now,
        proof_deadline: Some(now + Duration::from_millis(crate::node::transit_proof_timeout(hops))),
    };
    let mut bridges = shared.link_transport.lock().unwrap();
    // Prune before inserting, so the work tracks the requests that cause growth.
    bridges.retain(|_, bridge| !bridge.lapsed(now));
    if let Some(existing) = bridges.get_mut(&link_id) {
        // A retransmitted request: an unproved bridge follows it, a validated one stands.
        if existing.proof_deadline.is_some() {
            *existing = bridge;
        }
        return BridgeAdmission::Known;
    }
    if bridges.len() >= LINK_TRANSPORT_CAPACITY {
        let Some(stalest) = bridges
            .iter()
            .filter(|(_, bridge)| bridge.proof_deadline.is_some())
            .min_by_key(|(_, bridge)| bridge.seen)
            .or_else(|| {
                bridges
                    .iter()
                    .filter(|(_, bridge)| now.duration_since(bridge.seen) >= LINK_TRANSPORT_IDLE)
                    .min_by_key(|(_, bridge)| bridge.seen)
            })
            .map(|(id, _)| *id)
        else {
            return BridgeAdmission::Refused;
        };
        bridges.remove(&stalest);
    }
    bridges.insert(link_id, bridge);
    BridgeAdmission::New
}

/// Put a relayed announce on every permitted interface, counting it if it went anywhere.
fn relay_announce(
    shared: &Arc<Shared>,
    from: InterfaceId,
    pkt: Packet,
    egress: &InterfaceSelector,
) {
    if shared.broadcast_transit(from, pkt, egress) > 0 {
        shared
            .routing_stats
            .forwarded_announces
            .fetch_add(1, Ordering::Relaxed);
    }
}

/// Re-address a forwarded packet for the interface it leaves on (stripping our transport
/// stamp, so `send_on` re-adds the next hop's if there is one), bump hops, and send.
///
/// This is transit's single egress choke point: every packet carried for someone else leaves
/// through here, so the egress permission and the forwarded count are both enforced once.
fn forward_on(
    shared: &Arc<Shared>,
    out: InterfaceId,
    mut pkt: Packet,
    policy: &RoutingPolicy,
) -> bool {
    if !policy.allowed_egress.allows(out) {
        shared
            .routing_stats
            .policy_rejected
            .fetch_add(1, Ordering::Relaxed);
        return false;
    }
    if pkt.hops >= policy.max_hops {
        shared
            .routing_stats
            .hop_limit_dropped
            .fetch_add(1, Ordering::Relaxed);
        return false;
    }
    shared
        .routing_stats
        .forwarded_packets
        .fetch_add(1, Ordering::Relaxed);
    // Carried traffic queues as transit, so it can never outcompete this node's own.
    pkt.hops += 1;
    pkt.header_type = crate::packet::HeaderType::Type1;
    pkt.transport = None;
    shared.try_send_on_class(out, pkt, TrafficClass::Transit)
}

/// Register a link for endpoint-driven resource packets.
fn register_resource_session(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    liveness: Liveness,
    direction: LinkDirection,
    remote: LinkRemoteFact,
) -> Option<ResourceSession> {
    if !shared.begin_resource() {
        shared.send_on(iface, link.close_packet(&next_iv()));
        return None;
    }
    let (packet_tx, packets) = mpsc::channel(LINK_QUEUE);
    shared.write_diagnostic(|| {
        shared.links.lock().unwrap().insert(
            link.id(),
            LinkEntry {
                link: link.clone(),
                kind: LinkKind::Resource { packets: packet_tx },
                iface,
                direction,
                remote,
                liveness,
                lost: Arc::default(),
            },
        );
        ((), true)
    });
    Some(ResourceSession {
        shared: Arc::clone(shared),
        link,
        iface,
        packets,
        config: ResourceTransferConfig::default(),
        identified_peer: None,
    })
}

/// Build a [`LinkStream`] for a live link on `iface`, wiring the inbound feed and the
/// outbound relay, and register the link so the router can route to it.
fn register_stream(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    liveness: Liveness,
    direction: LinkDirection,
    remote: LinkRemoteFact,
) -> Option<LinkStream> {
    let (mine, theirs) = tokio::io::duplex(DUPLEX_BUF);
    let (mut read_half, mut write_half) = tokio::io::split(theirs);
    let (inbound_tx, mut inbound_rx) = mpsc::channel::<Vec<u8>>(LINK_QUEUE);
    let link_id = link.id();
    let write_chunk = write_chunk_for_mtu(link.mtu());
    let lost = Arc::new(AtomicBool::new(false));
    let fault = Arc::new(Mutex::new(None));

    shared.write_diagnostic(|| {
        shared.links.lock().unwrap().insert(
            link_id,
            LinkEntry {
                link: link.clone(),
                kind: LinkKind::BestEffort {
                    inbound: inbound_tx,
                    fault: Arc::clone(&fault),
                },
                iface,
                direction,
                remote,
                liveness,
                lost: Arc::clone(&lost),
            },
        );
        ((), true)
    });

    // Inbound: decrypted data from the router → the stream's read side.
    let inbound_started = track(shared, async move {
        while let Some(bytes) = inbound_rx.recv().await {
            if write_half.write_all(&bytes).await.is_err() {
                break;
            }
        }
        // The inbound channel closed: the link was torn down (a peer link-close, or
        // the endpoint shutting down). Shut the write side explicitly so the reader
        // sees EOF — dropping this half alone would not, since the outbound relay
        // still holds the duplex's read half alive.
        let _ = write_half.shutdown().await;
    });

    // Outbound: the stream's writes → encrypted link data packets, out the link's interface.
    // Drainable: it ends on its own when the stream is dropped, having read the
    // duplex to EOF, so an orderly shutdown can wait for exactly that.
    let out_link = link;
    let iv_shared = Arc::clone(shared);
    let out_lost = Arc::clone(&lost);
    let outbound_started = track_drainable(shared, async move {
        let mut buf = vec![0u8; write_chunk];
        loop {
            let read = read_half.read(&mut buf).await;
            // The watchdog dropped the link and already sent its close: stop, so the
            // writer sees a broken pipe instead of feeding a link nobody hears.
            if out_lost.load(Ordering::Acquire) {
                break;
            }
            match read {
                Ok(0) | Err(_) => {
                    // The stream was shut down or dropped: close the link so the
                    // peer's read side sees EOF. This is what lets a read-to-end
                    // protocol (e.g. gemini) end a response by closing the stream.
                    iv_shared.send_on(iface, out_link.close_packet(&next_iv()));
                    // A link close is final, so nothing more on this link needs routing:
                    // drop its entry (ending the inbound relay) and its inbound slot.
                    iv_shared.remove_link(link_id);
                    break;
                }
                Ok(n) => {
                    let iv = next_iv();
                    iv_shared.send_on(iface, out_link.data_packet(&buf[..n], &iv));
                }
            }
        }
    });

    if !inbound_started || !outbound_started {
        shared.remove_link(link_id);
        return None;
    }

    Some(LinkStream {
        inner: mine,
        receive_error: Some(fault),
        lost,
        link_id,
        iface,
    })
}

/// Build a **reliable** [`LinkStream`] for a live link: the RNS Channel/Buffer path with
/// link-proof acks (see [`crate::reliable`]). A single driver task owns the
/// [`ReliableChannel`] and pumps it — app writes in, ordered bytes out, a proof per
/// delivered packet, an inbound proof releasing its sequence, and retransmits on a clock —
/// so the stream stays honest over a lossy interface. `peer` is the remote identity: `Some`
/// for an initiator (the destination's identity from its announce), `None` for a responder,
/// which learns the initiator's identity from the IDENTIFY the initiator sends. Proofs are
/// validated against the link's own peer key either way. An initiator sends its IDENTIFY so
/// the responder learns who it is.
///
/// The channel times its retransmits by the RTT `liveness` measured at setup (RNS times its
/// channel by the link's RTT), with the configured estimate as a floor; a responder takes
/// the RTT from the initiator's RTT packet once it arrives.
fn register_reliable_stream(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    liveness: Liveness,
    peer: Option<Identity>,
    direction: LinkDirection,
    remote: LinkRemoteFact,
) -> Option<LinkStream> {
    let (mine, theirs) = tokio::io::duplex(DUPLEX_BUF);
    let (mut read_half, mut write_half) = tokio::io::split(theirs);
    let (pkt_tx, mut pkt_rx) = mpsc::channel::<Packet>(LINK_QUEUE);
    let link_id = link.id();
    let lost = Arc::new(AtomicBool::new(false));
    // The configured RTT is a floor under the one measured at setup: a responder has no
    // measurement yet, and a slow radio's data turnaround can exceed its setup round trip.
    let initial_rtt_ms = shared
        .reliable_initial_rtt_ms
        .load(Ordering::Relaxed)
        .max(liveness.rtt().unwrap_or(0));

    shared.write_diagnostic(|| {
        shared.links.lock().unwrap().insert(
            link_id,
            LinkEntry {
                link: link.clone(),
                kind: LinkKind::Reliable { packets: pkt_tx },
                iface,
                direction,
                remote,
                liveness,
                lost: Arc::clone(&lost),
            },
        );
        ((), true)
    });

    // An initiator (known peer) identifies itself so the responder learns who it is.
    // Each send is sealed under a fresh IV, so a re-send is a new packet with a new hash and
    // the responder's duplicate window does not count it (Ruling 72).
    let identify_link = peer.is_some().then(|| link.clone());
    let close_link = link.clone();
    let max_window = shared.reliable_max_window.load(Ordering::Relaxed);
    let decoded_frame_limit = shared.reliable_decoded_frame_limit.load(Ordering::Relaxed);
    let receive_error = Arc::new(Mutex::new(None));
    // App bytes read but not yet accepted by the bounded send queue, and whether the eof
    // frame still needs queueing. Holding these is what keeps backpressure from silently
    // becoming data loss.
    let mut pending: Vec<u8> = Vec::new();
    let mut finish_pending = false;
    let mut rc: ReliableChannel = match peer {
        Some(p) => ReliableChannel::new_with_initial_rtt_and_max_window(
            link,
            shared.identity.clone(),
            p,
            initial_rtt_ms,
            max_window,
        ),
        None => ReliableChannel::accepting_with_initial_rtt_and_max_window(
            link,
            shared.identity.clone(),
            initial_rtt_ms,
            max_window,
        ),
    };
    rc.set_decoded_frame_limit(decoded_frame_limit)
        .expect("endpoint validates the decoded frame limit");
    let driver_receive_error = Arc::clone(&receive_error);
    let drv = Arc::clone(shared);
    // A responder's proof has just gone out; the initiator's RTT packet answers it.
    let registered_at = Instant::now();
    let driver_started = track_drainable(shared, async move {
        // Identify to the responder so it learns who we are. RNS sends this once; we
        // re-send it over the first few ticks (in the clock arm below) so a dropped one still
        // lands on a lossy medium.
        if let Some(id_link) = &identify_link {
            drv.send_on(iface, id_link.identify_packet(&drv.identity, &next_iv()));
        }
        let mut identify_sends: u32 = 1;
        let mut buf = [0u8; WRITE_CHUNK];
        // The reliable channel measures RTT and sizes its retransmit timeout in this clock's
        // unit, and its RTT tiers are calibrated in milliseconds, so advance the clock by the
        // real tick period (below) rather than by 1 — otherwise the timeout is off by a factor
        // of RELIABLE_TICK_MS and either storms or stalls the medium.
        let mut clock: u64 = 0;
        let mut writer_open = true; // the app's write side is still open
        let mut peer_done = false; // the peer signalled end-of-stream (its eof frame)
        let mut interval = tokio::time::interval(Duration::from_millis(RELIABLE_TICK_MS));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

        'driver: loop {
            tokio::select! {
                // Raw inbound packets from the router: channel data (prove + deliver), an
                // ack (release its sequence), or the peer's link close.
                maybe = pkt_rx.recv() => {
                    let Some(pkt) = maybe else { break }; // router dropped the link
                    if pkt.packet_type == PacketType::Proof {
                        rc.on_proof(&pkt, clock);
                    } else if pkt.context == CTX_CHANNEL {
                        if let Some(proof) = rc.on_data_packet(&pkt) {
                            drv.send_on(iface, proof);
                        }
                        // One channel frame can expand beyond the Buffer read bound.
                        // Drain every available chunk before considering the peer's eof;
                        // another packet need not arrive to wake this task again.
                        loop {
                            let bytes = rc.read();
                            if bytes.is_empty() {
                                break;
                            }
                            if write_half.write_all(&bytes).await.is_err() {
                                break 'driver;
                            }
                        }
                        if let Some(error) = rc.receive_error() {
                            *driver_receive_error.lock().unwrap() = Some(error.into());
                            drv.send_on(iface, close_link.close_packet(&next_iv()));
                            break 'driver;
                        }
                        if !peer_done && rc.recv_finished() {
                            // The peer's stream ended: close our read side so the app's
                            // reader sees EOF. We keep running to finish our own sending.
                            let _ = write_half.shutdown().await;
                            peer_done = true;
                        }
                    } else if pkt.context == CTX_LINKIDENTIFY {
                        // The peer (an initiator) identified itself: learn its identity, which
                        // also validates proofs from older retinue initiators.
                        if rc.on_identify(&pkt)
                            && let Some(identity) = rc.peer().copied()
                        {
                            let present = drv.write_diagnostic(|| {
                                let mut links = drv.links.lock().unwrap();
                                let Some(entry) = links.get_mut(&link_id) else {
                                    return (false, false);
                                };
                                let changed = if entry.remote.identity == Some(identity) {
                                    false
                                } else {
                                    entry.remote.identity = Some(identity);
                                    true
                                };
                                (true, changed)
                            });
                            if !present {
                                break;
                            }
                        }
                    } else if pkt.context == link::CTX_LRRTT {
                        // Time the channel by the link's RTT, as RNS does, not a guess.
                        let measured = registered_at.elapsed().as_millis();
                        rc.on_rtt_packet(&pkt, u64::try_from(measured).unwrap_or(u64::MAX));
                    } else if pkt.context == CTX_LINKCLOSE
                        && close_link.receive(&pkt) == Some(Inbound::Close)
                    {
                        // Only a close that decrypts to the link id is the peer's: anyone
                        // can put this context on a packet addressed to the link.
                        let _ = write_half.shutdown().await;
                        break;
                    }
                }
                // App writes -> the reliable send queue. Disabled once the writer closes, so
                // we do not spin on end-of-stream.
                // Only read when the last write was fully accepted; otherwise we would pull
                // more from the app than the bounded queue can hold.
                res = read_half.read(&mut buf), if writer_open && pending.is_empty() => {
                    match res {
                        Ok(0) | Err(_) => {
                            finish_pending = true; // retried below until the queue takes it
                            writer_open = false;
                        }
                        Ok(n) => {
                            let accepted = rc.write(&buf[..n]);
                            if accepted < n {
                                pending.extend_from_slice(&buf[accepted..n]);
                            }
                        }
                    }
                }
                // The retransmit clock, in milliseconds (one tick = RELIABLE_TICK_MS real time).
                _ = interval.tick() => {
                    clock += RELIABLE_TICK_MS;
                    // Re-send IDENTIFY over the first few ticks so a dropped one still reaches
                    // the responder on a lossy medium (bounded; there is no ack to wait on).
                    if let Some(id_link) = &identify_link
                        && identify_sends < IDENTIFY_MAX_SENDS
                    {
                        drv.send_on(iface, id_link.identify_packet(&drv.identity, &next_iv()));
                        identify_sends += 1;
                    }
                }
            }

            // The send queue is bounded, so an earlier write or eof may have been refused.
            // Retry before transmitting, so anything accepted now goes out on this pass.
            if !pending.is_empty() {
                let accepted = rc.write(&pending);
                pending.drain(..accepted);
            }
            if finish_pending && pending.is_empty() && rc.finish() {
                finish_pending = false;
            }

            // After any event, put ready channel packets on the wire: new data within the
            // window, plus retransmits past their timeout.
            for pkt in rc.poll_transmit(clock, next_iv) {
                drv.send_on(iface, pkt);
            }
            // A packet went unproved through every try: the peer is gone. Fail the stream
            // and close the link, as RNS tears its link down when the channel times out.
            if rc.channel_error().is_some() {
                *driver_receive_error.lock().unwrap() = Some(StreamFault::Unacknowledged);
                drv.send_on(iface, close_link.close_packet(&next_iv()));
                break;
            }

            // The stream is fully done only when our side finished sending (write closed and
            // everything, including our eof frame, sent and proven) AND the peer finished
            // sending (its eof arrived). This preserves half-close: after our write closes we
            // keep delivering the peer's reply until it, too, ends. Then close the link.
            // `finish_pending` and `pending` must be clear too: the writer closing is not the
            // same as the queue having accepted everything, now that it can refuse.
            if !writer_open && pending.is_empty() && !finish_pending && peer_done && rc.send_idle()
            {
                drv.send_on(iface, close_link.close_packet(&next_iv()));
                break;
            }
        }
        drv.remove_link(link_id);
    });

    if !driver_started {
        shared.remove_link(link_id);
        return None;
    }

    Some(LinkStream {
        inner: mine,
        receive_error: Some(receive_error),
        lost,
        link_id,
        iface,
    })
}

/// Spawn a task and record its abort handle on `shared`, so the endpoint's drop can cancel
/// every task it started. Every `tokio::spawn` in this module goes through here; a task that
/// is not tracked would outlive the endpoint.
fn track<F>(shared: &Arc<Shared>, fut: F) -> bool
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
fn track_drainable<F>(shared: &Arc<Shared>, fut: F) -> bool
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

/// Removes a link's pending-setup state — the `pending` waker and the `pending_links`
/// half-open link — if setup does not complete: a timeout, or the caller dropping the `open`
/// future. Without it, a setup that never receives its proof leaks both entries. Disarmed
/// once the proof establishes the link, since the router has already removed them.
struct PendingGuard {
    shared: Arc<Shared>,
    link_id: AddressHash,
    armed: bool,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shared.pending.lock().unwrap().remove(&self.link_id);
            self.shared
                .pending_links
                .lock()
                .unwrap()
                .remove(&self.link_id);
        }
    }
}

/// Fill `buf` with cryptographically secure OS randomness. Link ephemeral secrets and AES
/// IVs depend on this being unpredictable — the whole link's secrecy rests on the ephemeral
/// key an eavesdropper must not be able to guess — so a failure to obtain entropy is fatal:
/// this panics rather than hand back weak bytes.
fn fill_random(buf: &mut [u8]) {
    getrandom::getrandom(buf).expect("OS CSPRNG unavailable");
}

/// Whole seconds from the host clock for a local announce ordinal.
///
/// [`TimebaseGenerator`] prevents a backward or repeated source clock from reusing an ordinal.
/// A host clock before the Unix epoch cannot supply the required non-negative wire value.
fn host_announce_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("host clock is before the Unix epoch")
        .as_secs()
}

/// A fresh 64-byte link ephemeral seed (`x25519_secret(32) || ed25519_seed(32)`), unique and
/// unpredictable per link.
fn ephemeral_seed() -> [u8; 64] {
    let mut seed = [0u8; 64];
    fill_random(&mut seed);
    seed
}

/// A fresh AES-CBC IV. Must be unpredictable per packet under a given link key.
fn next_iv() -> [u8; IV_LEN] {
    let mut iv = [0u8; IV_LEN];
    fill_random(&mut iv);
    iv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "compression")]
    #[tokio::test]
    async fn oversized_reliable_frame_closes_without_proof_and_errors_at_host_reader() {
        use crate::channel::{Envelope, STREAM_MSGTYPE, StreamFrame};
        use crate::link::{PendingLink, accept};

        let server_id = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
        let client_id = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
        let endpoint = Endpoint::new(server_id.clone());
        endpoint.set_reliable_decoded_frame_limit(32).unwrap();
        let mut iface = endpoint.attach_interface();
        let dest =
            DestinationName::new("retinue", ["decode-limit"]).destination_hash(server_id.public());
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x63; 64], trailer);
        let (server_link, proof) = accept(&request, &server_id, &[0x64; 64], trailer).unwrap();
        let client_link = pending.prove(&proof).unwrap();
        let mut stream = register_reliable_stream(
            &endpoint.shared,
            server_link,
            iface.id(),
            Liveness::responder(0, 0),
            None,
            LinkDirection::Inbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(*client_id.public()),
            },
        )
        .unwrap();

        let wire_frame = |sequence: u16, frame: StreamFrame| {
            client_link.sealed_packet(
                CTX_CHANNEL,
                &Envelope {
                    msgtype: STREAM_MSGTYPE,
                    sequence,
                    payload: frame.encode(),
                }
                .encode(),
                &[sequence as u8 + 1; IV_LEN],
            )
        };
        let prefix = wire_frame(
            0,
            StreamFrame {
                stream_id: 0,
                eof: false,
                compressed: false,
                data: b"prefix".to_vec(),
            },
        );
        let oversized = wire_frame(
            1,
            StreamFrame {
                stream_id: 0,
                eof: true,
                compressed: true,
                data: crate::resource::compress(&vec![b'x'; 100_000]),
            },
        );
        let sink = iface.sink();
        assert!(sink.deliver(prefix));
        assert!(sink.deliver(oversized));

        let mut got = Vec::new();
        let err = tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut got))
            .await
            .expect("driver closes failed stream")
            .unwrap_err();
        assert_eq!(got, b"prefix", "already decoded bytes reach the caller");
        assert_eq!(
            err.kind(),
            io::ErrorKind::InvalidData,
            "no healthy EOF after decode failure"
        );

        let mut proof_count = 0;
        loop {
            let packet = tokio::time::timeout(Duration::from_secs(2), iface.next_outbound())
                .await
                .expect("driver emits a close packet")
                .unwrap();
            if packet.context == CTX_LINKCLOSE {
                break;
            }
            if packet.packet_type == PacketType::Proof {
                proof_count += 1;
            }
        }
        assert_eq!(proof_count, 1, "the oversized frame itself is not proved");
    }

    /// A LINKCLOSE is the peer's only if it decrypts to the link id. One with the right
    /// context and address but a garbage payload is anybody's, and must not end the stream;
    /// the genuine close still does.
    #[tokio::test]
    async fn a_forged_link_close_leaves_a_reliable_stream_open() {
        use crate::channel::{Envelope, STREAM_MSGTYPE, StreamFrame};
        use crate::link::{PendingLink, accept};

        let server_id = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
        let endpoint = Endpoint::new(server_id.clone());
        let iface = endpoint.attach_interface();
        let dest =
            DestinationName::new("retinue", ["forged-close"]).destination_hash(server_id.public());
        let trailer = LinkTrailer {
            mode: LinkMode::Aes256Cbc,
            mtu: 500,
        };
        let (pending, request) = PendingLink::open(dest, *server_id.public(), &[0x73; 64], trailer);
        let (server_link, proof) = accept(&request, &server_id, &[0x74; 64], trailer).unwrap();
        let client_link = pending.prove(&proof).unwrap();
        let mut stream = register_reliable_stream(
            &endpoint.shared,
            server_link,
            iface.id(),
            Liveness::responder(0, 0),
            None,
            LinkDirection::Inbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: None,
            },
        )
        .unwrap();
        let sink = iface.sink();

        let forged = client_link.framed_packet(CTX_LINKCLOSE, vec![0xA5; 48]);
        assert!(sink.deliver(forged));
        let frame = client_link.sealed_packet(
            CTX_CHANNEL,
            &Envelope {
                msgtype: STREAM_MSGTYPE,
                sequence: 0,
                payload: StreamFrame {
                    stream_id: 0,
                    eof: false,
                    compressed: false,
                    data: b"still open".to_vec(),
                }
                .encode(),
            }
            .encode(),
            &[0x01; IV_LEN],
        );
        assert!(sink.deliver(frame));
        let mut got = [0u8; 10];
        tokio::time::timeout(Duration::from_secs(2), stream.read_exact(&mut got))
            .await
            .expect("the stream still delivers")
            .expect("the forged close did not end the stream");
        assert_eq!(&got, b"still open");

        assert!(sink.deliver(client_link.close_packet(&[0x02; IV_LEN])));
        let mut rest = Vec::new();
        tokio::time::timeout(Duration::from_secs(2), stream.read_to_end(&mut rest))
            .await
            .expect("a genuine close ends the stream")
            .unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn reliable_read_keeps_prefix_then_wakes_with_decode_error() {
        let (mine, mut driver_half) = tokio::io::duplex(64);
        let error = Arc::new(Mutex::new(None));
        let mut stream = LinkStream {
            inner: mine,
            receive_error: Some(Arc::clone(&error)),
            lost: Arc::default(),
            link_id: AddressHash::from_bytes([7; 16]),
            iface: 0,
        };
        driver_half.write_all(b"prefix").await.unwrap();
        let (pending_tx, pending_rx) = oneshot::channel();
        let reader = tokio::spawn(async move {
            let mut got = Vec::new();
            let mut pending_tx = Some(pending_tx);
            let mut chunk = [0u8; 64];
            loop {
                let result: io::Result<usize> = std::future::poll_fn(|cx| {
                    let mut read_buf = ReadBuf::new(&mut chunk);
                    match Pin::new(&mut stream).poll_read(cx, &mut read_buf) {
                        Poll::Pending => {
                            if let Some(tx) = pending_tx.take() {
                                let _ = tx.send(());
                            }
                            Poll::Pending
                        }
                        Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
                        Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                    }
                })
                .await;
                match result {
                    Ok(0) => break (Ok(()), got),
                    Ok(n) => got.extend_from_slice(&chunk[..n]),
                    Err(error) => break (Err(error), got),
                }
            }
        });
        tokio::time::timeout(Duration::from_secs(1), pending_rx)
            .await
            .expect("reader reaches Pending after prefix")
            .unwrap();
        *error.lock().unwrap() =
            Some(StreamDecodeError::DecodedFrameLimitExceeded { limit: 32 }.into());
        drop(driver_half); // closing the duplex half wakes an already-pending reader
        let (result, got) = tokio::time::timeout(Duration::from_secs(1), reader)
            .await
            .expect("pending reader woke")
            .unwrap();
        assert_eq!(got, b"prefix");
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn a_link_bridge_accepts_only_its_two_interfaces() {
        let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x93; 64]));
        let a = endpoint.attach_interface();
        let b = endpoint.attach_interface();
        let c = endpoint.attach_interface();
        endpoint.enable_routing();

        let link_id = AddressHash::from_bytes([0xA3; 16]);
        endpoint.shared.link_transport.lock().unwrap().insert(
            link_id,
            LinkBridge {
                from: a.id(),
                out: b.id(),
                destination: AddressHash::from_bytes([0xA4; 16]),
                seen: Instant::now(),
                proof_deadline: None,
            },
        );
        let packet = Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Transport,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: link_id,
            context: 0,
            payload: b"bridged data".to_vec(),
        };

        route(&endpoint.shared, a.id(), packet.clone());
        let to_b = b
            .outbound
            .queues
            .pop()
            .expect("first bridge end forwards to second");
        assert_eq!(to_b.destination, link_id);
        assert_eq!(to_b.hops, 1);
        b.outbound.queues.delivery_complete();

        route(&endpoint.shared, b.id(), packet.clone());
        let to_a = a
            .outbound
            .queues
            .pop()
            .expect("second bridge end forwards to first");
        assert_eq!(to_a.destination, link_id);
        assert_eq!(to_a.hops, 1);
        a.outbound.queues.delivery_complete();

        let seen_before_foreign = endpoint.shared.link_transport.lock().unwrap()[&link_id].seen;
        route(&endpoint.shared, c.id(), packet);
        assert!(a.outbound.queues.pop().is_none());
        assert!(b.outbound.queues.pop().is_none());
        assert!(c.outbound.queues.pop().is_none());
        assert_eq!(
            endpoint.shared.link_transport.lock().unwrap()[&link_id].seen,
            seen_before_foreign
        );
        let counters = endpoint.routing_counters();
        assert_eq!(counters.forwarded_packets, 2);
        assert_eq!(counters.policy_rejected, 1);
    }

    /// A routing endpoint with a radio side `a` and an IFAC'd 255-byte side `b`, a learned
    /// destination behind `b`, and that destination's link request, addressed through the
    /// endpoint and asking for 500 bytes.
    fn transit_fixture() -> (
        Endpoint,
        Interface,
        Interface,
        PrivateIdentity,
        link::PendingLink,
        Packet,
    ) {
        let endpoint = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x94; 64]));
        let a = endpoint.attach_interface();
        let ifac = Ifac::with_default_size(Some("transit"), None).unwrap();
        let b = endpoint.attach_interface_with_ifac(255, ifac).unwrap();
        endpoint.enable_routing();

        let responder = PrivateIdentity::from_secret_bytes(&[0x95; 64]);
        let (_, announce) = freshness_announce(&responder, "transit", 0, 1, 1, 0);
        endpoint
            .shared
            .address_book
            .lock()
            .unwrap()
            .ingest(&announce);
        endpoint.shared.path_table.lock().unwrap().insert(
            announce.destination,
            PathEntry {
                iface: b.id(),
                transport: None,
                hops: 0,
                learned: Instant::now(),
                mode: InterfaceMode::default(),
            },
        );
        let (pending, mut request) = link::PendingLink::open(
            announce.destination,
            *responder.public(),
            &[0x96; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        request.header_type = crate::packet::HeaderType::Type2;
        request.transport = Some(endpoint.identity().hash());
        (endpoint, a, b, responder, pending, request)
    }

    fn link_data(link_id: AddressHash) -> Packet {
        Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Broadcast,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: link_id,
            context: 0,
            payload: b"link data".to_vec(),
        }
    }

    /// A carried request is clamped to the smaller side less its access code (255 - 8), and
    /// keeps its link id. Its bridge waits for the proof, with the per-hop deadline.
    #[tokio::test]
    async fn a_carried_link_request_is_clamped_to_both_sides() {
        let (endpoint, a, b, _, pending, request) = transit_fixture();
        route(&endpoint.shared, a.id(), request);
        let carried = b.outbound.queues.pop().expect("the request is carried");
        assert_eq!(link::link_id(&carried).unwrap(), pending.link_id());
        assert_eq!(
            link::request_trailer(&carried).unwrap().map(|t| t.mtu),
            Some(247)
        );
        let bridge = endpoint.shared.link_transport.lock().unwrap()[&pending.link_id()];
        let allowance = bridge.proof_deadline.unwrap() - bridge.seen;
        assert_eq!(
            allowance,
            Duration::from_millis(crate::node::LINK_ESTABLISHMENT_TIMEOUT_PER_HOP)
        );
    }

    /// A request whose MTU must be lowered under a link mode this node cannot encode is
    /// dropped and takes no slot; one that already fits is carried as it is.
    #[tokio::test]
    async fn a_carried_request_that_cannot_be_clamped_is_dropped() {
        let (endpoint, a, b, _, pending, mut request) = transit_fixture();
        request.payload[link::LINK_KEYS_LEN] = 0xe0;
        route(&endpoint.shared, a.id(), request.clone());
        assert!(b.outbound.queues.pop().is_none());
        assert!(endpoint.shared.link_transport.lock().unwrap().is_empty());

        let trailer = request.payload.len() - link::TRAILER_LEN;
        request.payload[trailer..].copy_from_slice(&[0xe0, 0, 200]);
        route(&endpoint.shared, a.id(), request);
        let carried = b
            .outbound
            .queues
            .pop()
            .expect("a fitting request is carried");
        assert_eq!(carried.payload[trailer..], [0xe0, 0, 200]);
        assert!(
            endpoint
                .shared
                .link_transport
                .lock()
                .unwrap()
                .contains_key(&pending.link_id())
        );
    }

    /// With the destination's identity unknown, the proof cannot be checked; it is carried
    /// only from the destination's side.
    #[tokio::test]
    async fn without_the_destinations_identity_only_its_side_proves() {
        let (endpoint, a, b, responder, pending, request) = transit_fixture();
        endpoint
            .shared
            .address_book
            .lock()
            .unwrap()
            .forget(request.destination);
        route(&endpoint.shared, a.id(), request);
        let carried = b.outbound.queues.pop().unwrap();
        b.outbound.queues.delivery_complete();
        let trailer = link::request_trailer(&carried).unwrap().unwrap();
        let (_, proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();

        route(&endpoint.shared, a.id(), proof.clone());
        assert!(a.outbound.queues.pop().is_none() && b.outbound.queues.pop().is_none());
        route(&endpoint.shared, b.id(), proof);
        assert!(a.outbound.queues.pop().is_some());
        let bridge = endpoint.shared.link_transport.lock().unwrap()[&pending.link_id()];
        assert!(bridge.proof_deadline.is_none());
    }

    /// A proof this node does not send on, here for the hop ceiling, leaves its bridge
    /// unvalidated; RNS marks a bridge validated only as it transmits the proof.
    #[tokio::test]
    async fn a_proof_validates_its_bridge_only_once_carried() {
        let (endpoint, a, b, responder, pending, request) = transit_fixture();
        route(&endpoint.shared, a.id(), request);
        let carried = b.outbound.queues.pop().unwrap();
        b.outbound.queues.delivery_complete();
        let trailer = link::request_trailer(&carried).unwrap().unwrap();
        let (_, mut proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();
        proof.hops = MAX_HOPS;
        route(&endpoint.shared, b.id(), proof);
        assert!(a.outbound.queues.pop().is_none());
        assert_eq!(endpoint.routing_counters().hop_limit_dropped, 1);
        let bridge = endpoint.shared.link_transport.lock().unwrap()[&pending.link_id()];
        assert!(bridge.proof_deadline.is_some());
    }

    /// Nothing crosses a carried link before its destination proves it: early data, a proof
    /// signed by another identity, and the real proof heard from the initiator's side are all
    /// dropped. The real proof from the destination's side validates the bridge.
    #[tokio::test]
    async fn a_carried_link_waits_for_its_destinations_proof() {
        let (endpoint, a, b, responder, pending, request) = transit_fixture();
        let link_id = pending.link_id();
        route(&endpoint.shared, a.id(), request);
        let carried = b.outbound.queues.pop().unwrap();
        b.outbound.queues.delivery_complete();

        route(&endpoint.shared, a.id(), link_data(link_id));
        let trailer = link::request_trailer(&carried).unwrap().unwrap();
        let impostor = PrivateIdentity::from_secret_bytes(&[0x97; 64]);
        let (_, forged) = link::accept(&carried, &impostor, &[0x98; 64], trailer).unwrap();
        route(&endpoint.shared, b.id(), forged);
        let (_, proof) = link::accept(&carried, &responder, &[0x99; 64], trailer).unwrap();
        route(&endpoint.shared, a.id(), proof.clone());
        assert!(a.outbound.queues.pop().is_none());
        assert!(b.outbound.queues.pop().is_none());
        assert_eq!(endpoint.routing_counters().policy_rejected, 3);

        route(&endpoint.shared, b.id(), proof);
        let proved = a.outbound.queues.pop().expect("the proof is carried back");
        a.outbound.queues.delivery_complete();
        let link = pending.prove(&proved).expect("the initiator accepts it");
        assert_eq!(link.mtu(), 247);

        route(&endpoint.shared, a.id(), link_data(link_id));
        assert!(
            b.outbound.queues.pop().is_some(),
            "validated links carry data"
        );
    }

    /// At capacity a request displaces the stalest unproved link; with every slot validated,
    /// it is refused and not carried.
    #[tokio::test]
    async fn a_full_transit_table_never_evicts_a_validated_link() {
        let (endpoint, a, b, responder, pending, request) = transit_fixture();
        let now = Instant::now();
        let validated = |byte| {
            (
                AddressHash::from_bytes([byte; 16]),
                LinkBridge {
                    from: a.id(),
                    out: b.id(),
                    destination: AddressHash::from_bytes([0; 16]),
                    seen: now,
                    proof_deadline: None,
                },
            )
        };
        endpoint
            .shared
            .link_transport
            .lock()
            .unwrap()
            .extend((1..=LINK_TRANSPORT_CAPACITY as u8).map(validated));

        route(&endpoint.shared, a.id(), request.clone());
        assert!(b.outbound.queues.pop().is_none());
        assert_eq!(endpoint.routing_counters().policy_rejected, 1);
        assert!(
            !endpoint
                .shared
                .link_transport
                .lock()
                .unwrap()
                .contains_key(&pending.link_id())
        );

        let unproved = AddressHash::from_bytes([1; 16]);
        endpoint
            .shared
            .link_transport
            .lock()
            .unwrap()
            .get_mut(&unproved)
            .unwrap()
            .proof_deadline = Some(now + LINK_TRANSPORT_TTL);
        // A fresh request: a byte-identical retransmission of the refused one is now a
        // duplicate, filtered before transit as RNS's packet hashlist filters it.
        let (fresh, mut fresh_request) = link::PendingLink::open(
            request.destination,
            *responder.public(),
            &[0x97; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        fresh_request.header_type = crate::packet::HeaderType::Type2;
        fresh_request.transport = request.transport;
        route(&endpoint.shared, a.id(), fresh_request);
        assert!(b.outbound.queues.pop().is_some());
        let bridges = endpoint.shared.link_transport.lock().unwrap();
        assert_eq!(bridges.len(), LINK_TRANSPORT_CAPACITY);
        assert!(bridges.contains_key(&fresh.link_id()));
        assert!(!bridges.contains_key(&unproved));
    }

    /// With every slot validated, a link unheard for the RNS link timeout yields to a new
    /// request, so finished links cannot lock a transport node out of carrying more.
    #[tokio::test]
    async fn an_idle_validated_link_yields_to_a_new_request() {
        let (endpoint, a, b, _, pending, request) = transit_fixture();
        let now = Instant::now();
        let Some(idle_since) = now.checked_sub(LINK_TRANSPORT_IDLE) else {
            return; // A monotonic clock this young cannot express an idle link.
        };
        let validated = |byte: u8, seen| {
            (
                AddressHash::from_bytes([byte; 16]),
                LinkBridge {
                    from: a.id(),
                    out: b.id(),
                    destination: AddressHash::from_bytes([0; 16]),
                    seen,
                    proof_deadline: None,
                },
            )
        };
        endpoint.shared.link_transport.lock().unwrap().extend(
            (1..=LINK_TRANSPORT_CAPACITY as u8)
                .map(|byte| validated(byte, if byte == 1 { idle_since } else { now })),
        );
        route(&endpoint.shared, a.id(), request);
        assert!(b.outbound.queues.pop().is_some());
        let bridges = endpoint.shared.link_transport.lock().unwrap();
        assert_eq!(bridges.len(), LINK_TRANSPORT_CAPACITY);
        assert!(bridges.contains_key(&pending.link_id()));
        assert!(!bridges.contains_key(&AddressHash::from_bytes([1; 16])));
    }

    /// A destination answers with no more than the interface that heard the request carries.
    #[tokio::test]
    async fn a_responder_clamps_the_link_to_its_receiving_interface() {
        let identity = PrivateIdentity::from_secret_bytes(&[0x9A; 64]);
        let endpoint = Endpoint::new(identity.clone());
        let name = DestinationName::new("retinue", ["clamped"]);
        endpoint.register(name.clone(), b"");
        let narrow = endpoint.attach_interface_with_frame_limit(300).unwrap();
        let (pending, request) = link::PendingLink::open(
            name.destination_hash(identity.public()),
            *identity.public(),
            &[0x9B; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: 500,
            },
        );
        route(&endpoint.shared, narrow.id(), request);
        let proof = narrow.outbound.queues.pop().expect("the request is proved");
        assert_eq!(pending.prove(&proof).unwrap().mtu(), 300);
    }

    /// A signalled MTU of 0 means RNS's default, and a request below the smallest workable
    /// link is held to that floor.
    #[tokio::test]
    async fn a_responder_floors_what_it_offers() {
        let identity = PrivateIdentity::from_secret_bytes(&[0x9C; 64]);
        let endpoint = Endpoint::new(identity.clone());
        let name = DestinationName::new("retinue", ["floored"]);
        endpoint.register(name.clone(), b"");
        let iface = endpoint.attach_interface();
        for (seed, signalled, offered) in [
            (0x9D, 0, endpoint.shared.link_mtu.load(Ordering::Relaxed)),
            (0x9E, 1, crate::node::MIN_LOGICAL_MTU),
        ] {
            let (_, mut request) = link::PendingLink::open(
                name.destination_hash(identity.public()),
                *identity.public(),
                &[seed; 64],
                LinkTrailer {
                    mode: LinkMode::Aes256Cbc,
                    mtu: 500,
                },
            );
            let trailer = request.payload.len() - link::TRAILER_LEN;
            request.payload[trailer..].copy_from_slice(
                &LinkTrailer {
                    mode: LinkMode::Aes256Cbc,
                    mtu: signalled,
                }
                .encode(),
            );
            route(&endpoint.shared, iface.id(), request);
            let proof = iface.outbound.queues.pop().expect("the request is proved");
            iface.outbound.queues.delivery_complete();
            let echoed = &proof.payload[proof.payload.len() - link::TRAILER_LEN..];
            let echoed = LinkTrailer::decode(echoed.try_into().unwrap()).unwrap();
            assert_eq!(echoed.mtu, offered);
        }
    }

    fn freshness_announce(
        peer: &PrivateIdentity,
        destination_name: &str,
        context: u8,
        nonce: u8,
        timebase: u64,
        hops: u8,
    ) -> (Packet, Announce) {
        let blob = AnnounceBlob::mint([nonce; crate::announce::ANNOUNCE_NONCE_LEN], timebase)
            .expect("test timebase fits");
        let name = DestinationName::new("retinue", [destination_name]);
        let mut packet = announce::build(peer, name.name_hash(), &blob, None, b"freshness-test");
        packet.context = context;
        packet.hops = hops;
        let decoded = Announce::decode(&packet).expect("locally built announce verifies");
        (packet, decoded)
    }

    fn emitted_timebase(packet: &Packet) -> u64 {
        AnnounceBlob::from_wire(
            Announce::decode(packet)
                .expect("locally emitted announce verifies")
                .rand_hash,
        )
        .timebase()
    }

    #[tokio::test]
    async fn endpoint_announce_advances_within_one_source_second() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x90; 64]));
        let name = DestinationName::new("retinue", ["endpoint-same-second"]);

        let first = ep.build_announce_at(&name, b"cap", 4_000);
        let second = ep.build_announce_at(&name, b"cap", 4_000);

        assert_eq!(emitted_timebase(&first), 4_000);
        assert_eq!(emitted_timebase(&second), 4_001);
    }

    #[tokio::test]
    async fn endpoint_announce_ignores_a_backward_source_clock() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x91; 64]));
        let name = DestinationName::new("retinue", ["endpoint-backward-clock"]);

        let first = ep.build_announce_at(&name, b"cap", 9_000);
        let second = ep.build_announce_at(&name, b"cap", 8_999);

        assert_eq!(emitted_timebase(&first), 9_000);
        assert_eq!(emitted_timebase(&second), 9_001);
    }

    #[tokio::test]
    async fn endpoint_and_owned_path_response_keep_timebases_per_destination() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x92; 64]));
        let first_name = DestinationName::new("retinue", ["endpoint-first"]);
        let second_name = DestinationName::new("retinue", ["endpoint-second"]);
        let second_destination = second_name.destination_hash(ep.identity());
        ep.shared.registered.lock().unwrap().push(Registered {
            dest: second_destination,
            kind: RegistrationKind::BestEffort,
            name: second_name.clone(),
            app_data: b"path-cap".to_vec(),
            ratchets: None,
        });

        let first = ep.build_announce_at(&first_name, b"first-cap", 700);
        let path_response = ep
            .shared
            .path_response_at(second_destination, 700)
            .expect("owned destination answers a path request");
        let first_again = ep.build_announce_at(&first_name, b"first-cap", 700);
        let path_response_again = ep
            .shared
            .path_response_at(second_destination, 700)
            .expect("owned destination answers a second path request");

        assert_eq!(emitted_timebase(&first), 700);
        assert_eq!(emitted_timebase(&path_response), 700);
        assert_eq!(emitted_timebase(&first_again), 701);
        assert_eq!(emitted_timebase(&path_response_again), 701);
        assert_eq!(path_response.context, crate::path::CTX_PATH_RESPONSE);
    }

    /// The link packet memory holds its bound, forgets oldest first, and leaves alone the
    /// contexts whose repeats are legitimate: every keepalive request shares one hash, and a
    /// re-sent resource part repeats its own.
    #[test]
    fn link_packet_memory_is_bounded_and_skips_keepalives_and_resources() {
        let mut window = HashWindow::new(2);
        let [a, b, c] = [1u8, 2, 3].map(|n| AddressHash::from_bytes([n; 16]));
        assert!(window.insert(a) && window.insert(b) && !window.insert(a));
        assert!(window.insert(c));
        assert_eq!(window.order.len(), 2);
        assert!(!window.contains(&a), "the oldest is forgotten first");
        assert!(window.contains(&b) && window.contains(&c));

        let link_packet = |context| Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Broadcast,
            destination_type: DestinationType::Link,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: AddressHash::from_bytes([9; 16]),
            context,
            payload: vec![link::KEEPALIVE_REQUEST],
        };
        let mut memory = LinkPacketMemory::new();
        for context in [link::CTX_KEEPALIVE, link::CTX_RESOURCE] {
            let packet = link_packet(context);
            memory.note_sent(&packet);
            assert_eq!(memory.admit(&packet), LinkPacketAdmission::New);
            assert_eq!(memory.admit(&packet), LinkPacketAdmission::New);
        }
        let data = link_packet(0);
        assert_eq!(memory.admit(&data), LinkPacketAdmission::New);
        assert_eq!(memory.admit(&data), LinkPacketAdmission::Duplicate);
        let channel = link_packet(CTX_CHANNEL);
        memory.note_sent(&channel);
        assert_eq!(
            memory.admit(&channel),
            LinkPacketAdmission::OwnEcho,
            "our own Channel packet is ours"
        );
    }

    #[test]
    fn ifac_overhead_counts_against_interface_frame_admission() {
        let queues = Arc::new(OutboundQueues::new(
            QueueWeights::DEFAULT,
            QueueDepths::DEFAULT,
        ));
        let packet = Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Broadcast,
            destination_type: DestinationType::Plain,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: AddressHash::from_bytes([0x51; 16]),
            context: 0,
            payload: b"frame admission".to_vec(),
        };
        let actual = packet.encoded_len() + 8;
        let interface = Iface {
            id: 1,
            outbound: queues,
            frame_limit: Arc::new(AtomicUsize::new(actual - 1)),
            wire_overhead: 8,
            mode: InterfaceMode::Full,
        };

        assert_eq!(
            interface.push(packet, TrafficClass::Interactive),
            QueueAdmission::FrameLimit {
                actual,
                limit: actual - 1,
            }
        );
    }

    #[tokio::test]
    async fn a_learned_route_expires_and_is_evicted() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[1u8; 64]));
        let dest = AddressHash::from_bytes([0xAB; 16]);
        let learned = Instant::now();
        ep.shared.learn_path_at(dest, 7, 2, None, learned);
        assert_eq!(
            ep.route_to_at(dest, learned),
            Some((7, 2)),
            "a fresh route is returned"
        );

        assert_eq!(
            ep.route_to_at(dest, learned + ep.shared.route_ttl()),
            None,
            "an expired route is not returned"
        );
        assert!(
            !ep.shared.path_table.lock().unwrap().contains_key(&dest),
            "and is evicted on lookup",
        );
    }

    #[tokio::test]
    async fn route_facts_are_ordered_current_and_read_only() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x12; 64]));
        let interface = ep.attach_interface().id();
        let later = AddressHash::from_bytes([0xBB; 16]);
        let earlier = AddressHash::from_bytes([0x11; 16]);
        let transport = AddressHash::from_bytes([0x77; 16]);
        ep.shared.learn_path(later, interface, 3, Some(transport));
        ep.shared.learn_path(earlier, interface, 1, None);
        let learned = ep
            .shared
            .path_table
            .lock()
            .unwrap()
            .values()
            .map(|entry| entry.learned)
            .max()
            .unwrap();

        let facts = ep.route_facts_at(learned);
        assert_eq!(
            facts
                .iter()
                .map(|fact| fact.destination)
                .collect::<Vec<_>>(),
            vec![earlier, later]
        );
        assert!(facts.iter().all(|fact| fact.interface == interface));
        assert_eq!(facts[1].transport, Some(transport));

        let before = ep.shared.path_table.lock().unwrap().len();
        assert!(
            ep.route_facts_at(learned + ep.shared.route_ttl())
                .is_empty()
        );
        assert_eq!(
            ep.shared.path_table.lock().unwrap().len(),
            before,
            "diagnostic capture must not evict expired routes",
        );
    }

    #[tokio::test]
    async fn diagnostic_capture_waits_for_an_inflight_writer() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x19; 64]));
        let initial_generation = ep.diagnostic_generation();
        let destination = AddressHash::from_bytes([0x66; 16]);
        let (writer_entered_tx, writer_entered_rx) = std::sync::mpsc::channel();
        let (release_writer_tx, release_writer_rx) = std::sync::mpsc::channel();
        let writer_shared = Arc::clone(&ep.shared);
        let writer = std::thread::spawn(move || {
            writer_shared.write_diagnostic(|| {
                writer_entered_tx.send(()).unwrap();
                release_writer_rx.recv().unwrap();
                writer_shared.path_table.lock().unwrap().insert(
                    destination,
                    PathEntry {
                        iface: 0,
                        transport: None,
                        hops: 1,
                        learned: Instant::now(),
                        mode: InterfaceMode::Full,
                    },
                );
                ((), true)
            });
        });

        writer_entered_rx.recv().unwrap();
        let (capture_started_tx, capture_started_rx) = std::sync::mpsc::channel();
        let (capture_done_tx, capture_done_rx) = std::sync::mpsc::channel();
        let capture_shared = Arc::clone(&ep.shared);
        let capture = std::thread::spawn(move || {
            capture_started_tx.send(()).unwrap();
            let result = capture_shared
                .capture_diagnostic(|| capture_shared.path_table.lock().unwrap().len());
            capture_done_tx.send(result).unwrap();
        });

        capture_started_rx.recv().unwrap();
        assert!(
            capture_done_rx
                .recv_timeout(Duration::from_millis(50))
                .is_err(),
            "capture must not pass a writer holding the revision barrier",
        );

        release_writer_tx.send(()).unwrap();
        writer.join().unwrap();
        let (generation, route_count) = capture_done_rx
            .recv_timeout(Duration::from_secs(1))
            .unwrap();
        capture.join().unwrap();

        assert_eq!(route_count, 1);
        assert_eq!(generation, initial_generation + 1);
        assert_eq!(generation, ep.diagnostic_generation());
    }

    #[tokio::test]
    async fn announce_facts_retain_ingress_route_and_observation_order() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x13; 64]));
        let interface = ep.attach_interface().id();
        let peer = PrivateIdentity::from_secret_bytes(&[0x14; 64]);
        let name = DestinationName::new("retinue", ["management-fact"]);
        let blob = AnnounceBlob::from_wire([0x22; 10]);
        let mut packet = announce::build(&peer, name.name_hash(), &blob, None, b"opaque");
        packet.hops = 2;
        packet.header_type = crate::packet::HeaderType::Type2;
        packet.transport = Some(AddressHash::from_bytes([0x55; 16]));
        let decoded = Announce::decode(&packet).unwrap();
        let destination = decoded.destination;

        process_verified_announce(&ep.shared, interface, packet, decoded);
        let fact = ep.next_announcement().await.unwrap();
        assert_eq!(fact.destination, destination);
        assert_eq!(fact.identity.hash(), peer.hash());
        assert_eq!(fact.app_data, b"opaque");
        assert_eq!(fact.interface, interface);
        assert_eq!(fact.hops, 2);
        assert_eq!(fact.transport, Some(AddressHash::from_bytes([0x55; 16])));
        assert_eq!(fact.sequence, 1);
    }

    #[tokio::test]
    async fn freshness_replay_and_stale_rejection_leave_all_announce_effects_unchanged() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x41; 64]));
        let iface = ep.attach_interface().id();
        ep.enable_routing();
        let peer = PrivateIdentity::from_secret_bytes(&[0x42; 64]);
        let (accepted_packet, accepted) =
            freshness_announce(&peer, "freshness-effects", 0, 1, 10, 1);
        let destination = accepted.destination;
        process_verified_announce(&ep.shared, iface, accepted_packet.clone(), accepted.clone());
        let first = ep.next_announcement().await.expect("accepted announcement");
        let route = *ep
            .shared
            .path_table
            .lock()
            .unwrap()
            .get(&destination)
            .expect("accepted route");
        let seen = ep.shared.seen_announces.lock().unwrap().1.len();

        // Same wire blob in a different context is still an exact freshness replay. It must
        // not advance the address book, route, publication sequence, relay cache, or output.
        let (mut replay_packet, replay) =
            freshness_announce(&peer, "freshness-effects", 0x0b, 1, 10, 1);
        replay_packet.context = crate::path::CTX_PATH_RESPONSE;
        process_verified_announce(&ep.shared, iface, replay_packet, replay);

        // A distinct blob behind the incumbent is stale for the same destination.
        let (stale_packet, stale) = freshness_announce(&peer, "freshness-effects", 0, 2, 9, 3);
        process_verified_announce(&ep.shared, iface, stale_packet, stale);

        assert_eq!(
            ep.shared
                .address_book
                .lock()
                .unwrap()
                .resolve(destination)
                .expect("accepted peer retained")
                .announces_seen,
            1,
        );
        assert_eq!(
            *ep.shared
                .path_table
                .lock()
                .unwrap()
                .get(&destination)
                .unwrap(),
            route,
        );
        assert_eq!(
            ep.shared.announce_sequence.load(Ordering::Relaxed),
            first.sequence
        );
        assert_eq!(ep.shared.seen_announces.lock().unwrap().1.len(), seen);
        assert!(
            matches!(
                ep.announce_rx
                    .try_lock()
                    .expect("receiver is idle")
                    .try_recv(),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty)
            ),
            "rejected announces are never published",
        );
        let counters = ep.routing_counters();
        assert_eq!(counters.freshness_replays_rejected, 1);
        assert_eq!(counters.freshness_stale_rejected, 1);
    }

    #[tokio::test]
    async fn held_older_announce_cannot_publish_after_newer_direct_ingress() {
        let hub = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0xA1; 64]));
        hub.enable_routing();
        let noisy = hub.attach_interface();
        let noisy_id = noisy.id();
        let noisy_sink = noisy.sink();
        let quiet = hub.attach_interface();
        let quiet_id = quiet.id();
        let quiet_sink = quiet.sink();
        let _egress = hub.attach_interface();
        let ingress_policy = AnnounceIngressPolicy {
            held_capacity: 4,
            frequency_window: Duration::from_secs(10),
            burst_hold: Duration::from_millis(200),
            burst_penalty: Duration::from_millis(200),
            held_release_interval: Duration::from_millis(1),
            new_interface_hz: 1,
            established_interface_hz: 1,
            destination_target: Duration::ZERO,
            ..AnnounceIngressPolicy::default()
        };
        hub.set_announce_ingress_policy(ingress_policy);

        // The first two unknown destinations prime the noisy interface. The third enters the
        // real held queue while remaining unknown to the address book and path table.
        for (index, (seed, name)) in [(0xA2, "freshness-prime-a"), (0xA3, "freshness-prime-b")]
            .into_iter()
            .enumerate()
        {
            let peer = PrivateIdentity::from_secret_bytes(&[seed; 64]);
            let (packet, _) = freshness_announce(&peer, name, 0, 1, 1, 1);
            assert!(noisy_sink.deliver(packet));
            let expected = index as u64 + 1;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            while hub.shared.announce_sequence.load(Ordering::Relaxed) < expected
                && tokio::time::Instant::now() < deadline
            {
                tokio::task::yield_now().await;
            }
            assert_eq!(
                hub.shared.announce_sequence.load(Ordering::Relaxed),
                expected
            );
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        let target_peer = PrivateIdentity::from_secret_bytes(&[0xA4; 64]);
        let (older_packet, older) =
            freshness_announce(&target_peer, "freshness-held-order", 0, 1, 10, 2);
        let destination = older.destination;
        assert!(noisy_sink.deliver(older_packet));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while hub.announce_ingress_counters(noisy_id).held == 0
            && tokio::time::Instant::now() < deadline
        {
            tokio::task::yield_now().await;
        }
        assert_eq!(hub.announce_ingress_counters(noisy_id).held, 1);
        assert!(hub.resolve(destination).is_none());
        hub.set_announce_ingress_policy(AnnounceIngressPolicy {
            new_interface_hz: 1_000,
            established_interface_hz: 1_000,
            ..ingress_policy
        });

        // A newer copy on the quiet interface is processed immediately. Reconfiguring the
        // retained bounds while the release task exists shares the same freshness guard.
        let (newer_packet, _) =
            freshness_announce(&target_peer, "freshness-held-order", 0, 2, 11, 1);
        assert!(quiet_sink.deliver(newer_packet));
        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while hub.resolve(destination).is_none() && tokio::time::Instant::now() < deadline {
            tokio::task::yield_now().await;
        }
        assert!(hub.resolve(destination).is_some());
        hub.set_announce_freshness_policy(hub.announce_freshness_policy())
            .unwrap();
        let sequence_before_release = hub.shared.announce_sequence.load(Ordering::Relaxed);
        assert_eq!(sequence_before_release, 3);
        let forwards_before_release = hub.routing_counters().forwarded_announces;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
        while hub.announce_ingress_counters(noisy_id).released == 0
            && tokio::time::Instant::now() < deadline
        {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        assert_eq!(hub.announce_ingress_counters(noisy_id).released, 1);
        assert_eq!(hub.routing_counters().freshness_stale_rejected, 1);
        assert_eq!(
            hub.shared.announce_sequence.load(Ordering::Relaxed),
            sequence_before_release,
            "the deferred stale copy did not publish"
        );
        assert_eq!(
            hub.routing_counters().forwarded_announces,
            forwards_before_release,
            "the deferred stale copy did not relay"
        );
        assert_eq!(hub.route_to(destination), Some((quiet_id, 1)));
        assert_eq!(
            hub.shared
                .address_book
                .lock()
                .unwrap()
                .resolve(destination)
                .unwrap()
                .announces_seen,
            1
        );
    }

    #[tokio::test]
    async fn newer_equal_and_worse_routes_replace_the_incumbent() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x43; 64]));
        let first_iface = ep.attach_interface().id();
        let equal_iface = ep.attach_interface().id();
        let worse_iface = ep.attach_interface().id();
        let peer = PrivateIdentity::from_secret_bytes(&[0x44; 64]);

        let (first_packet, first) = freshness_announce(&peer, "freshness-route", 0, 1, 10, 1);
        let destination = first.destination;
        process_verified_announce(&ep.shared, first_iface, first_packet, first);
        let _ = ep.next_announcement().await.unwrap();

        let (equal_packet, equal) = freshness_announce(&peer, "freshness-route", 0, 2, 11, 1);
        process_verified_announce(&ep.shared, equal_iface, equal_packet, equal);
        let _ = ep.next_announcement().await.unwrap();
        assert_eq!(ep.route_to(destination), Some((equal_iface, 1)));

        let (worse_packet, worse) = freshness_announce(&peer, "freshness-route", 0, 3, 12, 5);
        process_verified_announce(&ep.shared, worse_iface, worse_packet, worse);
        let _ = ep.next_announcement().await.unwrap();
        assert_eq!(ep.route_to(destination), Some((worse_iface, 5)));
    }

    /// An endpoint whose routes live a minute, so a test can age one past its TTL.
    fn short_ttl_endpoint(seed: u8) -> Endpoint {
        Endpoint::with_announce_freshness_policy(
            PrivateIdentity::from_secret_bytes(&[seed; 64]),
            AnnounceFreshnessPolicy {
                route_ttl: Duration::from_secs(60),
                ..AnnounceFreshnessPolicy::default()
            },
        )
        .unwrap()
    }

    fn expire_route(ep: &Endpoint, destination: AddressHash) {
        let aged = Instant::now()
            .checked_sub(ep.shared.route_ttl())
            .expect("the monotonic clock predates one route TTL");
        ep.shared
            .path_table
            .lock()
            .unwrap()
            .get_mut(&destination)
            .expect("learned route")
            .learned = aged;
        assert_eq!(ep.route_to(destination), None, "route expired");
    }

    /// RNS culls a path row with its random blobs (`Transport.py` 957-978, 1086-1090), so the
    /// next announce is a first sighting whatever its emission time or hops.
    #[tokio::test]
    async fn expired_routes_admit_any_announce_as_a_first_sighting() {
        let ep = short_ttl_endpoint(0x45);
        let first_iface = ep.attach_interface().id();
        let replacement_iface = ep.attach_interface().id();
        let better_peer = PrivateIdentity::from_secret_bytes(&[0x46; 64]);
        let equal_peer = PrivateIdentity::from_secret_bytes(&[0x47; 64]);
        let worse_peer = PrivateIdentity::from_secret_bytes(&[0x48; 64]);
        let cases = [
            (&better_peer, "freshness-expired-better", 1_u8),
            (&equal_peer, "freshness-expired-equal", 2_u8),
            (&worse_peer, "freshness-expired-worse", 3_u8),
        ];

        for (peer, name, hops) in cases {
            let (first_packet, first) = freshness_announce(peer, name, 0, 1, 10, 2);
            let destination = first.destination;
            process_verified_announce(&ep.shared, first_iface, first_packet, first);
            let _ = ep.next_announcement().await.unwrap();
            expire_route(&ep, destination);

            let (packet, older) = freshness_announce(peer, name, 0, 2, 9, hops);
            process_verified_announce(&ep.shared, replacement_iface, packet, older);
            let accepted = ep.next_announcement().await.unwrap();
            assert_eq!((accepted.destination, accepted.hops), (destination, hops));
            assert_eq!(ep.route_to(destination), Some((replacement_iface, hops)));
        }
        assert_eq!(ep.routing_counters().freshness_stale_rejected, 0);
    }

    /// A transport answering `request_path` from its cache sends the blob it already relayed
    /// (`Transport.py` 3459-3530). While the route lives that is a replay; once it has gone,
    /// the same blob restores it.
    #[tokio::test]
    async fn a_cached_path_response_restores_an_expired_route() {
        let ep = short_ttl_endpoint(0x4B);
        let first_iface = ep.attach_interface().id();
        let answer_iface = ep.attach_interface().id();
        let peer = PrivateIdentity::from_secret_bytes(&[0x4C; 64]);
        let (packet, announced) = freshness_announce(&peer, "freshness-restore", 0, 1, 10, 2);
        let destination = announced.destination;
        let (cached_packet, cached) = freshness_announce(
            &peer,
            "freshness-restore",
            crate::path::CTX_PATH_RESPONSE,
            1,
            10,
            3,
        );
        process_verified_announce(&ep.shared, first_iface, packet.clone(), announced.clone());
        let _ = ep.next_announcement().await.unwrap();

        process_verified_announce(
            &ep.shared,
            answer_iface,
            cached_packet.clone(),
            cached.clone(),
        );
        assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);
        assert_eq!(ep.route_to(destination), Some((first_iface, 2)));

        expire_route(&ep, destination);
        process_verified_announce(&ep.shared, answer_iface, cached_packet, cached);
        let restored = ep.next_announcement().await.unwrap();
        assert_eq!((restored.destination, restored.sequence), (destination, 2));
        assert_eq!(ep.route_to(destination), Some((answer_iface, 3)));
        assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);

        process_verified_announce(&ep.shared, first_iface, packet, announced);
        assert_eq!(
            ep.routing_counters().freshness_replays_rejected,
            2,
            "the restored route refuses the blob again"
        );
    }

    /// A refused identity publishes no `PeerAnnounce`, but its path is learned and its
    /// freshness committed: a book's capacity never decides what the router can reach.
    #[tokio::test]
    async fn address_book_refusal_still_learns_the_path() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x47; 64]));
        let iface = ep.attach_interface().id();
        let peer = PrivateIdentity::from_secret_bytes(&[0x48; 64]);
        let (packet, announcement) = freshness_announce(&peer, "freshness-refusal", 0, 1, 10, 1);
        let destination = announcement.destination;
        *ep.shared.address_book.lock().unwrap() = AddressBook::with_max_peers(0);
        process_verified_announce(&ep.shared, iface, packet.clone(), announcement.clone());
        assert_eq!(ep.routing_counters().refused_announces, 1);
        assert_eq!(ep.shared.announce_sequence.load(Ordering::Relaxed), 0);
        assert_eq!(ep.route_to(destination), Some((iface, 1)));
        assert!(ep.resolve(destination).is_none());

        *ep.shared.address_book.lock().unwrap() = AddressBook::with_max_peers(1);
        process_verified_announce(&ep.shared, iface, packet, announcement);
        assert_eq!(ep.routing_counters().freshness_replays_rejected, 1);
    }

    /// A full book admits a newcomer by evicting the least recently heard peer whose path
    /// has gone stale, and the newcomer is relayed whether or not the book takes it.
    #[tokio::test]
    async fn a_full_address_book_evicts_a_peer_whose_path_expired() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4B; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        *ep.shared.address_book.lock().unwrap() = AddressBook::with_max_peers(2);
        let announcements: Vec<_> = (0..4u8)
            .map(|i| {
                let peer = PrivateIdentity::from_secret_bytes(&[0x60 + i; 64]);
                freshness_announce(&peer, "eviction", 0, i, 10, 1)
            })
            .collect();
        let relayed = |destination: AddressHash| {
            let sent = b.outbound.queues.pop();
            b.outbound.queues.delivery_complete();
            sent.is_some_and(|p| p.destination == destination)
        };

        for (packet, announcement) in &announcements[..3] {
            process_verified_announce(&ep.shared, a.id(), packet.clone(), announcement.clone());
            assert!(relayed(announcement.destination));
        }
        let held = |i: usize| ep.resolve(announcements[i].1.destination).is_some();
        assert!(held(0) && held(1) && !held(2));
        assert_eq!(ep.routing_counters().refused_announces, 1);
        assert_eq!(ep.shared.announce_sequence.load(Ordering::Relaxed), 2);

        // Every path is now past its TTL, so nothing protects the held peers.
        ep.shared.route_ttl_ms.store(0, Ordering::Relaxed);
        let (packet, announcement) = announcements[3].clone();
        process_verified_announce(&ep.shared, a.id(), packet, announcement.clone());
        assert!(relayed(announcement.destination));
        assert_eq!(ep.next_announcement().await.unwrap().sequence, 1);
        assert_eq!(ep.next_announcement().await.unwrap().sequence, 2);
        assert_eq!(
            ep.next_announcement().await.unwrap().destination,
            announcement.destination
        );
        assert!(held(3));
        assert_eq!(usize::from(held(0)) + usize::from(held(1)), 1);
        assert_eq!(ep.shared.address_book.lock().unwrap().evicted(), 1);
    }

    /// RNS learns from a path response but never rebroadcasts it: it answers one requester.
    #[tokio::test]
    async fn a_path_response_is_learned_but_not_relayed() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4C; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let peer = PrivateIdentity::from_secret_bytes(&[0x4D; 64]);
        let (packet, announcement) = freshness_announce(
            &peer,
            "path-response",
            crate::path::CTX_PATH_RESPONSE,
            1,
            10,
            2,
        );
        process_verified_announce(&ep.shared, a.id(), packet, announcement.clone());

        let event = ep.next_announcement().await.unwrap();
        assert_eq!(event.destination, announcement.destination);
        assert_eq!(ep.route_to(announcement.destination), Some((a.id(), 2)));
        assert!(b.outbound.queues.pop().is_none(), "not relayed");
        assert!(a.outbound.queues.pop().is_none());
        assert_eq!(ep.routing_counters().forwarded_announces, 0);
    }

    /// A relay echoing one of our own announces back must not teach us a path to ourselves,
    /// publish ourselves as a peer, or be relayed again.
    #[tokio::test]
    async fn an_echo_of_our_own_announce_is_dropped() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x4E; 64]));
        let name = DestinationName::new("retinue", ["own-echo"]);
        let destination = name.destination_hash(ep.identity());
        ep.register(name.clone(), b"own");
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();

        let mut echo = ep.build_announce_at(&name, b"own", 4_000);
        echo.hops += 1;
        echo.header_type = crate::packet::HeaderType::Type2;
        echo.transport = Some(AddressHash::from_bytes([0x7E; 16]));
        route(&ep.shared, a.id(), echo);

        assert_eq!(ep.shared.announce_sequence.load(Ordering::Relaxed), 0);
        assert!(ep.route_to(destination).is_none());
        assert!(ep.resolve(destination).is_none());
        assert!(a.outbound.queues.pop().is_none());
        assert!(b.outbound.queues.pop().is_none());
        assert_eq!(ep.routing_counters().forwarded_announces, 0);
    }

    #[tokio::test]
    async fn freshness_capacity_eviction_is_visible() {
        let policy = AnnounceFreshnessPolicy {
            destination_capacity: 1,
            blob_capacity: 1,
            ..AnnounceFreshnessPolicy::default()
        };
        let ep = Endpoint::with_announce_freshness_policy(
            PrivateIdentity::from_secret_bytes(&[0x49; 64]),
            policy,
        )
        .unwrap();
        let iface = ep.attach_interface().id();
        let peer = PrivateIdentity::from_secret_bytes(&[0x4A; 64]);
        let (a_packet, a) = freshness_announce(&peer, "freshness-capacity-a", 0, 1, 10, 1);
        let (renewed_packet, renewed) =
            freshness_announce(&peer, "freshness-capacity-a", 0, 3, 11, 1);
        let (b_packet, b) = freshness_announce(&peer, "freshness-capacity-b", 0, 2, 10, 1);
        process_verified_announce(&ep.shared, iface, a_packet, a);
        let _ = ep.next_announcement().await.unwrap();
        process_verified_announce(&ep.shared, iface, renewed_packet, renewed);
        let _ = ep.next_announcement().await.unwrap();
        process_verified_announce(&ep.shared, iface, b_packet, b);
        let _ = ep.next_announcement().await.unwrap();
        assert_eq!(ep.routing_counters().freshness_rows_evicted, 1);
        assert_eq!(ep.routing_counters().freshness_blobs_evicted, 1);
    }

    #[tokio::test]
    async fn an_inbound_link_fact_keeps_unknown_remote_unknown() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x15; 64]));
        let interface = ep.attach_interface().id();
        let destination =
            DestinationName::new("retinue", ["management-link"]).destination_hash(ep.identity());
        let (_, request) = link::PendingLink::open(
            destination,
            *ep.identity(),
            &[0x17; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: DEFAULT_LINK_MTU,
            },
        );
        let (link, _) = link::accept(
            &request,
            &ep.shared.identity,
            &[0x18; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: DEFAULT_LINK_MTU,
            },
        )
        .unwrap();
        let _stream = register_stream(
            &ep.shared,
            link,
            interface,
            Liveness::responder(0, 0),
            LinkDirection::Inbound,
            LinkRemoteFact::default(),
        )
        .unwrap();

        let facts = ep.link_facts();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].direction, LinkDirection::Inbound);
        assert_eq!(facts[0].remote, LinkRemoteFact::default());
        assert_eq!(facts[0].interface, interface);
    }

    /// One radio routinely reaches different destinations through different transport
    /// nodes. Keyed by interface, the second one learned overwrote the first, and every
    /// packet for the first was addressed to the wrong node: announce A via X and B via Y on
    /// one interface, and A silently routes through Y.
    /// A peer that connects and drops repeatedly -- a flapping link, a daemon being
    /// restarted -- used to leave its interface record and queues behind on every cycle.
    #[tokio::test]
    async fn a_dropped_tcp_peer_leaves_no_interface_behind() {
        let server = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x36; 64]));
        let addr = server
            .listen_tcp("127.0.0.1:0".parse().unwrap())
            .await
            .expect("listener");
        let settled = server.shared.interfaces.lock().unwrap().len();

        for _ in 0..6 {
            let client = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x37; 64]));
            client.attach_tcp_client(addr).await.expect("connect");
            // Dropping the client closes its socket, which the server's reader observes.
            drop(client);
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
        }

        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(
            server.shared.interfaces.lock().unwrap().len(),
            settled,
            "six connect/drop cycles must leave nothing accumulated",
        );
    }

    /// The path table was the last place a stranger could grow this process's memory for
    /// free. It is capped now, and what it forgets is chosen rather than arbitrary: the peer
    /// that has gone quietest, because announces are what feed and refresh the table.
    #[tokio::test]
    async fn a_full_path_table_forgets_the_quietest_peer() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x34; 64]));
        let iface = InterfaceId::from(1_u32);
        let quiet = AddressHash::from_bytes([0x01; 16]);

        // The quiet one is learned first, so its `learned` is oldest.
        ep.shared.learn_path(quiet, iface, 1, None);
        for n in 2..=PATH_TABLE_CAPACITY as u8 {
            ep.shared
                .learn_path(AddressHash::from_bytes([n; 16]), iface, 1, None);
        }
        assert_eq!(
            ep.shared.path_table.lock().unwrap().len(),
            PATH_TABLE_CAPACITY
        );

        // One more destination than the table holds.
        let newcomer = AddressHash::from_bytes([0xFE; 16]);
        ep.shared.learn_path(newcomer, iface, 1, None);

        let table = ep.shared.path_table.lock().unwrap();
        assert_eq!(table.len(), PATH_TABLE_CAPACITY, "the bound holds");
        assert!(table.contains_key(&newcomer), "the newcomer is learned");
        assert!(
            !table.contains_key(&quiet),
            "and the peer that had gone quietest is what made room",
        );
        drop(table);
        assert_eq!(
            ep.routing_counters().paths_evicted,
            1,
            "evictions are counted, not silent",
        );
    }

    /// A peer that keeps announcing keeps its route, which is the other half of the policy:
    /// re-announcing refreshes `learned`, so the still-talking are never the ones evicted.
    #[tokio::test]
    async fn a_peer_that_keeps_announcing_keeps_its_route() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x35; 64]));
        let iface = InterfaceId::from(1_u32);
        let talkative = AddressHash::from_bytes([0x01; 16]);

        ep.shared.learn_path(talkative, iface, 1, None);
        for n in 2..=PATH_TABLE_CAPACITY as u8 {
            ep.shared
                .learn_path(AddressHash::from_bytes([n; 16]), iface, 1, None);
        }
        // It re-announces, which is what a live peer does and what moves it off oldest.
        ep.shared.learn_path(talkative, iface, 1, None);

        ep.shared
            .learn_path(AddressHash::from_bytes([0xFE; 16]), iface, 1, None);

        let table = ep.shared.path_table.lock().unwrap();
        assert!(
            table.contains_key(&talkative),
            "a peer still announcing must not be the one forgotten",
        );
    }

    /// Attaching was one-way, so a peer that reconnects repeatedly grew the interface list
    /// and its queues without bound, and the scheduler kept visiting records for carriers
    /// that were long gone.
    #[tokio::test]
    async fn detaching_an_interface_forgets_it() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x33; 64]));
        let before = ep.shared.interfaces.lock().unwrap().len();

        let iface = ep.attach_interface();
        let id = iface.id;
        assert_eq!(ep.shared.interfaces.lock().unwrap().len(), before + 1);

        ep.detach_interface(id);
        assert_eq!(
            ep.shared.interfaces.lock().unwrap().len(),
            before,
            "a detached interface leaves no record behind",
        );

        // And reconnecting the same carrier does not stack up.
        for _ in 0..8 {
            let again = ep.attach_interface();
            ep.detach_interface(again.id);
        }
        assert_eq!(
            ep.shared.interfaces.lock().unwrap().len(),
            before,
            "eight reconnects leave nothing accumulated",
        );
    }

    #[tokio::test]
    async fn two_destinations_on_one_interface_keep_their_own_transports() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x31; 64]));
        let iface = InterfaceId::from(3_u32);
        let a = AddressHash::from_bytes([0xAA; 16]);
        let b = AddressHash::from_bytes([0xBB; 16]);
        let via_x = AddressHash::from_bytes([0x11; 16]);
        let via_y = AddressHash::from_bytes([0x22; 16]);

        ep.shared.learn_path(a, iface, 1, Some(via_x));
        ep.shared.learn_path(b, iface, 1, Some(via_y));

        let addressed = |dest| {
            let mut pkt = crate::path::path_request(dest, &[0x5A; 16]);
            pkt.destination = dest;
            ep.shared.address_for(iface, pkt).transport
        };
        assert_eq!(addressed(a), Some(via_x), "A must still route through X");
        assert_eq!(addressed(b), Some(via_y), "B routes through Y");
    }

    /// Once freshness admits an announce, it is the current route even when the predecessor
    /// had fewer hops. Freshness owns ordering; routing does not reopen that decision with a
    /// local shortest-path filter.
    #[tokio::test]
    async fn a_newer_worse_route_replaces_the_incumbent() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x32; 64]));
        let dest = AddressHash::from_bytes([0xCC; 16]);
        let good = InterfaceId::from(1_u32);
        let worse = InterfaceId::from(2_u32);

        let learned = Instant::now();
        ep.shared.learn_path_at(dest, good, 1, None, learned);
        ep.shared.learn_path_at(
            dest,
            worse,
            5,
            Some(AddressHash::from_bytes([0xA5; 16])),
            learned + Duration::from_millis(1),
        );

        let entry = *ep.shared.path_table.lock().unwrap().get(&dest).unwrap();
        assert_eq!(entry.iface, worse, "the newer route becomes incumbent");
        assert_eq!(entry.hops, 5);
        assert_eq!(
            entry.transport,
            Some(AddressHash::from_bytes([0xA5; 16])),
            "all route facts come from the accepted announce",
        );
    }

    #[test]
    fn destination_admission_preserves_the_one_second_default_floor() {
        let mut admission = AnnounceAdmission::new(AnnounceIngressPolicy::default());
        let a = AddressHash::from_bytes([0x01; 16]);
        let b = AddressHash::from_bytes([0x02; 16]);
        assert_eq!(
            admission.observe_destination(a, 0),
            DestinationVerdict::Relay
        );
        assert_eq!(
            admission.observe_destination(b, 0),
            DestinationVerdict::Relay
        );
        assert_eq!(
            admission.observe_destination(a, 1),
            DestinationVerdict::BlockRelay,
            "a fresh re-announce is not rebroadcast"
        );
        let c = AddressHash::from_bytes([0x03; 16]);
        assert_eq!(
            admission.observe_destination(c, 1),
            DestinationVerdict::Relay
        );
    }

    /// A link-less data packet to `destination`, carrying `payload` as is.
    fn single_packet(destination: AddressHash, payload: Vec<u8>) -> Packet {
        Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination,
            context: 0,
            payload,
        }
    }

    /// Routes default to RNS's one-week lifetime, and an access-point or roaming interface
    /// bounds the routes learned on it to a day or six hours.
    #[tokio::test]
    async fn routes_live_a_week_unless_their_interface_mode_shortens_them() {
        assert_eq!(
            AnnounceFreshnessPolicy::default().route_ttl,
            Duration::from_secs(7 * 24 * 60 * 60)
        );
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x61; 64]));
        let full = ep.attach_interface();
        let roaming = ep.attach_interface();
        let access_point = ep.attach_interface();
        assert!(ep.set_interface_mode(roaming.id(), InterfaceMode::Roaming));
        assert!(ep.set_interface_mode(access_point.id(), InterfaceMode::AccessPoint));
        assert!(!ep.set_interface_mode(u32::MAX, InterfaceMode::Roaming));

        let learned = Instant::now();
        let hours = |h: u64| Duration::from_secs(h * 60 * 60);
        let [a, b, c] = [0xA1, 0xA2, 0xA3].map(|n| AddressHash::from_bytes([n; 16]));
        ep.shared.learn_path_at(a, full.id(), 1, None, learned);
        ep.shared.learn_path_at(b, roaming.id(), 1, None, learned);
        ep.shared
            .learn_path_at(c, access_point.id(), 1, None, learned);

        let just_before = |h| learned + hours(h) - Duration::from_millis(1);
        assert!(ep.route_to_at(b, just_before(6)).is_some());
        assert!(ep.route_to_at(b, learned + hours(6)).is_none());
        assert!(ep.route_to_at(c, just_before(24)).is_some());
        assert!(ep.route_to_at(c, learned + hours(24)).is_none());
        assert!(ep.route_to_at(a, just_before(7 * 24)).is_some());
        assert!(ep.route_to_at(a, learned + hours(7 * 24)).is_none());
    }

    /// A route that carries traffic is refreshed, as RNS refreshes a path whenever it inserts
    /// a packet into transport by it or forwards transit along it. A local send to a direct
    /// neighbour does not insert anything, and does not refresh.
    #[tokio::test]
    async fn a_route_in_use_is_refreshed() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x62; 64]));
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();
        let aged = Instant::now()
            .checked_sub(Duration::from_secs(60 * 60))
            .expect("the monotonic clock predates an hour");
        let learned = |dest| ep.shared.path_table.lock().unwrap()[&dest].learned;
        let [via_relay, direct, onward] =
            [0xB1, 0xB2, 0xB3].map(|n| AddressHash::from_bytes([n; 16]));
        let relay = AddressHash::from_bytes([0xBF; 16]);
        ep.shared
            .learn_path_at(via_relay, a.id(), 2, Some(relay), aged);
        ep.shared.learn_path_at(direct, a.id(), 0, None, aged);
        ep.shared.learn_path_at(onward, b.id(), 0, None, aged);

        ep.shared
            .queue_single(via_relay, single_packet(via_relay, vec![1; 64]));
        assert_eq!(a.outbound.queues.pop().unwrap().transport, Some(relay));
        a.outbound.queues.delivery_complete();
        assert!(
            learned(via_relay) > aged,
            "inserting into transport refreshes"
        );

        ep.shared
            .queue_single(direct, single_packet(direct, vec![2; 64]));
        assert!(a.outbound.queues.pop().is_some());
        a.outbound.queues.delivery_complete();
        assert_eq!(learned(direct), aged, "a direct send does not");

        let mut transit = single_packet(onward, vec![3; 64]);
        transit.header_type = crate::packet::HeaderType::Type2;
        transit.transport = Some(ep.identity().hash());
        route(&ep.shared, a.id(), transit);
        assert!(b.outbound.queues.pop().is_some());
        assert!(learned(onward) > aged, "carrying transit refreshes");
    }

    /// Detaching an interface culls the routes learned on it and the bridges that cross it, so
    /// a send to that destination falls back to broadcast on what is left.
    #[tokio::test]
    async fn detaching_an_interface_culls_its_routes_and_bridges() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x63; 64]));
        let gone = ep.attach_interface();
        let kept = ep.attach_interface();
        let dest = AddressHash::from_bytes([0xC1; 16]);
        ep.shared.learn_path(
            dest,
            gone.id(),
            1,
            Some(AddressHash::from_bytes([0xCF; 16])),
        );
        let [crossing, beside] = [0xC2, 0xC3].map(|n| AddressHash::from_bytes([n; 16]));
        {
            let mut bridges = ep.shared.link_transport.lock().unwrap();
            let bridge = |from, out| LinkBridge {
                from,
                out,
                destination: AddressHash::from_bytes([0xCF; 16]),
                seen: Instant::now(),
                proof_deadline: None,
            };
            bridges.insert(crossing, bridge(kept.id(), gone.id()));
            bridges.insert(beside, bridge(kept.id(), kept.id()));
        }

        ep.detach_interface(gone.id());
        assert_eq!(ep.route_to(dest), None);
        let bridges = ep.shared.link_transport.lock().unwrap().clone();
        assert!(!bridges.contains_key(&crossing));
        assert!(bridges.contains_key(&beside));

        let result = ep
            .shared
            .queue_single(dest, single_packet(dest, vec![4; 64]));
        assert_eq!(result.queued, 1, "broadcast on the remaining interface");
        let sent = kept.outbound.queues.pop().expect("broadcast reached it");
        assert_eq!(sent.header_type, crate::packet::HeaderType::Type1);
    }

    /// A path request for one of our destinations is answered once per tag, on the interface
    /// it came in on only. Tagless requests and requests relayed past one hop are ignored.
    #[tokio::test]
    async fn a_path_request_is_answered_once_on_its_own_interface() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x64; 64]));
        let name = crate::destination::DestinationName::new("retinue", ["pathonce"]);
        let dest = name.destination_hash(ep.identity());
        ep.register(name, b"once");
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        let request = |tag: u8| crate::path::path_request(dest, &[tag; crate::path::TAG_LEN]);
        let take = |iface: &Interface| {
            let packet = iface.outbound.queues.pop();
            if packet.is_some() {
                iface.outbound.queues.delivery_complete();
            }
            packet
        };

        route(&ep.shared, a.id(), request(1));
        let response = take(&a).expect("answered on the requesting interface");
        assert_eq!(response.packet_type, PacketType::Announce);
        assert_eq!(response.context, crate::path::CTX_PATH_RESPONSE);
        assert!(take(&b).is_none(), "and on no other");

        route(&ep.shared, b.id(), request(1));
        let mut tagless = request(2);
        tagless.payload.truncate(crate::hash::ADDRESS_HASH_LEN);
        route(&ep.shared, b.id(), tagless);
        let mut far = request(3);
        far.hops = 2;
        route(&ep.shared, b.id(), far);
        assert!(take(&a).is_none() && take(&b).is_none());
        assert_eq!(ep.routing_counters().filtered_packets, 3);

        // The three-field form a transport-enabled RNS sends still names its tag.
        let mut from_transport = request(4);
        from_transport.payload.splice(16..16, [0xEE; 16]);
        route(&ep.shared, b.id(), from_transport);
        assert!(take(&b).is_some(), "a new tag is answered");
        assert!(take(&a).is_none());
    }

    /// A packet addressed through another transport is filtered before local dispatch, even a
    /// link request naming one of our own destinations.
    #[tokio::test]
    async fn a_type_two_packet_for_another_transport_is_not_dispatched() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x65; 64]));
        let name = crate::destination::DestinationName::new("retinue", ["elsewhere"]);
        let dest = name.destination_hash(ep.identity());
        ep.register(name, b"");
        let a = ep.attach_interface();
        let (_, mut request) = link::PendingLink::open(
            dest,
            *ep.identity(),
            &[0x66; 64],
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: DEFAULT_LINK_MTU,
            },
        );
        request.header_type = crate::packet::HeaderType::Type2;
        request.transport = Some(AddressHash::from_bytes([0xEE; 16]));
        route(&ep.shared, a.id(), request.clone());
        assert!(
            a.outbound.queues.pop().is_none(),
            "no proof for another's packet"
        );
        assert_eq!(ep.routing_counters().filtered_packets, 1);

        request.header_type = crate::packet::HeaderType::Type1;
        request.transport = None;
        route(&ep.shared, a.id(), request);
        assert_eq!(
            a.outbound.queues.pop().map(|proof| proof.packet_type),
            Some(PacketType::Proof)
        );
    }

    /// A single packet heard twice, directly and from a relay, is delivered once; a transit
    /// packet heard twice is carried once.
    #[tokio::test]
    async fn repeated_single_and_transit_packets_are_filtered() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x67; 64]));
        let name = crate::destination::DestinationName::new("retinue", ["single"]);
        let dest = name.destination_hash(ep.identity());
        ep.register(name, b"");
        let a = ep.attach_interface();
        let b = ep.attach_interface();
        ep.enable_routing();

        let payload =
            crate::token::encrypt_to_identity(ep.identity(), &[7; KEY_LEN], &[8; IV_LEN], b"once");
        let single = single_packet(dest, payload);
        route(&ep.shared, a.id(), single.clone());
        route(&ep.shared, b.id(), single);
        assert_eq!(ep.accept_single().await.unwrap().data, b"once");
        assert!(
            tokio::time::timeout(Duration::from_millis(100), ep.accept_single())
                .await
                .is_err(),
            "the copy is not delivered"
        );

        let onward = AddressHash::from_bytes([0xD1; 16]);
        ep.shared.learn_path(onward, b.id(), 0, None);
        let mut transit = single_packet(onward, vec![5; 64]);
        transit.header_type = crate::packet::HeaderType::Type2;
        transit.transport = Some(ep.identity().hash());
        route(&ep.shared, a.id(), transit.clone());
        transit.hops = 3;
        route(&ep.shared, a.id(), transit);
        assert!(b.outbound.queues.pop().is_some());
        b.outbound.queues.delivery_complete();
        assert!(
            b.outbound.queues.pop().is_none(),
            "the loop's copy is dropped"
        );
        let counters = ep.routing_counters();
        assert_eq!(
            (counters.forwarded_packets, counters.filtered_packets),
            (1, 2)
        );
    }

    #[tokio::test]
    async fn answers_a_path_request_for_an_owned_destination() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[9u8; 64]));
        let mut iface = ep.attach_interface();
        let name = crate::destination::DestinationName::new("retinue", ["pathtest"]);
        let dest = name.destination_hash(ep.identity());
        ep.register(name, b"hello");

        // Registration broadcasts a spontaneous announce (context 0); drain it.
        let first = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
            .await
            .expect("registration announce")
            .expect("interface open");
        assert_eq!(first.packet_type, PacketType::Announce);
        assert_eq!(first.context, 0, "a spontaneous announce has context 0");

        // A peer requests a path to our destination.
        let sink = iface.sink();
        assert!(sink.deliver(crate::path::path_request(
            dest,
            &[0x77; crate::path::TAG_LEN]
        )));

        // We answer with a path response: an announce for that destination, context 0x0b.
        let resp = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
            .await
            .expect("path response emitted")
            .expect("interface open");
        assert_eq!(resp.packet_type, PacketType::Announce);
        assert_eq!(resp.context, crate::path::CTX_PATH_RESPONSE);
        assert_eq!(resp.destination, dest);
        // It is a valid announce that reconstructs to our destination and app data.
        let decoded = Announce::decode(&resp).expect("valid announce");
        assert_eq!(decoded.destination, dest);
        assert_eq!(decoded.app_data, b"hello");
        assert_eq!(decoded.identity.hash(), ep.identity().hash());
    }

    #[tokio::test]
    async fn ignores_a_path_request_for_an_unknown_destination() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[10u8; 64]));
        let mut iface = ep.attach_interface();
        let sink = iface.sink();
        let unknown = AddressHash::from_bytes([0xCC; 16]);
        assert!(sink.deliver(crate::path::path_request(
            unknown,
            &[0; crate::path::TAG_LEN]
        )));

        // We own nothing, hold no cache, so we stay silent.
        let got = tokio::time::timeout(Duration::from_millis(200), iface.next_outbound()).await;
        assert!(got.is_err(), "no response for an unknown destination");
    }

    /// Repeated asking for the same destination broadcasts once, and a different destination
    /// is unaffected. The thing being bounded is a stranger's ability to decide how much of a
    /// shared band we use: what provokes a path request is usually inbound traffic we cannot
    /// verify, so without a floor a peer gets one broadcast per packet it sends.
    #[tokio::test]
    async fn a_path_request_is_rate_limited_per_destination() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[11u8; 64]));
        let mut iface = ep.attach_interface();
        let wanted = AddressHash::from_bytes([0xA1; 16]);
        let other = AddressHash::from_bytes([0xB2; 16]);

        assert!(ep.request_path(wanted), "the first ask goes out");
        assert!(
            !ep.request_path(wanted),
            "an immediate repeat is suppressed"
        );
        assert!(
            ep.request_path(other),
            "a different destination is its own budget"
        );

        let first = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
            .await
            .expect("first request")
            .expect("interface open");
        assert_eq!(first.destination, crate::path::path_request_destination());
        let second = tokio::time::timeout(Duration::from_secs(1), iface.next_outbound())
            .await
            .expect("the other destination's request")
            .expect("interface open");
        assert_eq!(second.destination, crate::path::path_request_destination());
        let extra = tokio::time::timeout(Duration::from_millis(200), iface.next_outbound()).await;
        assert!(
            extra.is_err(),
            "the suppressed repeat put nothing on the air"
        );

        // Past the floor, asking again is allowed: a destination that never answered must be
        // askable later, or one lost response becomes permanent.
        tokio::time::sleep(PATH_REQUEST_MIN_INTERVAL + Duration::from_millis(20)).await;
        assert!(ep.request_path(wanted), "the floor expires");
    }

    /// A flood of unique destinations cannot broadcast without bound, because the peer that
    /// provokes a path request also chooses the destination it names: the per-destination
    /// floor never engages when no key repeats, so the global cap is what actually limits
    /// the airtime — and, since a refused request records nothing, the budget table too.
    #[tokio::test]
    async fn fabricated_unique_destinations_hit_the_global_path_request_cap() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[12u8; 64]));
        let _iface = ep.attach_interface();

        let mut sent = 0;
        for i in 0..(PATH_REQUEST_GLOBAL_MAX as u8 * 4) {
            let mut bytes = [0xD0; 16];
            bytes[0] = i;
            if ep.request_path(AddressHash::from_bytes(bytes)) {
                sent += 1;
            }
        }
        assert_eq!(
            sent, PATH_REQUEST_GLOBAL_MAX,
            "the window admits exactly the cap"
        );
        assert!(
            ep.shared.path_request_budget.lock().unwrap().len() <= PATH_REQUEST_GLOBAL_MAX,
            "refused requests must not grow the budget table",
        );

        // The cap is a window, not a lifetime total: once it slides, asking resumes.
        tokio::time::sleep(PATH_REQUEST_MIN_INTERVAL + Duration::from_millis(20)).await;
        assert!(
            ep.request_path(AddressHash::from_bytes([0xEE; 16])),
            "a fresh window admits new requests",
        );
    }
}
