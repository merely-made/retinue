//! TCP client dialing: name resolution, connect timeout, and reconnect (`TCPInterface.py`
//! 97-180, 231-303, 415-446).

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpStream, lookup_host};

use crate::identity::PrivateIdentity;
use crate::ifac::Ifac;
use crate::node::InterfaceMode;

use super::interface::InterfaceId;
use super::pump::{PumpEnd, carry_tcp};
use super::runtime::{Endpoint, endpoint_closed, track};
use super::sockopt;

/// A TCP client interface: RNS's `TCPClientInterface` settings.
#[derive(Clone, Debug)]
pub struct TcpClient {
    /// Host name or address literal, resolved again on every attempt.
    pub host: String,
    /// Port to dial.
    pub port: u16,
    /// Access code, if the hub uses one.
    pub ifac: Option<Ifac>,
    /// Use RNS's slower keepalive profile for a tunnel through I2P.
    pub i2p_tunneled: bool,
    /// Prefer an IPv6 address when the host has both families (`BackboneInterface.py` 965-971).
    pub prefer_ipv6: bool,
    /// Bound on resolving and connecting, per attempt. RNS ignores its own setting and
    /// always uses 5 s (`TCPInterface.py` 241).
    pub connect_timeout: Duration,
    /// Wait before each reconnect attempt (`TCPInterface.py` 80).
    pub reconnect_wait: Duration,
    /// Attempts per outage before the interface is forgotten; `None` retries forever. RNS
    /// parses this but never stops (`TCPInterface.py` 276-299); here it is honoured.
    pub max_reconnect_tries: Option<u32>,
}

impl TcpClient {
    /// RNS's defaults: 5 s connect timeout and reconnect wait, retrying forever.
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
            ifac: None,
            i2p_tunneled: false,
            prefer_ipv6: false,
            connect_timeout: Duration::from_secs(5),
            reconnect_wait: Duration::from_secs(5),
            max_reconnect_tries: None,
        }
    }
}

impl Endpoint {
    /// Create an endpoint and dial one TCP peer as its first interface.
    pub async fn connect(addr: SocketAddr, identity: PrivateIdentity) -> io::Result<Self> {
        let ep = Self::new(identity);
        ep.attach_tcp_client(addr).await?;
        Ok(ep)
    }

    /// Attach a TCP client interface. Its id is stable: on disconnect the interface goes
    /// offline, keeping its routes, and redials every `reconnect_wait`.
    ///
    /// The first attempt runs before this returns, so a reachable hub is online on return;
    /// an unreachable one is retried in the background, as RNS does. Fails only if the
    /// endpoint is closed.
    pub async fn attach_tcp(&self, client: TcpClient) -> io::Result<InterfaceId> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let first = dial(&client).await.ok();
        self.supervise(client, first)
    }

    /// Dial a TCP peer and attach it, reconnecting as [`Self::attach_tcp`] does. Unlike
    /// it, fails if the first dial fails.
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
        let client = TcpClient {
            ifac,
            ..TcpClient::new(addr.ip().to_string(), addr.port())
        };
        let first = dial(&client).await?;
        self.supervise(client, Some(first))
    }

    /// Register the interface once and keep it connected: carry each connection, then wait
    /// and redial. Only giving up or a detach forgets it.
    fn supervise(
        &self,
        client: TcpClient,
        mut stream: Option<TcpStream>,
    ) -> io::Result<InterfaceId> {
        let attached = self.shared.add_interface(
            crate::packet::MTU,
            client.ifac.clone(),
            InterfaceMode::Full,
            stream.is_some(),
        );
        if !attached.registered {
            return Err(endpoint_closed());
        }
        let (id, online, mut outbound) = (attached.id, attached.online, attached.outbound);
        let shared = Arc::clone(&self.shared);
        let started = track(&self.shared, async move {
            let mut tries = 0u32;
            loop {
                if let Some(connected) = stream.take() {
                    tries = 0;
                    if carry_tcp(&shared, id, &online, &mut outbound, connected).await
                        == PumpEnd::Closed
                    {
                        return;
                    }
                }
                // RNS waits before every attempt, the first included (`TCPInterface.py` 276).
                tokio::time::sleep(client.reconnect_wait).await;
                if shared.with_iface(id, |_| ()).is_none() {
                    return;
                }
                tries += 1;
                if client.max_reconnect_tries.is_some_and(|max| tries > max) {
                    shared.forget_interface(id);
                    return;
                }
                stream = dial(&client).await.ok();
            }
        });
        if !started {
            self.shared.forget_interface(id);
            return Err(endpoint_closed());
        }
        Ok(id)
    }
}

/// Resolve and connect once, within the client's timeout. Of several addresses, the first of
/// the preferred family wins, else the first (`BackboneInterface.py` 959-971).
async fn dial(client: &TcpClient) -> io::Result<TcpStream> {
    let attempt = async {
        let addrs: Vec<SocketAddr> = lookup_host((client.host.as_str(), client.port))
            .await?
            .collect();
        let addr = addrs
            .iter()
            .find(|addr| addr.is_ipv6() == client.prefer_ipv6)
            .or(addrs.first())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "host has no address"))?;
        TcpStream::connect(*addr).await
    };
    let stream = tokio::time::timeout(client.connect_timeout, attempt)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??;
    let _ = sockopt::tune(&stream, client.i2p_tunneled);
    Ok(stream)
}
