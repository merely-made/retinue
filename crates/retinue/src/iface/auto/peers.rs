//! The AutoInterface peer table: a pure state machine over caller milliseconds
//! (`AutoInterface.py` 376-482, 526-612, 649-665).
//!
//! Peers are keyed by address and interface index, so one link-local seen on two links is
//! two peers; stock keys by address alone.

use alloc::vec::Vec;

use std::net::Ipv6Addr;

use super::{MAX_PEERS, MCAST_ECHO_TIMEOUT, MIF_LEN, MIF_TTL, PEERING_TIMEOUT, REVERSE_INTERVAL};
use crate::auto::HashRing;
use crate::iface::udp::frame_hash;

fn ms(d: core::time::Duration) -> u64 {
    d.as_millis() as u64
}

/// What a valid token means.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Token {
    /// One of our own addresses: multicast loopback, which marks the carrier alive.
    Echo,
    /// An unknown peer: the caller attaches its interface and [`Table::insert`]s it.
    New,
    /// A known peer, now refreshed.
    Refresh,
    /// An unknown peer on an interface that already has [`MAX_PEERS`]: ignored.
    Full,
}

/// What a data datagram means.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Data<'a, T> {
    /// Deliver it on this peer's interface.
    Admit(&'a T),
    /// The same bytes arrived from some peer within [`MIF_TTL`]: another path to it.
    Duplicate,
    /// Not from a peer; stock drops it silently (`AutoInterface.py` 610-612).
    Unknown,
}

/// One adopted interface's multicast health (`AutoInterface.py` 241, 462-480).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Carrier {
    pub index: u32,
    pub link_local: Ipv6Addr,
    last_echo: u64,
    /// Whether any own token has come back since adoption.
    pub echoed: bool,
    /// False once no own token came back for [`MCAST_ECHO_TIMEOUT`].
    pub up: bool,
}

#[derive(Debug)]
pub(super) struct Peer<T> {
    pub addr: Ipv6Addr,
    pub index: u32,
    pub last_heard: u64,
    last_out: u64,
    pub value: T,
}

/// What a job tick found on one interface.
#[derive(Debug, Default)]
pub(super) struct Tick<T> {
    /// Peers silent past [`PEERING_TIMEOUT`], removed; the caller detaches them.
    pub expired: Vec<Peer<T>>,
    /// Peers owed a unicast token (reverse peering).
    pub reverse: Vec<Ipv6Addr>,
    /// The carrier's new state, if it changed.
    pub carrier: Option<bool>,
}

#[derive(Debug)]
pub(super) struct Table<T> {
    /// Every link-local of every adopted interface: tokens from these are echoes.
    own: Vec<(u32, Ipv6Addr)>,
    carriers: Vec<Carrier>,
    peers: Vec<Peer<T>>,
    mif: HashRing<MIF_LEN>,
}

impl<T> Default for Table<T> {
    fn default() -> Self {
        Self {
            own: Vec::new(),
            carriers: Vec::new(),
            peers: Vec::new(),
            mif: HashRing::default(),
        }
    }
}

impl<T> Table<T> {
    /// Adopt (or re-adopt) interface `index` at `link_local`, with all of its link-locals.
    /// The echo clock starts now, as stock seeds it at adoption.
    pub fn adopt(&mut self, index: u32, link_local: Ipv6Addr, all: &[Ipv6Addr], now: u64) {
        self.own.retain(|(i, _)| *i != index);
        self.own.extend(all.iter().map(|a| (index, *a)));
        match self.carriers.iter_mut().find(|c| c.index == index) {
            Some(carrier) => carrier.link_local = link_local,
            None => self.carriers.push(Carrier {
                index,
                link_local,
                last_echo: now,
                echoed: false,
                up: true,
            }),
        }
    }

    pub fn carriers(&self) -> &[Carrier] {
        &self.carriers
    }

    pub fn peers(&self) -> &[Peer<T>] {
        &self.peers
    }

    /// Classify a token that already passed [`crate::auto::token_valid`] (`AutoInterface.py`
    /// 526-604).
    pub fn on_token(&mut self, src: Ipv6Addr, index: u32, now: u64) -> Token {
        if self.own.iter().any(|(_, a)| *a == src) {
            if let Some(c) = self.carriers.iter_mut().find(|c| c.link_local == src) {
                c.last_echo = now;
                c.echoed = true;
            }
            return Token::Echo;
        }
        if let Some(at) = self.position(src, index) {
            self.peers[at].last_heard = now;
            Token::Refresh
        } else if self.peers.iter().filter(|p| p.index == index).count() >= MAX_PEERS {
            Token::Full
        } else {
            Token::New
        }
    }

    /// Record a new peer. Its reverse token is due after [`REVERSE_INTERVAL`], as stock
    /// starts both clocks at creation.
    pub fn insert(&mut self, addr: Ipv6Addr, index: u32, value: T, now: u64) {
        self.peers.push(Peer {
            addr,
            index,
            last_heard: now,
            last_out: now,
            value,
        });
    }

    /// Classify a data datagram. Duplicates are dropped before they refresh the peer, and
    /// the dedup spans every peer of the interface (`AutoInterface.py` 649-665).
    pub fn on_data(&mut self, src: Ipv6Addr, index: u32, frame: &[u8], now: u64) -> Data<'_, T> {
        let Some(at) = self.position(src, index) else {
            return Data::Unknown;
        };
        let hash = frame_hash(frame);
        if self.mif.fresh(&hash, now, ms(MIF_TTL)) {
            return Data::Duplicate;
        }
        self.mif.insert(hash, now);
        let peer = &mut self.peers[at];
        peer.last_heard = now;
        Data::Admit(&peer.value)
    }

    /// The 4 s job for interface `index` (`AutoInterface.py` 376-408, 462-477).
    pub fn tick(&mut self, index: u32, now: u64) -> Tick<T> {
        let mut tick = Tick {
            expired: Vec::new(),
            reverse: Vec::new(),
            carrier: None,
        };
        let mut kept = Vec::with_capacity(self.peers.len());
        for peer in self.peers.drain(..) {
            if peer.index == index && now > peer.last_heard + ms(PEERING_TIMEOUT) {
                tick.expired.push(peer);
            } else {
                kept.push(peer);
            }
        }
        self.peers = kept;
        for peer in self.peers.iter_mut().filter(|p| p.index == index) {
            if now > peer.last_out + ms(REVERSE_INTERVAL) {
                peer.last_out = now;
                tick.reverse.push(peer.addr);
            }
        }
        if let Some(c) = self.carriers.iter_mut().find(|c| c.index == index) {
            let up = now.saturating_sub(c.last_echo) <= ms(MCAST_ECHO_TIMEOUT);
            if up != c.up {
                c.up = up;
                tick.carrier = Some(up);
            }
        }
        tick
    }

    /// Remove every peer on interface `index`, for the caller to detach.
    pub fn drain(&mut self, index: u32) -> Vec<Peer<T>> {
        let (gone, kept) = self.peers.drain(..).partition(|p| p.index == index);
        self.peers = kept;
        gone
    }

    fn position(&self, addr: Ipv6Addr, index: u32) -> Option<usize> {
        self.peers
            .iter()
            .position(|p| p.addr == addr && p.index == index)
    }
}
