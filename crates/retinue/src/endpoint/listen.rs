//! TCP listeners and the interfaces they spawn per connection (`TCPInterface.py` 511-688).

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};

use crate::ifac::Ifac;
use crate::node::InterfaceMode;

use super::iface_policy::IfacePolicy;
use super::interface::InterfaceId;
use super::pump::carry_tcp;
use super::runtime::{Endpoint, endpoint_closed, track};
use super::shared::Shared;
use super::sockopt;

/// Back-off after a failed `accept`, so descriptor exhaustion does not spin.
const ACCEPT_RETRY: Duration = Duration::from_millis(100);

/// What every connection a listener accepts inherits, as RNS copies a server interface's
/// settings onto each spawned client (`TCPInterface.py` 587-650).
///
/// Not yet modelled: the `IN` flag, egress control and path-request rates (`ec_pr_freq`,
/// `ic_pr_burst_*`), and `recursive_prs`.
#[derive(Clone, Debug, Default)]
pub struct ListenPolicy {
    /// Mode of each spawned interface.
    pub mode: InterfaceMode,
    /// Access code each spawned interface requires.
    pub ifac: Option<Ifac>,
    /// Use RNS's slower keepalive profile for connections tunnelled through I2P.
    pub i2p_tunneled: bool,
    /// Announce and transmit policy of each spawned interface.
    pub iface: IfacePolicy,
    /// Told each spawned interface's id, as RNS lists `spawned_interfaces`. Advisory: an id
    /// that finds the channel full or closed is dropped, so a slow consumer never stalls
    /// accepting.
    pub spawned: Option<mpsc::Sender<InterfaceId>>,
}

/// A running listener. Dropping the handle leaves it listening; [`Self::close`] stops it.
#[derive(Debug)]
pub struct Listener {
    local: SocketAddr,
    stop: Arc<Notify>,
}

impl Listener {
    /// The bound address.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// Stop accepting and release the port. Connections already accepted keep their
    /// interfaces, as RNS's server detach leaves its spawned clients (`TCPInterface.py` 675-688).
    pub fn close(&self) {
        self.stop.notify_one();
    }
}

impl Endpoint {
    /// Listen on TCP; every accepted connection becomes an interface. Returns the bound
    /// address (pass port 0 to get an OS-assigned one).
    pub async fn listen_tcp(&self, addr: SocketAddr) -> io::Result<SocketAddr> {
        let listener = self.listen_tcp_with(addr, ListenPolicy::default()).await?;
        Ok(listener.local_addr())
    }

    /// Listen for IFAC-authenticated TCP connections. Stock TCP and Local peers without
    /// `ifac_size` use [`Ifac::for_stream`]'s 16-byte codes.
    pub async fn listen_tcp_with_ifac(
        &self,
        addr: SocketAddr,
        ifac: Ifac,
    ) -> io::Result<SocketAddr> {
        let policy = ListenPolicy {
            ifac: Some(ifac),
            ..ListenPolicy::default()
        };
        Ok(self.listen_tcp_with(addr, policy).await?.local_addr())
    }

    /// Listen on TCP, giving every accepted connection `policy`.
    ///
    /// A failed `accept` (descriptor exhaustion, an aborted handshake) is retried after
    /// 100 ms rather than ending the listener, as RNS's server keeps serving.
    pub async fn listen_tcp_with(
        &self,
        addr: SocketAddr,
        policy: ListenPolicy,
    ) -> io::Result<Listener> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let listener = TcpListener::bind(addr).await?;
        let local = listener.local_addr()?;
        let stop = Arc::new(Notify::new());
        let (shared, stopped) = (Arc::clone(&self.shared), Arc::clone(&stop));
        let started = track(&self.shared, async move {
            loop {
                let accepted = tokio::select! {
                    () = stopped.notified() => return,
                    accepted = listener.accept() => accepted,
                };
                let Ok((stream, _)) = accepted else {
                    tokio::time::sleep(ACCEPT_RETRY).await;
                    continue;
                };
                if !shared.is_running() {
                    return;
                }
                let (id, attached) = spawn(&shared, stream, &policy);
                if let (true, Some(spawned)) = (attached, &policy.spawned) {
                    let _ = spawned.try_send(id);
                }
            }
        });
        if !started {
            return Err(endpoint_closed());
        }
        Ok(Listener { local, stop })
    }

    /// Attach a connected TCP stream as an interface, and return its id.
    pub fn attach_stream(&self, stream: TcpStream) -> InterfaceId {
        spawn(&self.shared, stream, &ListenPolicy::default()).0
    }

    /// Attach an IFAC-authenticated connected TCP stream; see [`Ifac::for_stream`].
    pub fn attach_stream_with_ifac(&self, stream: TcpStream, ifac: Ifac) -> InterfaceId {
        let policy = ListenPolicy {
            ifac: Some(ifac),
            ..ListenPolicy::default()
        };
        spawn(&self.shared, stream, &policy).0
    }
}

/// Attach one accepted connection under `policy`, carried until it ends and then forgotten.
/// The flag is false if the endpoint has stopped.
fn spawn(shared: &Arc<Shared>, stream: TcpStream, policy: &ListenPolicy) -> (InterfaceId, bool) {
    let _ = sockopt::tune(&stream, policy.i2p_tunneled);
    let attached = shared.add_interface(crate::packet::MTU, policy.ifac.clone(), policy.mode, true);
    let id = attached.id;
    if !attached.registered {
        return (id, false);
    }
    if policy.iface != IfacePolicy::default() {
        shared
            .iface_policies
            .lock()
            .unwrap()
            .insert(id, policy.iface);
    }
    let (online, mut outbound) = (attached.online, attached.outbound);
    let owner = Arc::clone(shared);
    let started = track(shared, async move {
        carry_tcp(&owner, id, &online, &mut outbound, stream).await;
        owner.forget_interface(id);
    });
    if !started {
        shared.forget_interface(id);
    }
    (id, started)
}
