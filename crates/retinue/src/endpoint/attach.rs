//! Attaching and detaching interfaces. The TCP carriers are in `dial` and `listen`.

use alloc::vec::Vec;

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use crate::ifac::Ifac;
use crate::node::InterfaceMode;

use super::interface::{Iface, Interface, InterfaceId};
use super::queue::{OutboundPackets, OutboundQueues, QueueDepths, QueueWeights};
use super::runtime::Endpoint;
use super::shared::{Lifecycle, Shared};

/// A carrier's handles on the interface it registered.
pub(super) struct Attached {
    pub(super) id: InterfaceId,
    pub(super) outbound: OutboundPackets,
    pub(super) frame_limit: Arc<AtomicUsize>,
    pub(super) online: Arc<AtomicBool>,
    /// False if the endpoint had stopped: the queues are then closed and nothing is recorded.
    pub(super) registered: bool,
}

impl Shared {
    fn register_interface(&self, iface: Iface) -> bool {
        let state = self.lifecycle.lock().unwrap();
        if *state != Lifecycle::Running {
            return false;
        }
        let id = iface.id;
        self.write_diagnostic(|| {
            self.interfaces.lock().unwrap().push(iface);
            let evicted = self
                .announce_admission
                .lock()
                .unwrap()
                .attach_interface(id, self.announce_admission_now_ms());
            if let Some(evicted) = evicted {
                self.held_announces.lock().unwrap().purge(evicted);
            }
            (true, true)
        })
    }

    /// Register a new interface whose frame limit is `max_frame_len`, capped at the protocol
    /// MTU plus the access code.
    pub(super) fn add_interface(
        &self,
        max_frame_len: usize,
        ifac: Option<Ifac>,
        mode: InterfaceMode,
        online: bool,
    ) -> Attached {
        let id = self.next_iface_id.fetch_add(1, Ordering::Relaxed);
        let wire_overhead = ifac.as_ref().map_or(0, Ifac::size);
        let queues = Arc::new(OutboundQueues::new(
            self.queue_weights(),
            self.queue_depths(),
        ));
        let frame_limit = Arc::new(AtomicUsize::new(
            max_frame_len.min(crate::packet::MTU + wire_overhead),
        ));
        let online = Arc::new(AtomicBool::new(online));
        let registered = self.register_interface(Iface {
            id,
            outbound: Arc::clone(&queues),
            frame_limit: Arc::clone(&frame_limit),
            wire_overhead,
            mode,
            online: Arc::clone(&online),
            unsendable: AtomicU64::new(0),
        });
        if !registered {
            queues.close();
        }
        Attached {
            id,
            outbound: OutboundPackets {
                queues,
                delivery_in_flight: false,
                ifac,
            },
            frame_limit,
            online,
            registered,
        }
    }

    /// Read an attached interface's record.
    pub(super) fn with_iface<T>(
        &self,
        id: InterfaceId,
        read: impl FnOnce(&Iface) -> T,
    ) -> Option<T> {
        self.interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|iface| iface.id == id)
            .map(read)
    }

    /// Forget an interface, closing its outbound queues: a holder of the matching
    /// [`Interface`] sees them closed and stops, which is how a carrier ends.
    pub(super) fn forget_interface(&self, id: InterfaceId) {
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
            self.iface_policies.lock().unwrap().remove(&id);
            // RNS culls routes and bridges with their interface (`Transport.py` 880-881,
            // 975-978); a destination without a route is then reached by broadcast.
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
            self.held_announces.lock().unwrap().purge(id);
            self.held_release_wake.notify_waiters();
            ((), removed || routes_removed)
        });
    }

    /// The mode of an attached interface; [`InterfaceMode::Full`] if it is not attached.
    pub(super) fn interface_mode(&self, id: InterfaceId) -> InterfaceMode {
        self.with_iface(id, |iface| iface.mode)
            .unwrap_or(InterfaceMode::Full)
    }

    /// The queue weights currently configured (from the routing policy).
    fn queue_weights(&self) -> QueueWeights {
        self.routing.lock().unwrap().queue_weights
    }

    /// The queue depths currently configured (from the routing policy).
    fn queue_depths(&self) -> QueueDepths {
        self.routing.lock().unwrap().queue_depths
    }

    /// The largest packet an attached interface carries: its frame limit less its access code.
    pub(super) fn link_mtu_on(&self, iface: InterfaceId) -> Option<u32> {
        self.with_iface(iface, |i| {
            let limit = i.frame_limit.load(Ordering::Acquire);
            u32::try_from(limit.saturating_sub(i.wire_overhead)).unwrap_or(u32::MAX)
        })
    }
}

impl Endpoint {
    /// Attach a raw packet [`Interface`] and return its handle, doing no I/O or framing. The
    /// caller drains [`Interface::next_outbound`] and delivers received packets through its
    /// [`InterfaceSink`](super::InterfaceSink); `attach_tcp` and `listen_tcp` are this plus
    /// framing.
    pub fn attach_interface(&self) -> Interface {
        self.attach_interface_with_frame_limit(crate::packet::MTU)
            .expect("the Reticulum protocol MTU is a valid interface frame limit")
    }

    /// Detach an interface, closing its queues and forgetting its record, so a carrier that
    /// reconnects does not leave its old record behind.
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

    /// Attach a raw packet interface with an explicit complete-frame limit.
    ///
    /// The effective limit cannot exceed Reticulum's own protocol MTU. Interface
    /// drivers may lower it again if they discover a stricter carrier limit.
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
        let attached = self
            .shared
            .add_interface(max_frame_len, ifac, InterfaceMode::Full, true);
        Ok(Interface {
            id: attached.id,
            router_tx: self.shared.router_tx.clone(),
            frame_limit: attached.frame_limit,
            ifac: attached.outbound.ifac.clone(),
            outbound: attached.outbound,
            dropped: Arc::new(AtomicU64::new(0)),
        })
    }

    /// Whether an attached interface's carrier is up; false if it is not attached. A dialed
    /// TCP interface stays attached, with its routes, while it reconnects
    /// (`TCPInterface.py` 127-128, 415-446).
    pub fn interface_online(&self, id: InterfaceId) -> bool {
        self.shared
            .with_iface(id, |iface| iface.online.load(Ordering::Acquire))
            .unwrap_or(false)
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
}
