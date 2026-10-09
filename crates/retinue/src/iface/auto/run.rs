//! The AutoInterface carrier: one task per adopted interface, one pump per peer.

use alloc::string::String;

use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::{Arc, Weak};

use tokio::net::UdpSocket;
use tokio::sync::watch;

use super::attach::LinkLocals;
use super::peers::{Data, Token};
use super::sockets::{Sockets, scoped};
use super::state::{Peer, State, apply_policy, pump};
use super::{ANNOUNCE_INTERVAL, AutoConfig, HW_MTU, PEER_JOB_INTERVAL, keep_link_local};
use crate::auto::{descope, peering_token, token_valid};
use crate::endpoint::Endpoint;
use crate::ifac::Ifac;
use crate::iface::udp::{RX_BUF, RX_ERROR_PAUSE, frame_fits};

/// Stock reads discovery datagrams into 1024 bytes (`AutoInterface.py` 367).
const TOKEN_BUF: usize = 1024;

/// One adopted interface's discovery and data task.
pub(super) struct Carrier {
    pub ep: Weak<Endpoint>,
    pub state: Arc<State>,
    pub cfg: Arc<AutoConfig>,
    pub group: Ipv6Addr,
    pub name: String,
    pub index: u32,
    pub link_local: Ipv6Addr,
    pub sockets: Sockets,
    /// The current data socket, which every peer pump of this interface sends from.
    pub data: watch::Sender<Arc<UdpSocket>>,
    pub follow: Option<LinkLocals>,
}

impl Carrier {
    pub async fn run(mut self, mut stop: watch::Receiver<bool>) {
        let mut announce = tokio::time::interval(ANNOUNCE_INTERVAL);
        let start = tokio::time::Instant::now() + PEER_JOB_INTERVAL;
        let mut job = tokio::time::interval_at(start, PEER_JOB_INTERVAL);
        let (mut mbuf, mut ubuf) = ([0u8; TOKEN_BUF], [0u8; TOKEN_BUF]);
        let mut dbuf = [0u8; RX_BUF];
        loop {
            tokio::select! {
                _ = stop.changed() => break,
                _ = announce.tick(), if self.cfg.multicast_tx => {
                    let to = scoped(self.group, self.cfg.discovery_port, self.index);
                    self.send_token(to).await;
                }
                _ = job.tick() => if !self.job().await { break },
                got = self.sockets.multicast.recv_from(&mut mbuf) => self.on_token(got, &mbuf).await,
                got = self.sockets.unicast.recv_from(&mut ubuf) => self.on_token(got, &ubuf).await,
                got = self.sockets.data.recv_from(&mut dbuf) => self.on_data(got, &dbuf).await,
            }
        }
        let gone = self.state.table.lock().unwrap().drain(self.index);
        if let Some(ep) = self.ep.upgrade() {
            gone.iter().for_each(|p| ep.detach_interface(p.value.id));
        }
    }

    async fn send_token(&self, to: SocketAddrV6) {
        let token = peering_token(&self.cfg.group_id, &self.link_local);
        let _ = self.sockets.data.send_to(&token, to).await;
    }

    async fn on_token(&mut self, got: io::Result<(usize, SocketAddr)>, buf: &[u8]) {
        let (n, src) = match got {
            Ok((n, SocketAddr::V6(src))) => (n, descope(*src.ip())),
            Ok(_) => return,
            Err(_) => return tokio::time::sleep(RX_ERROR_PAUSE).await,
        };
        if !token_valid(&self.cfg.group_id, &src, &buf[..n]) {
            return;
        }
        let token = self
            .state
            .table
            .lock()
            .unwrap()
            .on_token(src, self.index, self.state.now());
        match token {
            Token::New => self.add_peer(src).await,
            Token::Full => State::count(&self.state.counters.peers_refused),
            Token::Echo | Token::Refresh => {}
        }
    }

