//! [`ReticulumTransportBuilder`]: the options a host sets before binding.

use std::path::PathBuf;
use std::time::Duration;

use identity::Ed25519Keypair;
use retinue::endpoint::NetworkStampPolicy;

use super::{ANNOUNCE_INTERVAL, CONNECT_TIMEOUT, ReticulumInterface, ReticulumTransport};
use crate::{Alpn, TransportError};

/// Builder for [`ReticulumTransport`].
pub struct ReticulumTransportBuilder<'a> {
    master: &'a Ed25519Keypair,
    alpns: Vec<Alpn>,
    watched: Vec<Alpn>,
    interfaces: Vec<ReticulumInterface>,
    announce_interval: Duration,
    connect_timeout: Duration,
    link_mtu: Option<u32>,
    reliable_links: bool,
    reliable_initial_rtt: Option<Duration>,
    reliable_max_window: Option<u32>,
    network_stamps: Option<NetworkStampPolicy>,
    stamp_storage: Option<PathBuf>,
}

impl<'a> ReticulumTransportBuilder<'a> {
    /// Start building a transport from the given master keypair.
    pub fn new(master: &'a Ed25519Keypair) -> Self {
        Self {
            master,
            alpns: Vec::new(),
            watched: Vec::new(),
            interfaces: Vec::new(),
            announce_interval: ANNOUNCE_INTERVAL,
            connect_timeout: CONNECT_TIMEOUT,
            link_mtu: None,
            reliable_links: false,
            reliable_initial_rtt: None,
            reliable_max_window: None,
            network_stamps: None,
            stamp_storage: None,
        }
    }

    /// ALPN protocols this transport will accept and announce.
    pub fn alpns(mut self, alpns: Vec<Alpn>) -> Self {
        self.alpns = alpns;
        self
    }

    /// ALPNs whose announces this transport learns peers from without serving
    /// or announcing them itself, so a reader can find holders of a name
    /// (see [`ReticulumTransport::announced_peers`]) and not look like one.
    pub fn watch_alpns(mut self, alpns: Vec<Alpn>) -> Self {
        self.watched = alpns;
        self
    }

    /// Network interfaces to attach (TCP server / client).
    pub fn interfaces(mut self, interfaces: Vec<ReticulumInterface>) -> Self {
        self.interfaces = interfaces;
        self
    }

    /// How often to re-announce registered destinations.
    pub fn announce_interval(mut self, interval: Duration) -> Self {
        self.announce_interval = interval;
        self
    }

    /// How long `connect` waits for peer discovery + link activation.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    /// MTU requested and offered by links opened through this transport.
    ///
    /// Retinue clamps this to its supported range. Packet radios commonly use
    /// 255 here so reliable-stream chunks fit the physical frame limit.
    pub fn link_mtu(mut self, mtu: u32) -> Self {
        self.link_mtu = Some(mtu);
        self
    }

    /// Use Retinue's acknowledged, retransmitted stream lane.
    ///
    /// Best-effort remains the default for fast or already-reliable bearers.
    /// Packet radios should enable this.
    pub fn reliable_links(mut self, reliable: bool) -> Self {
        self.reliable_links = reliable;
        self
    }

    /// Initial proof-turnaround estimate for reliable links.
    pub fn reliable_initial_rtt(mut self, rtt: Duration) -> Self {
        self.reliable_initial_rtt = Some(rtt);
        self
    }

    /// Maximum reliable frames in flight.
    ///
    /// A strict half-duplex radio normally wants one.
    pub fn reliable_max_window(mut self, frames: u32) -> Self {
        self.reliable_max_window = Some(frames);
        self
    }

    /// Network stamp emission and admission. The default is Retinue's, which matches
    /// RNS 1.6.1: stamp announces, path requests and requested link requests, enforce
    /// nothing. A peer that enforces drops an unstamped holder's announces.
    pub fn network_stamps(mut self, policy: NetworkStampPolicy) -> Self {
        self.network_stamps = Some(policy);
        self
    }

    /// An RNS-style storage path whose `stamps` directory keeps this host's destination
    /// stamps across restarts. Without one, each start mines its stamps again before
    /// its first announce. A stamp mined by stock RNS can be copied in.
    pub fn stamp_storage(mut self, storage: impl Into<PathBuf>) -> Self {
        self.stamp_storage = Some(storage.into());
        self
    }

    /// Bind the transport: create the retinue endpoint, attach interfaces,
    /// register destinations, and start the announce + accept driver tasks.
    pub async fn bind(self) -> Result<ReticulumTransport, TransportError> {
        ReticulumTransport::bind_inner(
            self.master,
            self.alpns,
            self.watched,
            self.interfaces,
            self.announce_interval,
            self.connect_timeout,
            self.link_mtu,
            self.reliable_links,
            self.reliable_initial_rtt,
            self.reliable_max_window,
            self.network_stamps,
            self.stamp_storage,
        )
        .await
    }
}
