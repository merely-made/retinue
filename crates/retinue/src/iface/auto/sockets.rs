//! The three sockets of one adopted interface (`AutoInterface.py` 253-310, 331-347).
//!
//! Stock sends tokens from unbound sockets and lets the kernel pick the source. Here the
//! data socket sends everything, tokens included, so the source is always the adopted
//! link-local that peers hash and key on.

use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

/// A socket address on interface `index`, scoped when the address needs a zone: a
/// link-local unicast or a link-scope multicast.
pub fn scoped(addr: Ipv6Addr, port: u16, index: u32) -> SocketAddrV6 {
    let s = addr.segments()[0];
    let link = s & 0xffc0 == 0xfe80 || (s & 0xff00 == 0xff00 && s & 0x000f == 0x2);
    SocketAddrV6::new(addr, port, 0, if link { index } else { 0 })
}

fn udp6(reuse: bool) -> io::Result<Socket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    if reuse {
        socket.set_reuse_address(true)?;
        socket.set_reuse_port(true)?;
    }
    socket.set_nonblocking(true)?;
    Ok(socket)
}

fn bound(socket: Socket, at: SocketAddrV6) -> io::Result<UdpSocket> {
    socket.bind(&SocketAddr::V6(at).into())?;
    UdpSocket::from_std(socket.into())
}

/// The discovery group listener: joined on `index` and bound to the group, zoned for link
/// scope. Shared (SO_REUSEADDR and SO_REUSEPORT) with any other instance on the host.
pub fn multicast(group: Ipv6Addr, index: u32, port: u16) -> io::Result<UdpSocket> {
    let socket = udp6(true)?;
    socket.set_multicast_if_v6(index)?;
    socket.join_multicast_v6(&group, index)?;
    bound(socket, scoped(group, port, index))
}

/// The reverse-peering listener on the adopted link-local, shared like [`multicast`].
pub fn unicast_discovery(link_local: Ipv6Addr, index: u32, port: u16) -> io::Result<UdpSocket> {
    bound(udp6(true)?, scoped(link_local, port, index))
}

/// The data socket on the adopted link-local, also the sender of every token. No reuse, so
/// it never takes the port from another instance (`AutoInterface.py` 342).
pub fn data(link_local: Ipv6Addr, index: u32, port: u16) -> io::Result<UdpSocket> {
    let socket = udp6(false)?;
    socket.set_multicast_if_v6(index)?;
    bound(socket, scoped(link_local, port, index))
}

/// The three sockets of one adopted interface.
#[derive(Debug)]
pub(super) struct Sockets {
    pub multicast: UdpSocket,
    pub unicast: UdpSocket,
    pub data: std::sync::Arc<UdpSocket>,
}

impl Sockets {
    /// Bind all three. A data port already in use gets an error that says why.
    pub fn open(
        group: Ipv6Addr,
        link_local: Ipv6Addr,
        index: u32,
        discovery_port: u16,
        data_port: u16,
    ) -> io::Result<Self> {
        let data = data(link_local, index, data_port).map_err(|e| match e.kind() {
            io::ErrorKind::AddrInUse => io::Error::new(
                e.kind(),
                alloc::format!(
                    "AutoInterface data port {data_port} on [{link_local}] is taken, most likely \
                     by another Reticulum instance (rnsd) with an AutoInterface; two instances \
                     on one host share a link-local and can never peer"
                ),
            ),
            _ => e,
        })?;
        Ok(Self {
            multicast: multicast(group, index, discovery_port)?,
            unicast: unicast_discovery(link_local, index, discovery_port + 1)?,
            data: std::sync::Arc::new(data),
        })
    }
}
