//! RNS UDPInterface: one raw packet per IPv4 datagram, IFAC-sealed when configured, with no
//! framing, handshake or keepalive (`UDPInterface.py` 40-151).
//!
//! The usual stock config listens on `0.0.0.0` and forwards to a subnet broadcast on the same
//! port, so a host hears its own frames. Announce and link echoes are already dropped by the
//! router; this carrier also drops any frame it sent within the last 2 s, which covers path
//! requests and plain data.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use sha2::{Digest, Sha256};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use crate::auto::HashRing;
use crate::endpoint::{Endpoint, InterfaceId, InterfaceSink, OutboundPackets};
use crate::ifac::Ifac;

/// Stock's hardware MTU for UDP (`UDPInterface.py` 74).
pub const HW_MTU: usize = 1064;
/// Stock's bitrate guess, which sizes the announce cap (`UDPInterface.py` 41, 80).
pub const BITRATE_GUESS: u64 = 10_000_000;
/// Receive buffer: past every frame a peer may legally send.
pub(crate) const RX_BUF: usize = 2048;
/// Pause after a failed receive, so a dead socket cannot spin.
pub(crate) const RX_ERROR_PAUSE: Duration = Duration::from_millis(100);
const ECHO_RING: usize = 32;
const ECHO_WINDOW_MS: u64 = 2_000;

/// Whether a received frame is within stock's bound: the packet left after removing an
/// `ifac`-byte code may be at most `hw_mtu + ifac` (`Transport.py` 1790-1791).
pub(crate) fn frame_fits(len: usize, hw_mtu: usize, ifac: usize) -> bool {
    len.saturating_sub(ifac) <= hw_mtu + ifac
}

pub(crate) fn frame_hash(frame: &[u8]) -> [u8; 32] {
    Sha256::digest(frame).into()
}

/// Where the carrier receives and sends: stock's `listen_ip`/`listen_port` and
/// `forward_ip`/`forward_port`. A missing `listen` makes it send-only, a missing `forward`
/// receive-only (stock needs both; `UDPInterface.py` 90-114).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UdpConfig {
    pub listen: Option<SocketAddrV4>,
    pub forward: Option<SocketAddrV4>,
    /// Set SO_REUSEADDR on the receive socket, so carriers on one host can share a
    /// broadcast port. Stock sets nothing; off by default.
    pub reuse_address: bool,
}

impl UdpConfig {
    /// Stock's `device` option: listen on and forward to the subnet broadcast of `device`'s
    /// first IPv4 address (`UDPInterface.py` 83-87). Binding the broadcast address means the
    /// carrier hears broadcasts only.
    #[cfg(all(feature = "auto", unix))]
    pub fn device(device: &str, listen_port: u16, forward_port: u16) -> io::Result<Self> {
        let netif = super::netinfo::find(device)?
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such device"))?;
        let broadcast = netif.broadcast_v4().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                "device has no IPv4 address",
            )
        })?;
        Ok(Self {
            listen: Some(SocketAddrV4::new(broadcast, listen_port)),
            forward: Some(SocketAddrV4::new(broadcast, forward_port)),
            reuse_address: false,
        })
    }
}

/// Frame counters of one UDP carrier.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UdpCounters {
    pub rx: u64,
    pub tx: u64,
    /// Frames this carrier itself sent within the last 2 s, heard back and dropped.
    pub own_echo: u64,
    /// Frames past stock's size bound, dropped.
    pub oversize: u64,
}

#[derive(Debug, Default)]
struct Counters {
    rx: AtomicU64,
    tx: AtomicU64,
    own_echo: AtomicU64,
    oversize: AtomicU64,
}

/// A running UDP carrier. It stops when the endpoint closes or detaches its interface.
#[derive(Debug)]
pub struct UdpHandle {
    pub id: InterfaceId,
    /// The bound receive address, if the carrier listens.
    pub local: Option<SocketAddr>,
    counters: Arc<Counters>,
}

