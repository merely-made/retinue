//! The raw packet interface seam, and the endpoint's record of an attached interface.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use tokio::sync::mpsc;

use crate::ifac::Ifac;
use crate::node::InterfaceMode;
use crate::packet::Packet;

use super::queue::{OutboundPackets, OutboundQueues, TrafficClass};

/// Identifies one attached interface. A dialed TCP interface keeps its id across reconnects.
pub type InterfaceId = u32;

/// A raw packet interface: the seam every transport plugs into.
///
/// Drain outbound [`Packet`]s with [`next_outbound`] and deliver inbound ones through its
/// [`InterfaceSink`]. Nothing here does I/O or framing: TCP is this seam plus HDLC over a
/// socket, and a serial line or a test loss oracle is the same seam with another pump.
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
    /// [`Endpoint::attach_interface_with_frame_limit`]. Tulle also constrains this value
    /// synchronously when its driver is constructed.
    ///
    /// [`Endpoint::attach_interface_with_frame_limit`]: super::Endpoint::attach_interface_with_frame_limit
    pub fn frame_limit(&self) -> usize {
        self.frame_limit.load(Ordering::Acquire)
    }

    /// Lower this interface's admission limit to a carrier-discovered cap.
    ///
    /// Monotonic, and meant for before the endpoint can queue traffic. Prefer
    /// [`Endpoint::attach_interface_with_frame_limit`] when the limit is already known.
    ///
    /// [`Endpoint::attach_interface_with_frame_limit`]: super::Endpoint::attach_interface_with_frame_limit
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
    /// Returns whether the endpoint is **still there**, not whether the packet was queued: a
    /// burst must not look like a dead endpoint to the caller. A full router queue drops the
    /// packet, since Reticulum's upper layers retransmit, and counts it in [`Self::dropped`].
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
    /// Nonzero means the endpoint is offered more than it can route, which the wire alone
    /// cannot tell from a peer that never transmitted.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Authenticate and decode one complete carrier frame, then deliver it.
    ///
    /// An IFAC-configured interface rejects open, incorrectly keyed, and modified frames
    /// before they reach the router. Without IFAC, a frame carrying the IFAC flag is passed
    /// on, and the router drops it and counts it in
    /// [`RoutingCounters::ifac_flag_rejected`](super::RoutingCounters::ifac_flag_rejected).
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
    /// Whether the carrier is up. A dialed interface outlives its connection, keeping its
    /// routes, and admits nothing while down (`TCPInterface.py` 127-128; `Transport.py` 1449).
    pub(super) online: Arc<AtomicBool>,
    /// Queued packets the carrier could not encode, dropped rather than ending the carrier.
    pub(super) unsendable: AtomicU64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueueAdmission {
    Queued,
    Full,
    FrameLimit { actual: usize, limit: usize },
}

impl Iface {
    pub(super) fn push(&self, packet: Packet, class: TrafficClass) -> QueueAdmission {
        if !self.online.load(Ordering::Acquire) {
            return QueueAdmission::Full;
        }
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
