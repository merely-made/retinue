//! `ResourceSession`: a link driven by the Resource transfer state machines.

use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::hash::AddressHash;
use crate::identity::Identity;
use crate::link::{Inbound, Link};
use crate::link_liveness::Liveness;
use crate::packet::Packet;
use crate::resource::{Advertisement, RANDOM_HASH_LEN};
use crate::resource_transfer::{
    DEFAULT_MAX_RESOURCE_SIZE, ResourceKind, ResourceReceiver, SegmentedReceiver, SegmentedSender,
};

use super::entropy::{fill_random, next_iv};
use super::facts::{LinkDirection, LinkRemoteFact};
use super::interface::InterfaceId;
use super::shared::{LinkEntry, LinkKind, Shared};
use super::stream::LINK_QUEUE;

/// Copies of a completed Resource's proof sent when the receive returns. A receiver often
/// drops its session, and the kept proof with it, as soon as it has the data; RNS ignores a
/// proof for a concluded resource, so the extra copies cost only airtime.
const RESOURCE_PROOF_MAX_SENDS: u32 = 3;

/// Runtime policy for an endpoint-driven resource transfer.
#[derive(Clone, Copy, Debug)]
pub struct ResourceTransferConfig {
    /// Maximum time allowed for the complete transfer, every segment of a split Resource
    /// included.
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

/// What [`ResourceSession::receive`] delivered.
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

/// A live link whose raw packets are driven by the resource transfer state machines.
///
/// One session carries one transfer at a time: one peer publishes, the other fetches.
pub struct ResourceSession {
    pub(super) shared: Arc<Shared>,
    pub(super) link: Link,
    pub(super) iface: InterfaceId,
    pub(super) packets: mpsc::Receiver<Packet>,
    pub(super) config: ResourceTransferConfig,
    pub(super) identified_peer: Option<Identity>,
    accept: Option<Arc<ResourceAccept>>,
    metadata: Option<Vec<u8>>,
    pub(super) max_resource_size: usize,
    pub(super) max_request_size: Option<usize>,
}

/// A resource accept policy shared by every receive on a session; see
/// [`ResourceSession::set_accept`].
type ResourceAccept = dyn Fn(&Advertisement) -> bool + Send + Sync;

/// How many quiet retry intervals a publisher that has sent every part waits between cache
/// requests for its proof. RNS waits about three round trips plus a grace period.
const PROOF_WAIT_RETRIES: u32 = 4;

/// How long a link keeps the last resource proof it sent, to answer the publisher's cache
/// request if the proof was lost.
pub(super) const RESOURCE_PROOF_CACHE_TTL: Duration = Duration::from_secs(120);

impl ResourceSession {
    /// The id of the link carrying this resource session.
    pub fn link_id(&self) -> AddressHash {
        self.link.id()
    }

    /// The interface this resource link arrived on.
    pub fn interface(&self) -> InterfaceId {
        self.iface
    }

