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
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};

use crate::address_book::AddressBook;
use crate::announce::{self, Announce, RAND_HASH_LEN};
use crate::announce_admission::{
    AnnounceAdmission, AnnounceIngressCounters, AnnounceIngressPolicy, DestinationVerdict,
    InterfaceVerdict,
};
use crate::destination::DestinationName;
use crate::hash::{AddressHash, NameHash};
use crate::identity::{Identity, KEY_LEN, PrivateIdentity};
use crate::ifac::Ifac;
use crate::iface::hdlc::{Deframer, frame};
use crate::link::{
    self, CTX_CHANNEL, CTX_LINKCLOSE, CTX_LINKIDENTIFY, Inbound, Link, LinkMode, LinkTrailer,
};
use crate::packet::{DestinationType, Packet, PacketType};
use crate::ratchet::RatchetStore;
use crate::reliable::ReliableChannel;
use crate::request::{Request, Response};
use crate::resource::RANDOM_HASH_LEN;
use crate::resource_transfer::{ResourceReceiver, ResourceSender};
use crate::token::{IV_LEN, TOKEN_OVERHEAD};

/// Largest plaintext chunk per link data packet. Kept under `ENCRYPTED_MDU` (383) so the
/// encrypted token plus header always fits the MTU.
const WRITE_CHUNK: usize = crate::packet::ENCRYPTED_MDU - 16;

/// Largest best-effort stream plaintext whose padded encrypted token and
/// Reticulum header fit the MTU negotiated for this link.
///
/// CBC always adds at least one padding byte and rounds to a 16-byte block.
/// Keep the ordinary 500-byte path at its existing conservative ceiling while
/// shrinking radio links enough that the interface driver never has to reject
/// a packet after `AsyncWrite` already accepted its bytes.
fn write_chunk_for_mtu(mtu: u32) -> usize {
    let payload_room = (mtu as usize)
        .saturating_sub(crate::packet::HEADER_MIN_LEN)
        .min(crate::packet::MDU);
    let ciphertext_room = payload_room.saturating_sub(TOKEN_OVERHEAD);
    let padded_plaintext = (ciphertext_room / 16) * 16;
    padded_plaintext.saturating_sub(1).clamp(1, WRITE_CHUNK)
}

/// In-memory buffer for a stream's inbound side.
const DUPLEX_BUF: usize = 64 * 1024;

/// The reliable driver's clock period. It advances a logical tick each period, which drives
/// retransmission of unproven channel packets (`DEFAULT_RETX_TIMEOUT` ticks). One timer per
/// active reliable link; a production build would pause it when the link is fully idle.
const RELIABLE_TICK_MS: u64 = 50;

/// Fast interfaces start here; radio callers can raise it before opening links.
const DEFAULT_RELIABLE_INITIAL_RTT_MS: u64 = 750;

/// Default dynamic Channel ceiling, matching RNS. Strict half-duplex callers
/// can lower it without changing the wire format.
const DEFAULT_RELIABLE_MAX_WINDOW: u32 = crate::channel::WINDOW_MAX;

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

/// How long [`Endpoint::open`] waits for a link proof before giving up. Multi-hop setup can
/// be slow, so this is generous; it exists to bound a setup that will otherwise never
/// complete (a peer that never proves) rather than to hang the caller forever.
const LINK_SETUP_TIMEOUT: Duration = Duration::from_secs(15);

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

/// How long a bridged link's interface pair is remembered after its last packet.
///
/// A transport node records which two interfaces a forwarded link joins, so a proof or link
/// data arriving on one goes out the other. Nothing ever removed those records, so the map
/// grew with every link the node had ever carried rather than the ones it was carrying. An
/// hour is far longer than any live link goes quiet, and short enough that a node carrying
/// strangers' traffic all day does not accumulate the day.
const LINK_TRANSPORT_TTL: Duration = Duration::from_secs(3600);

/// Maximum hops an announce or packet may travel before a transport node drops it. RNS's
/// default `m` (`PATHFINDER_M`).
const MAX_HOPS: u8 = 128;

/// How many recent announce packet-hashes to remember for de-duplication.
const SEEN_ANNOUNCES: usize = 4096;

/// How long a learned route stays valid without a fresh announce. A peer re-announces
/// periodically; past this, a route to a peer that has gone silent is treated as stale and
/// evicted rather than kept forever. Short under `cfg(test)` so the lib's own expiry test
/// runs without waiting; integration tests link the lib without `cfg(test)` and see the real
/// value.
#[cfg(not(test))]
const PATH_TTL: Duration = Duration::from_secs(60 * 30);
#[cfg(test)]
const PATH_TTL: Duration = Duration::from_millis(60);

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

