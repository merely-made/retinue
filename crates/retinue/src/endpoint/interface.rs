//! The raw packet interface seam, and the endpoint's record of an attached interface.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::sync::mpsc;

use crate::ifac::Ifac;
use crate::node::InterfaceMode;
use crate::packet::Packet;

use super::queue::{OutboundPackets, OutboundQueues, TrafficClass};

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
    pub(super) id: InterfaceId,
    pub(super) outbound: OutboundPackets,
    pub(super) router_tx: mpsc::Sender<(InterfaceId, Packet)>,
    pub(super) frame_limit: Arc<AtomicUsize>,
    pub(super) ifac: Option<Ifac>,
    /// Inbound packets dropped because the router's queue was full. Shared with every
    /// [`InterfaceSink`] split off this interface.
    pub(super) dropped: Arc<AtomicU64>,
}

impl Interface {
    /// This interface's id.
    pub fn id(&self) -> InterfaceId {
        self.id
    }

    /// Maximum complete Reticulum packet this interface currently admits.
    ///
    /// Raw interface owners can set an initial cap through
    /// [`Endpoint::attach_interface_with_frame_limit`](super::Endpoint::attach_interface_with_frame_limit). Tulle also constrains
    /// this value synchronously when its driver is constructed.
    pub fn frame_limit(&self) -> usize {
        self.frame_limit.load(Ordering::Acquire)
    }

    /// Lower this interface's admission limit to a carrier-discovered cap.
    ///
    /// This is monotonic and should be called before the endpoint can queue
    /// traffic. Prefer [`Endpoint::attach_interface_with_frame_limit`](super::Endpoint::attach_interface_with_frame_limit) when the
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
    /// drops it and counts it in [`RoutingCounters::ifac_flag_rejected`](super::RoutingCounters::ifac_flag_rejected).
    pub fn deliver_frame(&self, frame: &[u8]) -> crate::Result<bool> {
        let packet = match &self.ifac {
            Some(ifac) => Packet::decode(&ifac.open(frame)?)?,
            None => Packet::decode(frame)?,
        };
        Ok(self.deliver(packet))
    }
}

/// An attached interface: the scheduler its writer task drains.
pub(super) struct Iface {
    pub(super) id: InterfaceId,
    pub(super) outbound: Arc<OutboundQueues>,
    pub(super) frame_limit: Arc<AtomicUsize>,
    pub(super) wire_overhead: usize,
    /// Sets the lifetime of routes learned on this interface.
    pub(super) mode: InterfaceMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueueAdmission {
    Queued,
    Full,
    FrameLimit { actual: usize, limit: usize },
}

impl Iface {
    pub(super) fn push(&self, packet: Packet, class: TrafficClass) -> QueueAdmission {
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