    pub(super) fn retain_identified_peer(&self, identity: Identity) {
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

    /// Decide on each Resource offered to [`fetch`](Self::fetch) or
    /// [`receive`](Self::receive) from its advertisement, as an RNS link's `ACCEPT_APP`
    /// callback does. A refused offer is rejected on the wire, so the sender stops, and the
    /// call fails with [`io::ErrorKind::PermissionDenied`].
    pub fn set_accept(&mut self, accept: impl Fn(&Advertisement) -> bool + Send + Sync + 'static) {
        self.accept = Some(Arc::new(accept));
    }

    /// Refuse a received Resource larger than `max_size` bytes in all, metadata included.
    /// A Resource past one segment ([`MAX_SEGMENT_SIZE`]) is held whole until it
    /// completes, so this caps the memory one receive may take. The default is
    /// [`DEFAULT_MAX_RESOURCE_SIZE`].
    ///
    /// [`MAX_SEGMENT_SIZE`]: crate::resource::MAX_SEGMENT_SIZE
    pub fn set_max_resource_size(&mut self, max_size: usize) {
        self.max_resource_size = max_size;
    }

    /// Drop a request whose packed size exceeds `max_size`, as RNS's
    /// `Destination.max_request_size` does: a request packet is ignored and a request
    /// Resource rejected (`Link.py` 999-1000, 1036-1043). The default, `None`, leaves only
    /// [`set_max_resource_size`](Self::set_max_resource_size)'s cap on request Resources.
    pub fn set_max_request_size(&mut self, max_size: Option<usize>) {
        self.max_request_size = max_size;
    }

    /// The packed (msgpack) metadata attached to the last Resource received on this
    /// session, taken. `None` if it carried none.
    pub fn take_metadata(&mut self) -> Option<Vec<u8>> {
        self.metadata.take()
    }

    /// Receivers for inbound Resources under this session's policy: its window and size
    /// cap, `max_data_size` for each segment's advertised total, and, for application
    /// Resources but not requests or responses (`Link.py` 1035-1076), its accept hook.
    pub(super) fn receivers(
        &self,
        max_data_size: Option<usize>,
        with_accept: bool,
    ) -> impl Fn() -> SegmentedReceiver + Send + Sync + 'static {
        let link = self.link.clone();
        let window = self.config.request_window;
        let accept = self.accept.clone().filter(|_| with_accept);
        let max_size = max_data_size.map_or(self.max_resource_size, |max| {
            max.min(self.max_resource_size)
        });
        move || {
            let link = link.clone();
            let accept = accept.clone();
            SegmentedReceiver::new(link.clone(), move || {
                let mut receiver = ResourceReceiver::with_request_window(link.clone(), window);
                if let Some(max) = max_data_size {
                    receiver = receiver.with_max_data_size(max);
                }
                match &accept {
                    Some(accept) => {
                        let accept = Arc::clone(accept);
                        receiver.with_accept(move |advertisement| accept(advertisement))
                    }
                    None => receiver,
                }
            })
            .with_max_size(max_size)
        }
    }

    /// Publish one payload with metadata, one already-packed msgpack value an RNS receiver
    /// reads as `resource.metadata`, and wait until the receiver proves complete receipt.
    pub async fn publish_with_metadata(&mut self, data: &[u8], metadata: &[u8]) -> io::Result<()> {
        let mut random_hash = [0_u8; RANDOM_HASH_LEN];
        fill_random(&mut random_hash);
        let sender = SegmentedSender::new(
            self.link.clone(),
            data,
            Some(metadata),
            ResourceKind::Data,
            random_hash,
            &next_iv(),
        )
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.publish_sender(sender).await
    }

    /// Publish one payload and wait until the receiver proves complete receipt. A payload
    /// past one segment ([`MAX_SEGMENT_SIZE`]) goes as RNS segments, each proved before
    /// the next is advertised; `timeout` covers the whole transfer.
    ///
    /// [`MAX_SEGMENT_SIZE`]: crate::resource::MAX_SEGMENT_SIZE
    pub async fn publish(&mut self, data: &[u8]) -> io::Result<()> {
        let sender = self.sender(data, ResourceKind::Data)?;
        self.publish_sender(sender).await
    }