    /// Attach the peer's interface, inheriting mode and policy, and answer at once with a
    /// unicast token. Stock waits for its next job (up to about 9 s); peering is the same.
    async fn add_peer(&mut self, addr: Ipv6Addr) {
        let Some(ep) = self.ep.upgrade() else { return };
        let attached = match &self.cfg.ifac {
            Some(ifac) => ep.attach_interface_with_ifac(HW_MTU + ifac.size(), ifac.clone()),
            None => ep.attach_interface_with_frame_limit(HW_MTU),
        };
        let Ok(interface) = attached else { return };
        let id = interface.id();
        ep.set_interface_mode(id, self.cfg.mode);
        apply_policy(&ep, id, &self.cfg.policy);
        let (out, sink) = interface.split();
        let to = scoped(addr, self.cfg.data_port, self.index);
        let state = Arc::clone(&self.state);
        if ep
            .spawn_carrier(pump(out, self.data.subscribe(), to, state))
            .is_err()
        {
            return ep.detach_interface(id);
        }
        let now = self.state.now();
        let peer = Peer { id, sink };
        self.state
            .table
            .lock()
            .unwrap()
            .insert(addr, self.index, peer, now);
        let reverse = scoped(addr, self.cfg.discovery_port + 1, self.index);
        self.send_token(reverse).await;
    }

    async fn on_data(&mut self, got: io::Result<(usize, SocketAddr)>, buf: &[u8]) {
        let (n, src) = match got {
            Ok((n, SocketAddr::V6(src))) => (n, descope(*src.ip())),
            Ok(_) => return,
            Err(_) => return tokio::time::sleep(RX_ERROR_PAUSE).await,
        };
        let counters = &self.state.counters;
        if !frame_fits(n, HW_MTU, self.cfg.ifac.as_ref().map_or(0, Ifac::size)) {
            return State::count(&counters.oversize);
        }
        let frame = &buf[..n];
        let now = self.state.now();
        let sink = match self
            .state
            .table
            .lock()
            .unwrap()
            .on_data(src, self.index, frame, now)
        {
            Data::Admit(peer) => peer.sink.clone(),
            Data::Duplicate => return State::count(&counters.mif_duplicates),
            Data::Unknown => return,
        };
        State::count(&counters.rx);
        let _ = sink.deliver_frame(frame);
    }

    /// The 4 s job: follow the link-local, expire peers, send reverse tokens. False once
    /// the endpoint is gone.
    async fn job(&mut self) -> bool {
        let Some(ep) = self.ep.upgrade() else {
            return false;
        };
        if let Some(link_locals) = self.follow {
            self.follow_link_local(link_locals);
        }
        let tick = self
            .state
            .table
            .lock()
            .unwrap()
            .tick(self.index, self.state.now());
        tick.expired
            .iter()
            .for_each(|p| ep.detach_interface(p.value.id));
        for addr in tick.reverse {
            let to = scoped(addr, self.cfg.discovery_port + 1, self.index);
            self.send_token(to).await;
        }
        true
    }

    /// On a new link-local, rebind all three sockets and keep the peers. Stock rebinds only
    /// the data listener and never closes the old one (`AutoInterface.py` 433-457).
    fn follow_link_local(&mut self, link_locals: LinkLocals) {
        let Some(now_all) = link_locals(&self.name) else {
            return;
        };
        let Some(next) = keep_link_local(self.link_local, &now_all) else {
            return;
        };
        if next != self.link_local {
            let (group, port, data_port) =
                (self.group, self.cfg.discovery_port, self.cfg.data_port);
            // On failure the old sockets stay, and the next job tries again.
            let Ok(sockets) = Sockets::open(group, next, self.index, port, data_port) else {
                return;
            };
            self.sockets = sockets;
            self.link_local = next;
            self.data.send_replace(Arc::clone(&self.sockets.data));
        }
        let now = self.state.now();
        let mut table = self.state.table.lock().unwrap();
        table.adopt(self.index, self.link_local, &now_all, now);
    }
}
