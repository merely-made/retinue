//! The TCP interface: HDLC framing over a TCP stream.
//!
//! This is the R1 shell. It owns no protocol logic: it moves framed bytes between a socket
//! and R0's codec, which stays sans-io underneath.
//!
//! Both directions are the same object. A Reticulum TCP interface is symmetric once the
//! connection exists, so [`TcpInterface::connect`] (client) and
//! [`TcpInterface::from_stream`] (what a server does with an accepted socket) produce the
//! same thing.

use alloc::vec::Vec;

use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use super::hdlc::{Deframer, frame};
use crate::ifac::Ifac;
use crate::packet::Packet;

/// Anything that can go wrong reading a packet off an interface.
#[derive(Debug)]
pub enum RecvError {
    /// The socket failed, or the peer hung up.
    Io(io::Error),
    /// A frame arrived but was not a packet we could decode. The connection is still
    /// usable; this is a bad packet, not a bad connection.
    Wire(crate::Error),
}

impl From<io::Error> for RecvError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

impl core::fmt::Display for RecvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "interface i/o: {e}"),
            Self::Wire(e) => write!(f, "undecodable packet: {e}"),
        }
    }
}

impl core::error::Error for RecvError {}

/// One end of a Reticulum TCP interface.
pub struct TcpInterface {
    stream: TcpStream,
    deframer: Deframer,
    ifac: Option<Ifac>,
    /// Frames that arrived in the same read as an earlier one, not yet handed out.
    pending: VecDeque<Vec<u8>>,
}

impl TcpInterface {
    /// Dial a peer.
    pub async fn connect(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self::from_stream(TcpStream::connect(addr).await?))
    }

    /// Dial a peer through an IFAC-authenticated virtual network.
    pub async fn connect_with_ifac(addr: SocketAddr, ifac: Ifac) -> io::Result<Self> {
        Ok(Self::from_stream_with_ifac(
            TcpStream::connect(addr).await?,
            ifac,
        ))
    }

    /// Wrap an already-connected socket, as a server does after accepting.
    pub fn from_stream(stream: TcpStream) -> Self {
        Self::from_stream_access(stream, None)
    }

    /// Wrap an already-connected socket with IFAC authentication.
    pub fn from_stream_with_ifac(stream: TcpStream, ifac: Ifac) -> Self {
        Self::from_stream_access(stream, Some(ifac))
    }

    fn from_stream_access(stream: TcpStream, ifac: Option<Ifac>) -> Self {
        // Reticulum packets are small and latency matters more than packing.
        let _ = stream.set_nodelay(true);
        Self {
            stream,
            deframer: Deframer::new(),
            ifac,
            pending: VecDeque::new(),
        }
    }

    /// The peer's address.
    pub fn peer_addr(&self) -> io::Result<SocketAddr> {
        self.stream.peer_addr()
    }

    /// Frame a packet and put it on the wire.
    pub async fn send(&mut self, packet: &Packet) -> io::Result<()> {
        let logical = packet.encode();
        let wire = match &self.ifac {
            Some(ifac) => ifac
                .seal(&logical)
                .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?,
            None => logical,
        };
        self.send_raw(&wire).await
    }

    /// Frame raw packet bytes and put them on the wire.
    pub async fn send_raw(&mut self, packet: &[u8]) -> io::Result<()> {
        self.stream.write_all(&frame(packet)).await?;
        self.stream.flush().await
    }

    /// Read until a whole frame arrives, and return its bytes undecoded.
    ///
    /// Returns `UnexpectedEof` when the peer closes.
    pub async fn recv_frame(&mut self) -> io::Result<Vec<u8>> {
        loop {
            if let Some(f) = self.pending.pop_front() {
                return Ok(f);
            }
            let mut buf = [0u8; 4096];
            let n = self.stream.read(&mut buf).await?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "peer closed the connection",
                ));
            }
            self.pending.extend(self.deframer.push(&buf[..n]));
        }
    }

    /// Read until a whole packet arrives, and decode it.
    pub async fn recv(&mut self) -> Result<Packet, RecvError> {
        let raw = self.recv_frame().await?;
        let logical = match &self.ifac {
            Some(ifac) => ifac.open(&raw).map_err(RecvError::Wire)?,
            None => raw,
        };
        Packet::decode(&logical).map_err(RecvError::Wire)
    }
}

/// Accept incoming TCP interface connections.
pub struct TcpInterfaceListener {
    listener: TcpListener,
    ifac: Option<Ifac>,
}

impl TcpInterfaceListener {
    /// Bind and listen. Pass port 0 to let the OS choose, then ask [`local_addr`].
    ///
    /// [`local_addr`]: TcpInterfaceListener::local_addr
    pub async fn bind(addr: SocketAddr) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
            ifac: None,
        })
    }

    /// Bind an IFAC-authenticated TCP interface listener.
    pub async fn bind_with_ifac(addr: SocketAddr, ifac: Ifac) -> io::Result<Self> {
        Ok(Self {
            listener: TcpListener::bind(addr).await?,
            ifac: Some(ifac),
        })
    }

    /// The address actually bound, which is how you learn an OS-assigned port.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    /// Wait for a peer to connect.
    pub async fn accept(&self) -> io::Result<TcpInterface> {
        let (stream, _) = self.listener.accept().await?;
        Ok(match &self.ifac {
            Some(ifac) => TcpInterface::from_stream_with_ifac(stream, ifac.clone()),
            None => TcpInterface::from_stream(stream),
        })
    }
}