    /// A sender for `data` of `kind` on this session's link.
    pub(super) fn sender<'a>(
        &self,
        data: &'a [u8],
        kind: ResourceKind,
    ) -> io::Result<SegmentedSender<&'a [u8]>> {
        let mut random_hash = [0_u8; RANDOM_HASH_LEN];
        fill_random(&mut random_hash);
        SegmentedSender::new(self.link.clone(), data, None, kind, random_hash, &next_iv())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    pub(super) async fn publish_sender<D: AsRef<[u8]>>(
        &mut self,
        mut sender: SegmentedSender<D>,
    ) -> io::Result<()> {
        self.shared
            .send_on(self.iface, sender.advertisement(&next_iv()));

        let shared = Arc::clone(&self.shared);
        let iface = self.iface;
        let link = self.link.clone();
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let publishing = &mut sender;
        let transfer = async move {
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            let mut quiet = 0_u32;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                        })?;
                        if link.receive(&packet) == Some(Inbound::Close) {
                            return Err(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "resource link closed",
                            ));
                        }
                        quiet = 0;
                        for outbound in publishing.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        if publishing.is_done() {
                            return Ok(());
                        } else if publishing.is_canceled() {
                            return Err(io::Error::new(
                                io::ErrorKind::ConnectionAborted,
                                "resource publish canceled by receiver",
                            ));
                        }
                    }
                    _ = interval.tick() => {
                        quiet += 1;
                        publish_tick(&shared, iface, publishing, quiet)?;
                    }
                }
            }
        };
        match tokio::time::timeout(self.config.timeout, transfer).await {
            Ok(result) => result,
            Err(_) => {
                // Tell the receiver to stop rather than leave it requesting into silence.
                if let Some(cancel) = sender.cancel(&next_iv()) {
                    self.shared.send_on(self.iface, cancel);
                }
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
    ///
    /// Metadata the publisher attached is kept for [`take_metadata`](Self::take_metadata).
    pub async fn fetch(&mut self) -> io::Result<Vec<u8>> {
        let mut receiver = self.receivers(None, true)();
        let shared = Arc::clone(&self.shared);
        let iface = self.iface;
        let link = self.link.clone();
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let receiving = &mut receiver;
        let transfer = async move {
            let mut kept = 0;
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                        })?;
                        if link.receive(&packet) == Some(Inbound::Close) {
                            return Err(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "resource link closed",
                            ));
                        }
                        for outbound in receiving.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        keep_resource_proofs(&shared, iface, link.id(), receiving, &mut kept);
                        if receiving.is_complete() {
                            return Ok(());
                        }
                        resource_receive_ended(receiving)?;
                    }
                    _ = interval.tick() => {
                        for outbound in receiving.retransmit(next_iv) {
                            shared.send_on(iface, outbound);
                        }
                    }
                }
            }
        };
        let outcome = tokio::time::timeout(self.config.timeout, transfer).await;
        self.settle_receive(&mut receiver, outcome, "resource fetch timed out")?;
        Ok(self.take_received(&mut receiver))
    }

    /// Settle a receive that ended: on a timeout, tell the sender to stop.
    pub(super) fn settle_receive<T>(
        &self,
        receiver: &mut SegmentedReceiver,
        outcome: Result<io::Result<T>, tokio::time::error::Elapsed>,
        timed_out: &'static str,
    ) -> io::Result<T> {
        outcome.unwrap_or_else(|_| {
            if let Some(cancel) = receiver.cancel(&next_iv()) {
                self.shared.send_on(self.iface, cancel);
            }
            Err(io::Error::new(io::ErrorKind::TimedOut, timed_out))
        })
    }

    /// Take a completed receiver's payload, keeping its metadata for
    /// [`take_metadata`](Self::take_metadata).
    fn take_received(&mut self, receiver: &mut SegmentedReceiver) -> Vec<u8> {
        let (data, metadata) = receiver
            .take_payload()
            .expect("a completed receiver holds its payload");
        self.metadata = metadata;
        data
    }

    /// Receive either one best-effort data packet or one complete Resource.
    ///
    /// Protocols such as LXMF use both forms on one destination: register it with
    /// [`Endpoint::register_resource`] and receive here. Metadata a Resource carried is kept
    /// for [`take_metadata`](Self::take_metadata). A data packet that arrives while a
    /// Resource is in progress returns at once, abandoning that Resource; its sender is left
    /// to time out.
    ///
    /// [`Endpoint::register_resource`]: super::Endpoint::register_resource
    pub async fn receive(&mut self) -> io::Result<ReceivedPayload> {
        let mut receiver = self.receivers(None, true)();
        let shared = Arc::clone(&self.shared);
        let link = self.link.clone();
        let iface = self.iface;
        let packets = &mut self.packets;
        let retry = self.config.retry_interval;
        let mut identified = self.identified_peer;
        let receiving = &mut receiver;
        let transfer = async move {
            let mut kept = 0;
            let mut interval = tokio::time::interval(retry);
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            interval.tick().await;
            loop {
                tokio::select! {
                    maybe = packets.recv() => {
                        let packet = maybe.ok_or_else(|| {
                            io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                        })?;
                        // The sender's IDENTIFY, signed under the link: what authenticates a
                        // first message from a peer we have never heard announce.
                        if let Some(identity) = link.read_identify(&packet) {
                            identified = Some(identity);
                            continue;
                        }
                        match link.receive(&packet) {
                            Some(Inbound::Data(data)) => {
                                return Ok((identified, Some(data)));
                            }
                            Some(Inbound::Close) => {
                                return Err(io::Error::new(
                                    io::ErrorKind::BrokenPipe,
                                    "resource link closed",
                                ));
                            }
                            _ => {}
                        }
                        for outbound in receiving.on_packet(&packet, next_iv) {
                            shared.send_on(iface, outbound);
                        }
                        keep_resource_proofs(&shared, iface, link.id(), receiving, &mut kept);
                        if receiving.is_complete() {
                            return Ok((identified, None));
                        }
                        resource_receive_ended(receiving)?;
                    }
                    _ = interval.tick() => {
                        for outbound in receiving.retransmit(next_iv) {
                            shared.send_on(iface, outbound);
                        }
                    }
                }
            }
        };
        let outcome = tokio::time::timeout(self.config.timeout, transfer).await;
        let (identified, data) =
            self.settle_receive(&mut receiver, outcome, "payload receive timed out")?;
        self.identified_peer = identified;
        if let Some(identity) = identified {
            self.retain_identified_peer(identity);
        }
        Ok(match data {
            Some(data) => ReceivedPayload::Data(data),
            None => ReceivedPayload::Resource(self.take_received(&mut receiver)),
        })
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
/// accumulation, one past a size or part ceiling or refused by the accept policy, or one
/// whose body did not recover. The receiver has already sent the sender its cancel.
fn resource_receive_failure(error: crate::Error) -> io::Error {
    let kind = match error {
        crate::Error::MultiSegmentResource => io::ErrorKind::Unsupported,
        crate::Error::ResourceRejected => io::ErrorKind::PermissionDenied,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, error)
}

/// The error ending a receive whose transfer failed or was canceled by the sender, if it
/// has.
pub(super) fn resource_receive_ended(receiver: &SegmentedReceiver) -> io::Result<()> {
    if let Some(error) = receiver.failure() {
        Err(resource_receive_failure(error))
    } else if receiver.is_canceled() {
        Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "resource canceled by sender",
        ))
    } else {
        Ok(())
    }
}

