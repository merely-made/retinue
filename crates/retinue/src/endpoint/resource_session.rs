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
    Timing,
};

pub(super) use super::resource_pace::Pace;
pub use super::resource_pace::ResourceTransferConfig;

use super::entropy::{fill_random, next_iv};
use super::facts::{LinkDirection, LinkRemoteFact};
use super::interface::InterfaceId;
use super::shared::{LinkEntry, LinkKind, Shared};
use super::stream::LINK_QUEUE;

/// Copies of a completed Resource's proof sent when the receive returns. A receiver often
/// drops its session, and the kept proof with it, as soon as it has the data; RNS ignores a
/// proof for a concluded resource, so the extra copies cost only airtime.
const RESOURCE_PROOF_MAX_SENDS: u32 = 3;

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
    /// Whether the peer opened this link. Only a responder takes an IDENTIFY (`Link.py` 973).
    pub(super) responder: bool,
    accept: Option<Arc<ResourceAccept>>,
    metadata: Option<Vec<u8>>,
    pub(super) max_resource_size: usize,
    pub(super) max_request_size: Option<usize>,
}

/// A resource accept policy shared by every receive on a session; see
/// [`ResourceSession::set_accept`].
type ResourceAccept = dyn Fn(&Advertisement) -> bool + Send + Sync;

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

    /// Replace the retry and silence policy for subsequent transfer work.
    pub fn set_config(&mut self, config: ResourceTransferConfig) {
        self.config = config;
    }

    /// The link's transfer timing: its measured RTT, else the configured retry interval.
    fn timing(&self) -> Timing {
        let floor = self
            .config
            .retry_interval
            .as_millis()
            .min(u128::from(u64::MAX)) as u64;
        let rtt = self
            .shared
            .links
            .lock()
            .unwrap()
            .get(&self.link.id())
            .and_then(|entry| entry.liveness.rtt());
        Timing {
            rtt: rtt.unwrap_or(floor),
            floor,
        }
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

    pub(super) fn set_metadata(&mut self, metadata: Option<Vec<u8>>) {
        self.metadata = metadata;
    }

    /// Receivers for inbound Resources under this session's policy: its window, timing and
    /// size cap, `max_data_size` for each segment's advertised total, and, for application
    /// Resources but not requests or responses (`Link.py` 1035-1076), its accept hook.
    pub(super) fn receivers(
        &self,
        max_data_size: Option<usize>,
        with_accept: bool,
    ) -> impl Fn() -> SegmentedReceiver + Send + Sync + 'static {
        let link = self.link.clone();
        let window = self.config.request_window;
        let timing = self.timing();
        let accept = self.accept.clone().filter(|_| with_accept);
        let max_size = max_data_size.map_or(self.max_resource_size, |max| {
            max.min(self.max_resource_size)
        });
        move || {
            let link = link.clone();
            let accept = accept.clone();
            SegmentedReceiver::new(link.clone(), move || {
                let mut receiver =
                    ResourceReceiver::with_request_window(link.clone(), window).with_timing(timing);
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
    /// the next is advertised; the silence limit applies across them all.
    ///
    /// [`MAX_SEGMENT_SIZE`]: crate::resource::MAX_SEGMENT_SIZE
    pub async fn publish(&mut self, data: &[u8]) -> io::Result<()> {
        let sender = self.sender(data, ResourceKind::Data)?;
        self.publish_sender(sender).await
    }

    /// A sender for `data` of `kind` on this session's link, under its timing.
    pub(super) fn sender<'a>(
        &self,
        data: &'a [u8],
        kind: ResourceKind,
    ) -> io::Result<SegmentedSender<&'a [u8]>> {
        let mut random_hash = [0_u8; RANDOM_HASH_LEN];
        fill_random(&mut random_hash);
        SegmentedSender::new(self.link.clone(), data, None, kind, random_hash, &next_iv())
            .map(|sender| sender.with_timing(self.timing()))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))
    }

    pub(super) async fn publish_sender<D: AsRef<[u8]>>(
        &mut self,
        sender: SegmentedSender<D>,
    ) -> io::Result<()> {
        let mut sender = sender.with_timing(self.timing());
        let mut pace = Pace::new(self.config.timeout);
        self.shared
            .send_on(self.iface, sender.advertise(pace.now(), &next_iv()));
        loop {
            tokio::select! {
                maybe = self.packets.recv() => {
                    let packet = maybe.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                    })?;
                    if self.link.receive(&packet) == Some(Inbound::Close) {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "resource link closed",
                        ));
                    }
                    pace.heard();
                    for outbound in sender.on_packet(&packet, pace.now(), next_iv) {
                        self.shared.send_on(self.iface, outbound);
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
                _ = tokio::time::sleep_until(pace.wake(sender.deadline())) => {
                    let awaiting_proof = sender.awaiting_proof();
                    if pace.idle() {
                        // Tell the receiver to stop rather than leave it requesting into
                        // silence.
                        if let Some(cancel) = sender.cancel(&next_iv()) {
                            self.shared.send_on(self.iface, cancel);
                        }
                    } else if let Some(packet) = sender.poll(pace.now(), next_iv) {
                        self.shared.send_on(self.iface, packet);
                    }
                    if sender.is_canceled() {
                        return Err(publish_timed_out(&sender, awaiting_proof));
                    }
                }
            }
        }
    }

    /// Fetch one payload published by the peer, returning after verification and proof.
    ///
    /// Metadata the publisher attached is kept for [`take_metadata`](Self::take_metadata).
    pub async fn fetch(&mut self) -> io::Result<Vec<u8>> {
        let mut receiver = self.receivers(None, true)();
        let mut pace = Pace::new(self.config.timeout);
        let mut kept = 0;
        loop {
            tokio::select! {
                maybe = self.packets.recv() => {
                    let packet = maybe.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                    })?;
                    if self.link.receive(&packet) == Some(Inbound::Close) {
                        return Err(io::Error::new(
                            io::ErrorKind::BrokenPipe,
                            "resource link closed",
                        ));
                    }
                    pace.heard();
                    if self.on_resource_packet(&mut receiver, &packet, &pace, &mut kept)? {
                        return Ok(self.take_received(&mut receiver));
                    }
                }
                _ = tokio::time::sleep_until(pace.wake(receiver.deadline())) => {
                    self.poll_receiver(&mut receiver, &pace, "resource fetch timed out")?;
                }
            }
        }
    }

    /// Feed `receiver` one packet, keeping each segment proof it sends (`kept` counts
    /// those already kept); true once it has completed.
    pub(super) fn on_resource_packet(
        &mut self,
        receiver: &mut SegmentedReceiver,
        packet: &Packet,
        pace: &Pace,
        kept: &mut usize,
    ) -> io::Result<bool> {
        for outbound in receiver.on_packet(packet, pace.now(), next_iv) {
            self.shared.send_on(self.iface, outbound);
        }
        keep_resource_proofs(&self.shared, self.iface, self.link.id(), receiver, kept);
        if receiver.is_complete() {
            self.keep_carry(receiver);
            return Ok(true);
        }
        if receiver.failure().is_some() {
            self.keep_carry(receiver);
        }
        resource_receive_ended(receiver)?;
        Ok(false)
    }

    /// Leave the link where `receiver`'s request window ended, for the next transfer.
    pub(super) fn keep_carry(&mut self, receiver: &SegmentedReceiver) {
        if let Some(carry) = receiver.carry() {
            self.link.set_resource_carry(carry);
        }
    }

    /// Run `receiver`'s watchdog; past the silence limit, cancel and fail with `timed_out`.
    pub(super) fn poll_receiver(
        &mut self,
        receiver: &mut SegmentedReceiver,
        pace: &Pace,
        timed_out: &'static str,
    ) -> io::Result<()> {
        if pace.idle() {
            if let Some(cancel) = receiver.cancel(&next_iv()) {
                self.shared.send_on(self.iface, cancel);
            }
            return Err(io::Error::new(io::ErrorKind::TimedOut, timed_out));
        }
        for outbound in receiver.poll(pace.now(), next_iv) {
            self.shared.send_on(self.iface, outbound);
        }
        if receiver.failure().is_some() {
            self.keep_carry(receiver);
        }
        resource_receive_ended(receiver)
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
        let mut pace = Pace::new(self.config.timeout);
        let mut kept = 0;
        loop {
            tokio::select! {
                maybe = self.packets.recv() => {
                    let packet = maybe.ok_or_else(|| {
                        io::Error::new(io::ErrorKind::BrokenPipe, "resource link closed")
                    })?;
                    pace.heard();
                    // The sender's IDENTIFY, signed under the link: what authenticates a
                    // first message from a peer we have never heard announce. Only a
                    // responder takes one, and the first one stands (`Link.py` 973-990).
                    if self.responder && let Some(identity) = self.link.read_identify(&packet) {
                        let identity = *self.identified_peer.get_or_insert(identity);
                        self.retain_identified_peer(identity);
                        continue;
                    }
                    match self.link.receive(&packet) {
                        Some(Inbound::Data(data)) => return Ok(ReceivedPayload::Data(data)),
                        Some(Inbound::Close) => {
                            return Err(io::Error::new(
                                io::ErrorKind::BrokenPipe,
                                "resource link closed",
                            ));
                        }
                        _ => {}
                    }
                    if self.on_resource_packet(&mut receiver, &packet, &pace, &mut kept)? {
                        return Ok(ReceivedPayload::Resource(self.take_received(&mut receiver)));
                    }
                }
                _ = tokio::time::sleep_until(pace.wake(receiver.deadline())) => {
                    self.poll_receiver(&mut receiver, &pace, "payload receive timed out")?;
                }
            }
        }
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
        crate::Error::ResourceTimedOut => io::ErrorKind::TimedOut,
        _ => io::ErrorKind::InvalidData,
    };
    io::Error::new(kind, error)
}

/// The error for a publish that gave up: on a silent receiver, or a proof that never came.
fn publish_timed_out<D: AsRef<[u8]>>(
    sender: &SegmentedSender<D>,
    awaiting_proof: bool,
) -> io::Error {
    let message = if awaiting_proof {
        "resource proof never arrived".to_string()
    } else if sender.served_parts() > 0 {
        format!(
            "resource publish timed out after serving {} requested part(s)",
            sender.served_parts()
        )
    } else if sender.has_started() {
        "resource publish timed out after receiver request matched no parts".to_string()
    } else {
        "resource publish timed out before receiver request".to_string()
    };
    io::Error::new(io::ErrorKind::TimedOut, message)
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
        responder: direction == LinkDirection::Inbound,
        accept: None,
        metadata: None,
        max_resource_size: DEFAULT_MAX_RESOURCE_SIZE,
        max_request_size: None,
    })
}