impl AsyncRead for LinkStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_read(cx, buf)
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
    packets: mpsc::UnboundedReceiver<Packet>,
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
                        if let Some(data) = receiver.data() {
                            return Ok(data.to_vec());
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
                        if let Some(data) = receiver.data() {
                            return Ok((identified, ReceivedPayload::Resource(data.to_vec())));
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
                        if let Some(bytes) = receiver.data() {
                            let packed = bytes.to_vec();
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
        self.shared.links.lock().unwrap().remove(&self.link.id());
        self.shared
            .send_on(self.iface, self.link.close_packet(&next_iv()));
        self.shared.end_resource();
    }
}

/// A validated announce, surfaced to a consumer that needs the app_data binding (e.g. to
/// map an application-level peer id to a retinue destination).
#[derive(Clone, Debug)]
pub struct PeerAnnounce {
    /// The destination hash announced.
    pub destination: AddressHash,
    /// The announcing identity.
    pub identity: Identity,
    /// The app data the announce carried (a host binds its own peer id here).
    pub app_data: Vec<u8>,
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
}

/// The live counter cells behind [`RoutingCounters`].
#[derive(Debug, Default)]
struct RoutingStats {
    forwarded_packets: AtomicU64,
    forwarded_announces: AtomicU64,
    policy_rejected: AtomicU64,
    hop_limit_dropped: AtomicU64,
    refused_announces: AtomicU64,
    held_announces: AtomicU64,
    held_announces_dropped: AtomicU64,
    relay_rate_limited_announces: AtomicU64,
    paths_evicted: AtomicU64,
}

impl RoutingStats {
    fn snapshot(&self) -> RoutingCounters {
        RoutingCounters {
            forwarded_packets: self.forwarded_packets.load(Ordering::Relaxed),
            forwarded_announces: self.forwarded_announces.load(Ordering::Relaxed),
            policy_rejected: self.policy_rejected.load(Ordering::Relaxed),
            hop_limit_dropped: self.hop_limit_dropped.load(Ordering::Relaxed),
            refused_announces: self.refused_announces.load(Ordering::Relaxed),
            held_announces: self.held_announces.load(Ordering::Relaxed),
            held_announces_dropped: self.held_announces_dropped.load(Ordering::Relaxed),
            relay_rate_limited_announces: self.relay_rate_limited_announces.load(Ordering::Relaxed),
            paths_evicted: self.paths_evicted.load(Ordering::Relaxed),
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
    /// modified frames before they reach the endpoint router.
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
    #[allow(dead_code)]
    iface: InterfaceId,
}

/// The delivery discipline of a link's stream, chosen when the stream is registered.
enum LinkKind {
    /// The router decrypts each data packet and forwards the plaintext (right for TCP,
    /// where the medium never drops).
    BestEffort {
        inbound: mpsc::UnboundedSender<Vec<u8>>,
    },
    /// The router forwards raw channel-data and proof packets to the reliable driver task,
    /// which orders them, proves receipts, and drives retransmission (for lossy media).
    Reliable {
        packets: mpsc::UnboundedSender<Packet>,
    },
    /// Raw resource control, part, and proof packets are handed to a resource session.
    Resource {
        packets: mpsc::UnboundedSender<Packet>,
    },
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

/// Shared router state.
struct Shared {
    lifecycle: Mutex<Lifecycle>,
    closed_notify: tokio::sync::Notify,
    identity: PrivateIdentity,
    address_book: Mutex<AddressBook>,
    links: Links,
    registered: Mutex<Vec<Registered>>,
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
    /// Upper bound, in milliseconds, of the random delay before relaying an announce. Zero
    /// (the default) relays immediately. See [`Endpoint::set_relay_jitter`].
    relay_jitter_ms: AtomicU64,
    /// First reliable-channel RTT estimate. Proofs adapt it after traffic starts.
    reliable_initial_rtt_ms: AtomicU64,
    /// Maximum unproved reliable frames allowed in flight on subsequently opened links.
    reliable_max_window: AtomicU32,
    /// Link-request retry interval for subsequently opened links.
    link_setup_retry_ms: AtomicU64,
    /// MTU requested and offered by subsequently established links.
    link_mtu: AtomicU32,
    /// Proofs for recently accepted link requests, keyed by link id. Replaying the same
    /// proof avoids creating a second stream when only the first proof was lost.
    inbound_link_proofs: Mutex<HashMap<AddressHash, (Packet, Instant)>>,
    /// Learned routes: destination → the interface to reach it and its hop count. Populated
    /// from announces.
    path_table: Mutex<HashMap<AddressHash, PathEntry>>,
    /// Recently-seen announce packet hashes, for de-duplication (a ring of the last
    /// [`SEEN_ANNOUNCES`]).
    seen_announces: Mutex<(HashSet<AddressHash>, VecDeque<AddressHash>)>,
    /// The bounded interface and destination announce-admission state machines. Their clock
    /// is relative to this endpoint so the verdicts are deterministic under a supplied time.
    announce_admission: Mutex<AnnounceAdmission>,
    announce_admission_started: Instant,
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
    link_transport: Mutex<HashMap<AddressHash, (InterfaceId, InterfaceId, Instant)>>,
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
#[derive(Clone, Copy)]
struct PathEntry {
    iface: InterfaceId,
    /// The transport node this destination is reached through, from the `transport` field of
    /// the header-type-2 announce that taught us the route. Per destination, because an
    /// interface can reach many destinations through different nodes: one radio hearing A
    /// via X and B via Y is the ordinary case, not an exotic one.
    transport: Option<AddressHash>,
    hops: u8,
    /// When this route was last (re)learned from an announce. Routes older than [`PATH_TTL`]
    /// are treated as stale and evicted on lookup.
    learned: Instant,
}

impl Shared {
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
        self.interfaces.lock().unwrap().push(iface);
        self.announce_admission
            .lock()
            .unwrap()
            .attach_interface(id, self.announce_admission_now_ms());
        true
    }

    /// Forget an interface, closing its outbound queues.
    ///
    /// Attaching was one-way: a transport that connects, drops, and reconnects left its old
    /// record, its queues, and anything scheduling against them in place forever. Anything
    /// still holding the matching [`Interface`] will see its queues closed and stop, which
    /// is the intended way to end a carrier.
    fn forget_interface(&self, id: InterfaceId) {
        let mut interfaces = self.interfaces.lock().unwrap();
        if let Some(index) = interfaces.iter().position(|iface| iface.id == id) {
            let iface = interfaces.swap_remove(index);
            iface.outbound.close();
        }
        self.announce_admission.lock().unwrap().forget_interface(id);
        self.held_announces
            .lock()
            .unwrap()
            .retain(|announce| announce.interface != id);
        self.held_release_wake.notify_waiters();
    }

    fn announce_admission_now_ms(&self) -> u64 {
        self.announce_admission_started
            .elapsed()
            .as_millis()
            .min(u128::from(u64::MAX)) as u64
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
        if let Some(t) = via {
            pkt.header_type = crate::packet::HeaderType::Type2;
            pkt.transport = Some(t);
        }
        let _ = iface;
        pkt
    }

    /// Build a path response for `target` if it is one of our registered destinations: an
    /// announce for it carrying context [`crate::path::CTX_PATH_RESPONSE`]. Returns `None` if
    /// we do not own `target` — we hold no announce cache, so we cannot answer for others and
    /// stay silent rather than guess.
    fn path_response(&self, target: AddressHash) -> Option<Packet> {
        let reg = self.registered.lock().unwrap();
        let r = reg.iter().find(|r| r.dest == target)?;
        let ratchet = r.ratchets.as_ref().and_then(RatchetStore::current_public);
        let mut pkt = announce::build(
            &self.identity,
            r.name.name_hash(),
            &rand_hash(),
            ratchet.as_ref(),
            &r.app_data,
        );
        pkt.context = crate::path::CTX_PATH_RESPONSE;
        Some(pkt)
    }

    /// Record that `dest` is reachable via `iface` at `hops`. Keeps the shortest fresh route,
    /// but always refreshes the learned time (so a re-announce keeps a route alive), and
    /// replaces a route that has expired regardless of hop count.
    fn learn_path(
        &self,
        dest: AddressHash,
        iface: InterfaceId,
        hops: u8,
        transport: Option<AddressHash>,
    ) {
        let mut t = self.path_table.lock().unwrap();
        let now = Instant::now();
        let keep_existing = t
            .get(&dest)
            .is_some_and(|e| e.hops <= hops && now.duration_since(e.learned) < PATH_TTL);
        if keep_existing {
            if let Some(e) = t.get_mut(&dest)
                && e.iface == iface
            {
                // Only an announce arriving the way this route goes proves this route is
                // alive. A worse alternate used to refresh it too, which meant a preferred
                // path that had died never aged out as long as any second path kept
                // announcing: failover could not happen, because the dead route stayed
                // permanently fresh.
                e.learned = now;
                e.transport = transport;
            }
        } else {
            if t.len() >= PATH_TABLE_CAPACITY && !t.contains_key(&dest) {
                // The dead first: a table full of expired routes must never evict a live one.
                t.retain(|_, e| now.duration_since(e.learned) < PATH_TTL);
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
            t.insert(
                dest,
                PathEntry {
                    iface,
                    transport,
                    hops,
                    learned: now,
                },
            );
        }
    }

    /// The interface to reach `dest`, if a route is known and unexpired. Evicts an expired
    /// route as a side effect, so a stale path never lingers past a lookup.
    fn path_iface(&self, dest: AddressHash) -> Option<InterfaceId> {
        let mut t = self.path_table.lock().unwrap();
        match t.get(&dest) {
            Some(e) if e.learned.elapsed() < PATH_TTL => Some(e.iface),
            Some(_) => {
                t.remove(&dest);
                None
            }
            None => None,
        }
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
            interfaces: Mutex::new(Vec::new()),
            router_tx,
            accepted_tx,
            reliable_accepted_tx,
            resource_accepted_tx,
            announce_tx,
            single_tx,
            pending: Mutex::new(HashMap::new()),
            pending_links: Mutex::new(HashMap::new()),
            next_iface_id: AtomicU32::new(0),
            routing: Mutex::new(RoutingPolicy::none()),
            routing_stats: RoutingStats::default(),
            relay_jitter_ms: AtomicU64::new(0),
            reliable_initial_rtt_ms: AtomicU64::new(DEFAULT_RELIABLE_INITIAL_RTT_MS),
            reliable_max_window: AtomicU32::new(DEFAULT_RELIABLE_MAX_WINDOW),
            link_setup_retry_ms: AtomicU64::new(DEFAULT_LINK_SETUP_RETRY_MS),
            link_mtu: AtomicU32::new(DEFAULT_LINK_MTU),
            inbound_link_proofs: Mutex::new(HashMap::new()),
            path_table: Mutex::new(HashMap::new()),
            seen_announces: Mutex::new((HashSet::new(), VecDeque::new())),
            announce_admission: Mutex::new(
                AnnounceAdmission::new(AnnounceIngressPolicy::default()),
            ),
            announce_admission_started: Instant::now(),
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
                route(&router, iface, pkt);
            }
        });

        Self {
            shared,
            accepted_rx: AsyncMutex::new(accepted_rx),
            reliable_accepted_rx: AsyncMutex::new(reliable_accepted_rx),
            resource_accepted_rx: AsyncMutex::new(resource_accepted_rx),
            announce_rx: AsyncMutex::new(announce_rx),
            single_rx: AsyncMutex::new(single_rx),
        }
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

    /// Configure bounded announce ingress control for subsequently observed packets. Existing
    /// histories remain, trimmed to the new capacities, so changing a setting cannot turn an
    /// unbounded backlog into a hidden one.
    pub fn set_announce_ingress_policy(&self, policy: AnnounceIngressPolicy) {
        self.shared
            .announce_admission
            .lock()
            .unwrap()
            .set_policy(policy);
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

    /// Set the first reliable-channel RTT estimate for subsequently opened links.
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

    /// Set the retry interval for link requests sent by subsequently opened links.
    pub fn set_link_setup_retry(&self, interval: Duration) {
        let millis = interval.as_millis().clamp(1, u128::from(u64::MAX)) as u64;
        self.shared
            .link_setup_retry_ms
            .store(millis, Ordering::Relaxed);
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
        let mut t = self.shared.path_table.lock().unwrap();
        match t.get(&dest) {
            Some(e) if e.learned.elapsed() < PATH_TTL => Some((e.iface, e.hops)),
            Some(_) => {
                t.remove(&dest);
                None
            }
            None => None,
        }
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
        let pkt = announce::build(
            &self.shared.identity,
            name.name_hash(),
            &rand_hash(),
            ratchet.as_ref(),
            app_data,
        );
        self.shared.broadcast(pkt);
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
        let (link, iface) = self.establish(dest, peer).await?;
        register_stream(&self.shared, link, iface).ok_or_else(endpoint_closed)
    }

    /// Open a **reliable** link to a destination — the Channel/Buffer path with proof acks,
    /// for lossy interfaces — and return its stream. `peer` is the destination's identity: the
    /// handshake authenticates it, and the peer's proofs of our packets are validated against
    /// it. As the initiator, the reliable driver IDENTIFYs us to the responder so it can
    /// validate our proofs in turn.
    pub async fn open_reliable(&self, dest: AddressHash, peer: Identity) -> io::Result<LinkStream> {
        let (link, iface) = self.establish(dest, peer).await?;
        register_reliable_stream(&self.shared, link, iface, Some(peer)).ok_or_else(endpoint_closed)
    }

    /// Open a link whose packets are driven by the resource transfer state machines.
    pub async fn open_resource(
        &self,
        dest: AddressHash,
        peer: Identity,
    ) -> io::Result<ResourceSession> {
        let (link, iface) = self.establish(dest, peer).await?;
        register_resource_session(&self.shared, link, iface).ok_or_else(endpoint_closed)
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
        let (link, iface) = self.establish(dest, peer).await?;
        if data.len() <= write_chunk_for_mtu(link.mtu()) {
            let mut stream =
                register_stream(&self.shared, link, iface).ok_or_else(endpoint_closed)?;
            stream.write_all(data).await?;
            stream.shutdown().await?;
            drop(stream);
            Ok(PayloadMode::Data)
        } else {
            let mut session =
                register_resource_session(&self.shared, link, iface).ok_or_else(endpoint_closed)?;
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

    /// Establish a link to `dest` (whose identity is `peer`), returning it with the interface
    /// its proof arrived on. The stream discipline is chosen by the caller.
    async fn establish(
        &self,
        dest: AddressHash,
        peer: Identity,
    ) -> io::Result<(Link, InterfaceId)> {
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
        let send_request = || match self.shared.path_iface(dest) {
            Some(iface) => self.shared.send_on(iface, request.clone()),
            None => self.shared.broadcast(request.clone()),
        };
        send_request();

        let retry = Duration::from_millis(self.shared.link_setup_retry_ms.load(Ordering::Relaxed));
        let mut retries = tokio::time::interval_at(tokio::time::Instant::now() + retry, retry);
        retries.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let deadline = tokio::time::sleep(LINK_SETUP_TIMEOUT);
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
                    Ok(established) => {
                        guard.armed = false; // router removed both entries on success
                        if self.shared.is_running() {
                            // The responder does not activate an inbound link until the
                            // initiator reports its measured RTT. Keep this ahead of any
                            // application packet emitted by the returned session.
                            self.shared.send_on(
                                established.1,
                                established.0.rtt_packet(0.05, &next_iv()),
                            );
                            return Ok(established);
                        }
                        self.shared
                            .send_on(established.1, established.0.close_packet(&next_iv()));
                        return Err(endpoint_closed());
                    }
                    Err(_) => return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "link setup dropped",
                    )),
                },
                _ = retries.tick() => send_request(),
                _ = &mut deadline => return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "link setup timed out",
                )),
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
        recv_until_closed(&self.shared, &self.accepted_rx).await
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
        recv_until_closed(&self.shared, &self.reliable_accepted_rx).await
    }

    /// Wait for an inbound resource link, including the destination it targeted.
    pub async fn accept_resource(&self) -> io::Result<AcceptedResource> {
        recv_until_closed(&self.shared, &self.resource_accepted_rx).await
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
        self.shared.links.lock().unwrap().clear();
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
            let policy = owner.routing.lock().unwrap().clone();
            process_verified_announce(&owner, iface, held.packet, held.announce, &policy);
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
    policy: &RoutingPolicy,
) {
    // The book's answer is the cap, and ignoring it made the cap decorative: a refused peer
    // still got a path-table entry, still went out the announce channel, and still relayed
    // onward, so the only bounded structure was the one thing that did not grow.
    if shared.address_book.lock().unwrap().ingest(&announce)
        == crate::address_book::Ingested::Refused
    {
        shared
            .routing_stats
            .refused_announces
            .fetch_add(1, Ordering::Relaxed);
        return;
    }
    let destination = announce.destination;
    // A header-type-2 announce names the transport node forwarding it. It belongs to this
    // destination's route, not to the interface: the same radio routinely reaches different
    // destinations through different nodes.
    shared.learn_path(destination, iface, pkt.hops, pkt.transport);
    let _ = shared.announce_tx.send(PeerAnnounce {
        destination,
        identity: announce.identity,
        app_data: announce.app_data,
    });

    // As a transport node, propagate the announce onward: hops+1, stamped with our identity
    // as the transport node so downstream peers address replies through us, out every
    // permitted interface but the one it came in on, de-duplicated by packet hash.
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

/// Dispatch one inbound packet that arrived on `iface`.
fn route(shared: &Arc<Shared>, iface: InterfaceId, pkt: Packet) {
    // A path request for a destination we own: answer it with a path response (an announce
    // carrying context 0x0b) so a peer that lost its route to us can rediscover it. We answer
    // only for our own destinations; with no announce cache we cannot answer for others.
    if pkt.packet_type == PacketType::Data
        && pkt.destination_type == DestinationType::Plain
        && let Some(target) = crate::path::parse_request(&pkt)
    {
        if let Some(resp) = shared.path_response(target) {
            shared.broadcast(resp);
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
        // Traffic on a bridge is proof it is still wanted, so a busy link keeps its entry
        // and only a silent one ages out.
        let bridged = {
            let mut bridges = shared.link_transport.lock().unwrap();
            let now = Instant::now();
            match bridges.get_mut(&pkt.destination) {
                Some((from, out, seen)) if now.duration_since(*seen) < LINK_TRANSPORT_TTL => {
                    *seen = now;
                    Some((*from, *out))
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
                Some((a, b)) => forward_on(shared, if iface == a { b } else { a }, pkt, &policy),
                None => forward(shared, iface, pkt, &policy),
            }
            return;
        }
    }
    match pkt.packet_type {
        PacketType::Announce => {
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
                        process_verified_announce(shared, iface, pkt, a, &policy);
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
                let ephemeral = ephemeral_seed();
                let configured_mtu = shared.link_mtu.load(Ordering::Relaxed);
                let requested_mtu = pkt
                    .payload
                    .get(link::LINK_KEYS_LEN..link::LINK_KEYS_LEN + link::TRAILER_LEN)
                    .and_then(|bytes| bytes.try_into().ok())
                    .and_then(|trailer| LinkTrailer::decode(trailer).ok())
                    .map(|trailer| trailer.mtu)
                    .unwrap_or(configured_mtu);
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
                    shared.send_on(iface, proof);
                    match kind {
                        RegistrationKind::Reliable => {
                            // Register eagerly with no peer yet: the driver learns the
                            // initiator's identity from the IDENTIFY it sends.
                            if let Some(stream) =
                                register_reliable_stream(shared, link, iface, None)
                            {
                                let _ = shared.reliable_accepted_tx.send(Accepted {
                                    stream,
                                    destination: dest,
                                    interface: iface,
                                });
                            }
                        }
                        RegistrationKind::Resource => {
                            if let Some(session) = register_resource_session(shared, link, iface) {
                                let _ = shared.resource_accepted_tx.send(AcceptedResource {
                                    session,
                                    destination: dest,
                                    interface: iface,
                                });
                            }
                        }
                        RegistrationKind::BestEffort => {
                            if let Some(stream) = register_stream(shared, link, iface) {
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
                if let Some(packets) = packets {
                    let _ = packets.send(pkt);
                }
            }
        }
        PacketType::Data => {
            // Link data: route to the matching stream by its delivery discipline. Clone the
            // sender(s) under the lock, then act on the packet once the lock is released.
            let (raw, best) = {
                let links = shared.links.lock().unwrap();
                match links.get(&pkt.destination) {
                    Some(e) => match &e.kind {
                        LinkKind::Reliable { packets } | LinkKind::Resource { packets } => {
                            (Some(packets.clone()), None)
                        }
                        LinkKind::BestEffort { inbound } => {
                            (None, Some((e.link.clone(), inbound.clone())))
                        }
                    },
                    None => (None, None),
                }
            };
            if let Some(packets) = raw {
                // The reliable or resource driver owns this packet; hand it over raw.
                let _ = packets.send(pkt);
            } else if let Some((link, inbound)) = best {
                match link.receive(&pkt) {
                    Some(Inbound::Data(bytes)) => {
                        let _ = inbound.send(bytes);
                    }
                    Some(Inbound::Close) => {
                        // The peer closed the link: drop its entry so the inbound
                        // sender is released. The stream's inbound relay then ends
                        // and the local reader sees EOF (what read-to-end needs).
                        shared.links.lock().unwrap().remove(&pkt.destination);
                    }
                    _ => {}
                }
            } else if pkt.destination_type == DestinationType::Single {
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
        if pkt.packet_type == PacketType::LinkRequest
            && let Ok(link_id) = link::link_id(&pkt)
        {
            let mut bridges = shared.link_transport.lock().unwrap();
            // Prune before inserting. These entries were never removed: every link this
            // node ever bridged stayed in the map for the life of the process, so a busy
            // transport node's memory tracked its lifetime traffic rather than its live
            // links. Pruning here rather than on a timer keeps the work proportional to
            // the thing causing the growth.
            let now = Instant::now();
            bridges.retain(|_, (_, _, seen)| now.duration_since(*seen) < LINK_TRANSPORT_TTL);
            bridges.insert(link_id, (from, out, now));
        }
        forward_on(shared, out, pkt, policy);
    }
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
fn forward_on(shared: &Arc<Shared>, out: InterfaceId, mut pkt: Packet, policy: &RoutingPolicy) {
    if !policy.allowed_egress.allows(out) {
        shared
            .routing_stats
            .policy_rejected
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
    shared
        .routing_stats
        .forwarded_packets
        .fetch_add(1, Ordering::Relaxed);
    // Carried traffic queues as transit, so it can never outcompete this node's own.
    pkt.hops += 1;
    pkt.header_type = crate::packet::HeaderType::Type1;
    pkt.transport = None;
    shared.send_on_class(out, pkt, TrafficClass::Transit);
}

/// Register a link for endpoint-driven resource packets.
fn register_resource_session(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
) -> Option<ResourceSession> {
    if !shared.begin_resource() {
        shared.send_on(iface, link.close_packet(&next_iv()));
        return None;
    }
    let (packet_tx, packets) = mpsc::unbounded_channel();
    shared.links.lock().unwrap().insert(
        link.id(),
        LinkEntry {
            link: link.clone(),
            kind: LinkKind::Resource { packets: packet_tx },
            iface,
        },
    );
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
fn register_stream(shared: &Arc<Shared>, link: Link, iface: InterfaceId) -> Option<LinkStream> {
    let (mine, theirs) = tokio::io::duplex(DUPLEX_BUF);
    let (mut read_half, mut write_half) = tokio::io::split(theirs);
    let (inbound_tx, mut inbound_rx) = mpsc::unbounded_channel::<Vec<u8>>();
    let link_id = link.id();
    let write_chunk = write_chunk_for_mtu(link.mtu());

    shared.links.lock().unwrap().insert(
        link_id,
        LinkEntry {
            link: link.clone(),
            kind: LinkKind::BestEffort {
                inbound: inbound_tx,
            },
            iface,
        },
    );

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
    let outbound_started = track_drainable(shared, async move {
        let mut buf = vec![0u8; write_chunk];
        loop {
            match read_half.read(&mut buf).await {
                Ok(0) | Err(_) => {
                    // The stream was shut down or dropped: close the link so the
                    // peer's read side sees EOF. This is what lets a read-to-end
                    // protocol (e.g. gemini) end a response by closing the stream.
                    iv_shared.send_on(iface, out_link.close_packet(&next_iv()));
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
        shared.links.lock().unwrap().remove(&link_id);
        return None;
    }

    Some(LinkStream {
        inner: mine,
        link_id,
        iface,
    })
}

/// Build a **reliable** [`LinkStream`] for a live link: the RNS Channel/Buffer path with
/// link-proof acks (see [`crate::reliable`]). A single driver task owns the
/// [`ReliableChannel`] and pumps it — app writes in, ordered bytes out, a proof per
/// delivered packet, an inbound proof releasing its sequence, and retransmits on a clock —
/// so the stream stays honest over a lossy interface. `peer` is the identity whose proofs
/// this side validates: `Some` for an initiator (the destination's identity from its
/// announce), `None` for a responder, which learns the initiator's identity from the IDENTIFY
/// the initiator sends. An initiator also sends its own IDENTIFY so the responder can validate
/// it in turn.
fn register_reliable_stream(
    shared: &Arc<Shared>,
    link: Link,
    iface: InterfaceId,
    peer: Option<Identity>,
) -> Option<LinkStream> {
    let (mine, theirs) = tokio::io::duplex(DUPLEX_BUF);
    let (mut read_half, mut write_half) = tokio::io::split(theirs);
    let (pkt_tx, mut pkt_rx) = mpsc::unbounded_channel::<Packet>();
    let link_id = link.id();

    shared.links.lock().unwrap().insert(
        link_id,
        LinkEntry {
            link: link.clone(),
            kind: LinkKind::Reliable { packets: pkt_tx },
            iface,
        },
    );

    // An initiator (known peer) identifies itself so the responder can validate our proofs.
    let identify = peer
        .is_some()
        .then(|| link.identify_packet(&shared.identity, &next_iv()));
    let close_link = link.clone();
    let initial_rtt_ms = shared.reliable_initial_rtt_ms.load(Ordering::Relaxed);
    let max_window = shared.reliable_max_window.load(Ordering::Relaxed);
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
    let drv = Arc::clone(shared);
    let driver_started = track_drainable(shared, async move {
        // Identify to the responder so it can validate our proofs. RNS sends this once; we
        // re-send it over the first few ticks (in the clock arm below) so a dropped one still
        // lands on a lossy medium.
        if let Some(id_packet) = &identify {
            drv.send_on(iface, id_packet.clone());
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

        loop {
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
                        let bytes = rc.read();
                        if !bytes.is_empty() && write_half.write_all(&bytes).await.is_err() {
                            break;
                        }
                        if rc.recv_finished() {
                            // The peer's stream ended: close our read side so the app's
                            // reader sees EOF. We keep running to finish our own sending.
                            let _ = write_half.shutdown().await;
                            peer_done = true;
                        }
                    } else if pkt.context == CTX_LINKIDENTIFY {
                        // The peer (an initiator) identified itself: learn its identity so we
                        // can validate its proofs of the data we send back.
                        rc.on_identify(&pkt);
                    } else if pkt.context == CTX_LINKCLOSE {
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
                    if let Some(id_packet) = &identify
                        && identify_sends < IDENTIFY_MAX_SENDS
                    {
                        drv.send_on(iface, id_packet.clone());
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
        drv.links.lock().unwrap().remove(&link_id);
    });

    if !driver_started {
        shared.links.lock().unwrap().remove(&link_id);
        return None;
    }

    Some(LinkStream {
        inner: mine,
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

/// A fresh 10-byte announce randomness value.
fn rand_hash() -> [u8; RAND_HASH_LEN] {
    let mut out = [0u8; RAND_HASH_LEN];
    fill_random(&mut out);
    out
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
        ep.shared.learn_path(dest, 7, 2, None);
        assert_eq!(ep.route_to(dest), Some((7, 2)), "a fresh route is returned");

        // PATH_TTL is short under cfg(test); wait past it.
        tokio::time::sleep(PATH_TTL + Duration::from_millis(40)).await;

        assert_eq!(ep.route_to(dest), None, "an expired route is not returned");
        assert!(
            !ep.shared.path_table.lock().unwrap().contains_key(&dest),
            "and is evicted on lookup",
        );
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

    /// A worse alternate route used to refresh the preferred one's learned time, so a
    /// preferred path that had died never aged out as long as any second path kept
    /// announcing. Failover could not happen.
    #[tokio::test]
    async fn a_worse_alternate_does_not_keep_a_dead_route_alive() {
        let ep = Endpoint::new(PrivateIdentity::from_secret_bytes(&[0x32; 64]));
        let dest = AddressHash::from_bytes([0xCC; 16]);
        let good = InterfaceId::from(1_u32);
        let worse = InterfaceId::from(2_u32);

        ep.shared.learn_path(dest, good, 1, None);
        let first = ep
            .shared
            .path_table
            .lock()
            .unwrap()
            .get(&dest)
            .unwrap()
            .learned;

        // No sleep: PATH_TTL is 60ms under cfg(test), and pausing inside that window made
        // this test fail under a loaded parallel run for the honest reason -- the route had
        // genuinely expired and was correctly replaced. Back to back keeps the question
        // about refreshing rather than about expiry.
        // A longer route arrives on another interface. It is correctly not adopted, and it
        // must not vouch for the route it lost to either.
        ep.shared.learn_path(dest, worse, 5, None);

        let entry = *ep.shared.path_table.lock().unwrap().get(&dest).unwrap();
        assert_eq!(entry.iface, good, "the shorter route is still preferred");
        assert_eq!(
            entry.learned, first,
            "and its age is untouched, so it can expire and let failover happen",
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