/// Keep each segment proof the receiver sent since `kept` segments, for its link, so the
/// router can answer the sender's cache request, or its re-advertisement, if it was lost.
/// The final proof also gets the copies after the one sent with completion (see
/// [`RESOURCE_PROOF_MAX_SENDS`]).
pub(super) fn keep_resource_proofs(
    shared: &Shared,
    iface: InterfaceId,
    link: AddressHash,
    receiver: &SegmentedReceiver,
    kept: &mut usize,
) {
    if receiver.segments_proved() == *kept {
        return;
    }
    *kept = receiver.segments_proved();
    if let Some(proof) = receiver.last_proof() {
        if receiver.is_complete() {
            for _ in 1..RESOURCE_PROOF_MAX_SENDS {
                shared.send_on(iface, proof.clone());
            }
        }
        shared.keep_resource_proof(link, proof.clone());
    }
}

/// One quiet retry interval of a publish: re-advertise until the receiver starts, then,
/// with every part sent and no proof back, ask the receiver's cache, as RNS does, until
/// those requests run out.
pub(super) fn publish_tick<D: AsRef<[u8]>>(
    shared: &Shared,
    iface: InterfaceId,
    sender: &mut SegmentedSender<D>,
    quiet: u32,
) -> io::Result<()> {
    if !sender.has_started() {
        shared.send_on(iface, sender.advertisement(&next_iv()));
    } else if quiet.is_multiple_of(PROOF_WAIT_RETRIES) && sender.awaiting_proof() {
        if let Some(request) = sender.cache_request() {
            shared.send_on(iface, request);
        } else {
            if let Some(cancel) = sender.cancel(&next_iv()) {
                shared.send_on(iface, cancel);
            }
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "resource proof never arrived",
            ));
        }
    }
    Ok(())
}

/// Register a link for endpoint-driven resource packets.
pub(super) fn register_resource_session(
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
        accept: None,
        metadata: None,
        max_resource_size: DEFAULT_MAX_RESOURCE_SIZE,
        max_request_size: None,
    })
}