impl UdpHandle {
    pub fn counters(&self) -> UdpCounters {
        let c = &self.counters;
        UdpCounters {
            rx: c.rx.load(Ordering::Relaxed),
            tx: c.tx.load(Ordering::Relaxed),
            own_echo: c.own_echo.load(Ordering::Relaxed),
            oversize: c.oversize.load(Ordering::Relaxed),
        }
    }
}

impl Endpoint {
    /// Attach an RNS UDPInterface. Stock peers that set `network_name` or `passphrase`
    /// without `ifac_size` use [`Ifac::for_stream`]'s 16-byte codes.
    ///
    /// Each frame is sent from one ephemeral, broadcast-enabled socket, as stock sends from a
    /// fresh one (`UDPInterface.py` 130-135). The interface takes stock's 10 Mbit/s bitrate
    /// guess. Its frame limit is the protocol MTU plus the code: [`HW_MTU`] bounds only what
    /// is received.
    pub async fn attach_udp(&self, cfg: UdpConfig, ifac: Option<Ifac>) -> io::Result<UdpHandle> {
        let rx = cfg
            .listen
            .map(|addr| bind_rx(addr, cfg.reuse_address))
            .transpose()?;
        let tx = match cfg.forward {
            Some(to) => {
                let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).await?;
                socket.set_broadcast(true)?;
                Some((socket, to))
            }
            None => None,
        };
        let local = rx.as_ref().map(UdpSocket::local_addr).transpose()?;
        let code = ifac.as_ref().map_or(0, Ifac::size);
        let interface = match ifac {
            Some(ifac) => self.attach_interface_with_ifac(HW_MTU + code, ifac)?,
            None => self.attach_interface_with_frame_limit(HW_MTU)?,
        };
        let id = interface.id();
        self.set_interface_bitrate(id, Some(BITRATE_GUESS));
        let counters = Arc::new(Counters::default());
        let (outbound, sink) = interface.split();
        tokio::spawn(run(outbound, sink, rx, tx, code, Arc::clone(&counters)));
        Ok(UdpHandle {
            id,
            local,
            counters,
        })
    }
}

fn bind_rx(addr: SocketAddrV4, reuse: bool) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(reuse)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::V4(addr).into())?;
    UdpSocket::from_std(socket.into())
}

async fn recv(socket: Option<&UdpSocket>, buf: &mut [u8]) -> io::Result<usize> {
    match socket {
        Some(socket) => socket.recv_from(buf).await.map(|(n, _)| n),
        None => core::future::pending().await,
    }
}

async fn run(
    mut out: OutboundPackets,
    sink: InterfaceSink,
    rx: Option<UdpSocket>,
    tx: Option<(UdpSocket, SocketAddrV4)>,
    code: usize,
    counters: Arc<Counters>,
) {
    let epoch = Instant::now();
    let now_ms = || u64::try_from(epoch.elapsed().as_millis()).unwrap_or(u64::MAX);
    let mut sent = HashRing::<ECHO_RING>::default();
    let mut buf = [0u8; RX_BUF];
    loop {
        tokio::select! {
            packet = out.recv() => {
                let Some(packet) = packet else { return };
                // Receive-only drops what it is offered; a packet IFAC cannot seal is skipped.
                let (Some((socket, to)), Ok(wire)) = (&tx, out.encode(&packet)) else { continue };
                sent.insert(frame_hash(&wire), now_ms());
                if socket.send_to(&wire, *to).await.is_ok() {
                    counters.tx.fetch_add(1, Ordering::Relaxed);
                }
            }
            got = recv(rx.as_ref(), &mut buf) => {
                let Ok(n) = got else {
                    tokio::time::sleep(RX_ERROR_PAUSE).await;
                    continue;
                };
                let frame = &buf[..n];
                if n == 0 {
                    continue;
                }
                if !frame_fits(n, HW_MTU, code) {
                    counters.oversize.fetch_add(1, Ordering::Relaxed);
                } else if sent.fresh(&frame_hash(frame), now_ms(), ECHO_WINDOW_MS) {
                    counters.own_echo.fetch_add(1, Ordering::Relaxed);
                } else {
                    counters.rx.fetch_add(1, Ordering::Relaxed);
                    if let Ok(false) = sink.deliver_frame(frame) {
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "udp_tests.rs"]
mod tests;
