//! The AutoInterface runtime: one task per adopted interface, one pump per peer.

use alloc::string::String;
use alloc::vec::Vec;

use std::io;
use std::net::{Ipv6Addr, SocketAddr, SocketAddrV6};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Instant;

use tokio::net::UdpSocket;
use tokio::sync::watch;

use super::handle::AutoHandle;
use super::peers::{Data, Table, Token};
use super::sockets::{Sockets, scoped};
use super::{
    ANNOUNCE_INTERVAL, Adopted, AutoConfig, BITRATE_GUESS, HW_MTU, PEER_JOB_INTERVAL,
    keep_link_local, select,
};
use crate::auto::{descope, discovery_group, peering_token, token_valid};
use crate::endpoint::{Endpoint, IfacePolicy, InterfaceId, InterfaceSink, OutboundPackets};
use crate::ifac::Ifac;
use crate::iface::netinfo;
use crate::iface::udp::{RX_BUF, RX_ERROR_PAUSE, frame_fits};

/// Stock reads discovery datagrams into 1024 bytes (`AutoInterface.py` 367).
const TOKEN_BUF: usize = 1024;

pub(super) struct Peer {
    pub id: InterfaceId,
    sink: InterfaceSink,
}

#[derive(Default)]
pub(super) struct Counters {
    pub rx: AtomicU64,
    pub tx: AtomicU64,
    pub mif_duplicates: AtomicU64,
    pub oversize: AtomicU64,
}

pub(super) struct State {
    epoch: Instant,
    pub table: Mutex<Table<Peer>>,
    pub names: Vec<(u32, String)>,
    pub counters: Counters,
}

impl State {
    pub fn now(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn count(counter: &AtomicU64) {
        counter.fetch_add(1, Ordering::Relaxed);
    }
}

impl Endpoint {
    /// Attach an RNS AutoInterface on every host interface [`select`] adopts.
    ///
    /// Fails if none can be adopted, or if the data port is taken on one: another
    /// instance on this host shares its link-locals, so the two could never peer.
    pub async fn attach_auto(self: &Arc<Self>, cfg: AutoConfig) -> io::Result<AutoHandle> {
        let adopted = select(&netinfo::interfaces()?, &cfg);
        attach_adopted(self, cfg, adopted, true)
    }
}

/// Start discovery on `adopted`. With `follow`, each job re-reads the host's link-locals.
pub(super) fn attach_adopted(
    ep: &Arc<Endpoint>,
    cfg: AutoConfig,
    adopted: Vec<Adopted>,
    follow: bool,
) -> io::Result<AutoHandle> {
    let group = discovery_group(&cfg.group_id, cfg.scope, cfg.addr_type);
    let mut opened = Vec::new();
    for a in adopted {
        match Sockets::open(
            group,
            a.link_local,
            a.index,
            cfg.discovery_port,
            cfg.data_port,
        ) {
            Ok(sockets) => opened.push((a, sockets)),
            Err(e) if e.kind() == io::ErrorKind::AddrInUse => return Err(e),
            // Stock skips an interface it cannot configure (`AutoInterface.py` 314-319).
            Err(_) => {}
        }
    }
    if opened.is_empty() {
        let why = "no interface with an IPv6 link-local address could be adopted";
        return Err(io::Error::new(io::ErrorKind::NotFound, why));
    }
    let state = Arc::new(State {
        epoch: Instant::now(),
        table: Mutex::new(Table::default()),
        names: opened
            .iter()
            .map(|(a, _)| (a.index, a.name.clone()))
            .collect(),
        counters: Counters::default(),
    });
    let (stop, stopped) = watch::channel(false);
    let cfg = Arc::new(cfg);
    for (a, sockets) in opened {
        state
            .table
            .lock()
            .unwrap()
            .adopt(a.index, a.link_local, &a.all, state.now());
        let carrier = Carrier {
            ep: Arc::downgrade(ep),
            state: Arc::clone(&state),
            cfg: Arc::clone(&cfg),
            group,
            name: a.name,
            index: a.index,
            link_local: a.link_local,
            data: watch::channel(Arc::clone(&sockets.data)).0,
            sockets,
            follow,
        };
        tokio::spawn(carrier.run(stopped.clone()));
    }
    Ok(AutoHandle::new(state, stop))
}

struct Carrier {
    ep: Weak<Endpoint>,
    state: Arc<State>,
    cfg: Arc<AutoConfig>,
    group: Ipv6Addr,
    name: String,
    index: u32,
    link_local: Ipv6Addr,
    sockets: Sockets,
    /// The current data socket, which every peer pump of this interface sends from.
    data: watch::Sender<Arc<UdpSocket>>,
    follow: bool,
}

impl Carrier {
    async fn run(mut self, mut stop: watch::Receiver<bool>) {
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
        if token == Token::New {
            self.add_peer(src).await;
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
        tokio::spawn(pump(out, self.data.subscribe(), to, state));
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
        if self.follow {
            self.follow_link_local();
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
    fn follow_link_local(&mut self) {
        let Ok(Some(netif)) = netinfo::find(&self.name) else {
            return;
        };
        let Some(next) = keep_link_local(self.link_local, &netif.link_local) else {
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
        table.adopt(self.index, self.link_local, &netif.link_local, now);
    }
}

fn apply_policy(ep: &Endpoint, id: InterfaceId, p: &IfacePolicy) {
    ep.set_interface_ingress_policy(id, p.ingress);
    ep.set_interface_announce_rate(id, p.announce_rate);
    ep.set_announce_cap(id, p.cap_percent);
    ep.set_interface_bitrate(id, Some(p.bitrate_bps.unwrap_or(BITRATE_GUESS)));
    ep.set_interface_gravity(id, p.gravity);
    ep.set_interface_outgoing(id, p.outgoing);
    ep.set_interface_mode_flags(id, p.announces_from_internal, p.announces_to_internal);
}

/// Send a peer's packets from the interface's current data socket until the endpoint
/// detaches it.
async fn pump(
    mut out: OutboundPackets,
    data: watch::Receiver<Arc<UdpSocket>>,
    to: SocketAddrV6,
    state: Arc<State>,
) {
    while let Some(packet) = out.recv().await {
        let Ok(wire) = out.encode(&packet) else {
            continue;
        };
        let socket = Arc::clone(&data.borrow());
        if socket.send_to(&wire, to).await.is_ok() {
            State::count(&state.counters.tx);
        }
    }
}
