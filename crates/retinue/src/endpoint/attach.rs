//! Attaching and detaching interfaces, raw and TCP.

use alloc::vec::Vec;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::identity::PrivateIdentity;
use crate::ifac::Ifac;
use crate::iface::hdlc::{Deframer, frame};
use crate::node::InterfaceMode;
use crate::packet::Packet;

use super::interface::{Iface, Interface, InterfaceId};
use super::queue::{OutboundPackets, OutboundQueues, QueueDepths, QueueWeights};
use super::runtime::{Endpoint, endpoint_closed, track};
use super::shared::{Lifecycle, Shared};

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

    /// Forget an interface, closing its outbound queues: a holder of the matching
    /// [`Interface`] sees them closed and stops, which is how a carrier ends.
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
        self.interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|iface| iface.id == id)
            .map_or(InterfaceMode::Full, |iface| iface.mode)
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
}

impl Endpoint {
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

    /// Attach a raw packet [`Interface`] and return its handle, doing no I/O or framing. The
    /// caller drains [`Interface::next_outbound`] and delivers received packets through its
    /// [`InterfaceSink`](super::InterfaceSink); `attach_tcp_client` and `listen_tcp` are this
    /// plus framing.
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
                // Await a full router queue rather than drop: TCP flow control then slows a
                // flooding peer. `send` fails only once the endpoint is shutting down.
                if let Ok(pkt) = Packet::decode(&logical)
                    && router_tx.send((id, pkt)).await.is_err()
                {
                    return;
                }
            }
        }
        // The socket is gone, and with it the interface.
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
