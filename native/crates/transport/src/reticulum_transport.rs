// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! Reticulum-backed [`Transport`] implementation.
//!
//! [`ReticulumTransport`] runs Mere's bilateral peer-to-peer lane over Reticulum
//! links, using [`retinue`] — Mark's from-scratch Rust implementation of the
//! Reticulum Network Stack, qualified against RNS 1.5.7 within the interoperability
//! scope recorded in `crates/retinue/README.md`. It is feature-gated behind the
//! `reticulum` cargo feature and is default-off. This is the bilateral stream
//! lane: gossip and blob transfer remain iroh-only, and catch-up sync rides the
//! stream through `sync_lane`.
//!
//! ## Identity
//!
//! Mere's master key is a single Ed25519 keypair; a retinue identity is dual-key
//! (X25519 ECDH + Ed25519 signing). [`keys::derive_identity`] derives the X25519
//! half via HKDF-SHA256 and uses the Mere key as the Ed25519 half, so a verified
//! Reticulum announce already carries the [`PeerID`] without another signature.
//! The same seed always yields the same retinue destination.
//!
//! ## Discovery
//!
//! A destination hash cannot be synthesized from a [`PeerID`], so peers are
//! learned from authenticated announces (see [`announce`]). The Reticulum
//! identity's signing key is the `PeerID`; its own announce signature binds that
//! key, the X25519 half, and the destination name. `connect` resolves a peer by
//! looking it up in the announce-populated address book; `accept` waits for an
//! inbound link on the ALPN's destination.
//!
//! ## ALPN mapping
//!
//! `mere/cable/v1` maps to `DestinationName::new("mere", ["cable", "v1"])`: the first
//! path segment is the Reticulum app name, the rest are its aspects. The expanded name
//! is `mere.cable.v1`, so the destination hash is the one RNS derives.
//!
//! [`retinue`]: https://github.com/merely-made/murm

mod announce;
mod builder;
mod keys;
mod stream;

pub use builder::ReticulumTransportBuilder;

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use identity::{CarrierKeypair, Ed25519Keypair};
use tokio::sync::{Mutex as TokioMutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, interval, sleep, timeout};

use retinue::destination::DestinationName;
use retinue::endpoint::{Endpoint, Interface, LinkStream, NetworkStampPolicy, StampDirectory};
use retinue::hash::AddressHash;
use retinue::identity::Identity;

use crate::{
    AcceptedSession, Alpn, IngressContext, IngressInterfaceId, PeerID, Transport, TransportError,
};

use self::announce::{build_app_data, recover_peer_id};
use self::keys::derive_identity;

pub use self::stream::ReticulumStream;

/// Default interval between periodic re-announces of registered destinations.
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(5);

/// Default time `connect` waits for peer discovery + link activation.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Learned peers: `PeerID -> (ALPN -> that peer's retinue identity)`.
type PeerMap = Arc<StdMutex<HashMap<PeerID, HashMap<Alpn, Identity>>>>;

/// A network interface to attach to a [`ReticulumTransport`].
#[derive(Clone, Debug)]
pub enum ReticulumInterface {
    /// TCP server accepting incoming connections.
    TcpServer {
        /// Address to bind, e.g. `127.0.0.1:4242`.
        bind: SocketAddr,
    },
    /// TCP client dialing a peer server.
    TcpClient {
        /// Address to connect to.
        addr: SocketAddr,
    },
}

/// Reticulum-backed implementation of the [`Transport`] trait.
pub struct ReticulumTransport {
    local_peer_id: PeerID,
    endpoint: Arc<Endpoint>,
    peers: PeerMap,
    names: Arc<HashMap<Alpn, DestinationName>>,
    app_data: Arc<HashMap<Alpn, Vec<u8>>>,
    /// Per-ALPN inbound accept queue, fed by the accept-router task. Behind an
    /// async mutex because `accept` takes `&self` yet must own the receiver while
    /// awaiting.
    inbound: HashMap<Alpn, Arc<TokioMutex<mpsc::UnboundedReceiver<InboundLink>>>>,
    connect_timeout: Duration,
    reliable_links: bool,
    /// Background driver tasks (accept router, announce listener, announce
    /// sender), aborted on drop.
    tasks: Vec<JoinHandle<()>>,
}

impl Drop for ReticulumTransport {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl ReticulumTransport {
    /// Construct a builder for a `ReticulumTransport`.
    pub fn builder(master: &Ed25519Keypair) -> ReticulumTransportBuilder<'_> {
        ReticulumTransportBuilder::new(master)
    }

    /// Construct a builder from a custodied key: a derived persona key or an
    /// unassociated one. An unassociated key yields a Reticulum identity (and
    /// `PeerID`) that shares nothing with the persona's plain identity.
    pub fn builder_for(key: &impl CarrierKeypair) -> ReticulumTransportBuilder<'_> {
        ReticulumTransportBuilder::new(key.keypair())
    }

    // This is the builder's one-to-one handoff; a parameter bundle here would
    // duplicate the public builder state solely to appease a lint.
    #[allow(clippy::too_many_arguments)]
    async fn bind_inner(
        master: &Ed25519Keypair,
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
    ) -> Result<Self, TransportError> {
        let local_peer_id = PeerID::from_public_key(master.public_key());
        let private_identity = derive_identity(master);
        let identity = *private_identity.public();

        let endpoint = Endpoint::new(private_identity);
        if let Some(link_mtu) = link_mtu {
            endpoint.set_link_mtu(link_mtu);
        }
        if let Some(rtt) = reliable_initial_rtt {
            endpoint.set_reliable_initial_rtt(rtt);
        }
        if let Some(window) = reliable_max_window {
            endpoint.set_reliable_max_window(window);
        }
        if let Some(policy) = network_stamps {
            endpoint.set_network_stamp_policy(policy);
        }
        if let Some(storage) = stamp_storage {
            endpoint.set_destination_stamp_store(StampDirectory::in_storage(&storage));
        }

        // Attach interfaces.
        for iface in interfaces {
            match iface {
                ReticulumInterface::TcpServer { bind } => {
                    endpoint.listen_tcp(bind).await?;
                },
                ReticulumInterface::TcpClient { addr } => {
                    endpoint.attach_tcp_client(addr).await?;
                },
            }
        }

        // Register one destination per ALPN. Record each ALPN's destination name
        // and its precomputed signed announce payload; wire up a per-ALPN inbound
        // accept queue keyed (for routing) by the destination hash.
        let mut names: HashMap<Alpn, DestinationName> = HashMap::new();
        let mut app_data: HashMap<Alpn, Vec<u8>> = HashMap::new();
        let mut inbound = HashMap::new();
        let mut inbound_senders: HashMap<AddressHash, mpsc::UnboundedSender<InboundLink>> =
            HashMap::new();
        for alpn in &alpns {
            let name = destination_name_for_alpn(alpn);
            let data = build_app_data(&local_peer_id, &name, &identity, master);
            let dest = name.destination_hash(&identity);
            if reliable_links {
                endpoint.register_reliable(name.clone(), &data);
            } else {
                endpoint.register(name.clone(), &data);
            }

            let (tx, rx) = mpsc::unbounded_channel();
            inbound_senders.insert(dest, tx);
            inbound.insert(alpn.clone(), Arc::new(TokioMutex::new(rx)));
            names.insert(alpn.clone(), name);
            app_data.insert(alpn.clone(), data);
        }

        // Watched names are matched against announces but never registered.
        for alpn in watched {
            names
                .entry(alpn.clone())
                .or_insert_with(|| destination_name_for_alpn(&alpn));
        }

        let endpoint = Arc::new(endpoint);
        let names = Arc::new(names);
        let app_data = Arc::new(app_data);
        let peers: PeerMap = Arc::new(StdMutex::new(HashMap::new()));

        let tasks = vec![
            // Accept router: dispatch each inbound link to its ALPN's queue by the
            // destination it targeted.
            tokio::spawn(run_accept_router(
                Arc::clone(&endpoint),
                inbound_senders,
                reliable_links,
            )),
            // Announce listener: learn peers from validated announce bindings.
            tokio::spawn(run_announce_listener(
                Arc::clone(&endpoint),
                Arc::clone(&names),
                Arc::clone(&peers),
            )),
            // Announce sender: periodically re-announce our destinations.
            tokio::spawn(run_announce_sender(
                Arc::clone(&endpoint),
                Arc::clone(&names),
                Arc::clone(&app_data),
                announce_interval,
            )),
        ];

        Ok(Self {
            local_peer_id,
            endpoint,
            peers,
            names,
            app_data,
            inbound,
            connect_timeout,
            reliable_links,
            tasks,
        })
    }

    /// Attach one transport-neutral Retinue packet interface.
    ///
    /// The caller owns the physical driver. Tulle's packet-radio bridge, a
    /// deterministic test link, or another bearer can drive this same seam
    /// without putting serial ports or PHY policy into Murm.
    pub fn attach_packet_interface(&self) -> Interface {
        self.endpoint.attach_interface()
    }

    /// Queue one announce for every ALPN registered on this transport.
    ///
    /// A packet interface is attached after the transport's background tasks
    /// start, so their initial timer tick may precede the physical driver.
    /// Call this after starting that driver to make discovery immediate and to
    /// avoid short periodic intervals that saturate a half-duplex radio.
    pub fn announce_now(&self) {
        for (alpn, name) in self.names.iter() {
            if let Some(data) = self.app_data.get(alpn) {
                self.endpoint.announce(name, data);
            }
        }
    }

    /// Peers whose verified announces named `alpn`, in no particular order.
    ///
    /// Only ALPNs this transport registered or watched are learned. A peer
    /// that announced a name proved the destination, not that it serves
    /// anything under it: the caller verifies what the peer then sends.
    pub fn announced_peers(&self, alpn: &Alpn) -> Vec<PeerID> {
        let peers = self.peers.lock().expect("peers mutex poisoned");
        peers
            .iter()
            .filter(|(_, by_alpn)| by_alpn.contains_key(alpn))
            .map(|(peer, _)| *peer)
            .collect()
    }

    /// Stop the Retinue endpoint after allowing already-written link bytes to
    /// reach its interface queues.
    ///
    /// Drop every accepted or connected stream before calling this. A stream
    /// still held by a caller cannot reach EOF and is bounded by `grace`.
    pub async fn shutdown(&self, grace: Duration) {
        self.endpoint.shutdown(grace).await;
    }

    /// Poll the announce-populated address book for a peer's retinue identity on
    /// an ALPN, up to [`connect_timeout`](Self::connect_timeout).
    async fn resolve_peer(&self, peer: &PeerID, alpn: &Alpn) -> Result<Identity, TransportError> {
        let start = Instant::now();
        loop {
            if let Some(identity) = self
                .peers
                .lock()
                .expect("peers mutex poisoned")
                .get(peer)
                .and_then(|by_alpn| by_alpn.get(alpn))
                .copied()
            {
                return Ok(identity);
            }
            if start.elapsed() >= self.connect_timeout {
                return Err(TransportError::Backend(
                    "peer not discovered before timeout".into(),
                ));
            }
            sleep(Duration::from_millis(100)).await;
        }
    }
}

/// Map an ALPN string to a retinue destination name: the first `/`-segment is the
/// app name, the rest are aspects (`mere/cable/v1` -> `("mere", ["cable", "v1"])`).
/// Separate aspects expand to the same `mere.cable.v1` as one dotted aspect did, and RNS
/// (and retinue from 0.3.1) refuse a dot inside an aspect.
fn destination_name_for_alpn(alpn: &Alpn) -> DestinationName {
    let text = String::from_utf8_lossy(alpn.as_bytes());
    let mut parts = text.splitn(2, '/');
    let app = parts.next().filter(|s| !s.is_empty()).unwrap_or("mere");
    let rest = parts.next().unwrap_or("");
    DestinationName::new(app, rest.split('/'))
}

/// Accept-router task: forward each inbound link to the queue for the ALPN whose
/// destination it targeted. Ends when the endpoint closes.
/// An inbound Reticulum link queued for `accept`, with the ingress facts
/// retinue reported when it arrived.
///
/// Reticulum best-effort acceptance cannot identify its initiator, so there is
/// deliberately no peer here: the application identity arrives later through a
/// session proof (plan D6).
struct InboundLink {
    stream: LinkStream,
    link: AddressHash,
    interface: IngressInterfaceId,
}

async fn run_accept_router(
    endpoint: Arc<Endpoint>,
    senders: HashMap<AddressHash, mpsc::UnboundedSender<InboundLink>>,
    reliable_links: bool,
) {
    loop {
        let accepted = if reliable_links {
            endpoint.accept_reliable_on_any().await
        } else {
            endpoint.accept_on_any().await
        };
        let Ok(accepted) = accepted else {
            break;
        };
        if let Some(tx) = senders.get(&accepted.destination) {
            // Carry the ingress facts retinue reported (plan V3/V4): the
            // router used to drop them here, which left `accept` unable to say
            // which bearer a session arrived over.
            let _ = tx.send(InboundLink {
                link: accepted.stream.link_id(),
                interface: IngressInterfaceId(u64::from(accepted.interface)),
                stream: accepted.stream,
            });
        }
    }
}

/// Announce-listener task: drain validated announces, recover the `PeerID`
/// binding, and record `peer_id -> (alpn, identity)`.
async fn run_announce_listener(
    endpoint: Arc<Endpoint>,
    names: Arc<HashMap<Alpn, DestinationName>>,
    peers: PeerMap,
) {
    loop {
        let announce = match endpoint.next_announcement().await {
            Ok(a) => a,
            Err(_) => break,
        };
        // Match the announce to one of our ALPNs by reconstructing that ALPN's
        // destination hash from the announcing identity, then verify the binding.
        for (alpn, name) in names.iter() {
            if name.destination_hash(&announce.identity) != announce.destination {
                continue;
            }
            if let Some(peer_id) = recover_peer_id(&announce.app_data, name, &announce.identity) {
                peers
                    .lock()
                    .expect("peers mutex poisoned")
                    .entry(peer_id)
                    .or_default()
                    .insert(alpn.clone(), announce.identity);
            }
            break;
        }
    }
}

/// Announce-sender task: periodically re-announce every registered destination
/// with its precomputed, signed `app_data`.
async fn run_announce_sender(
    endpoint: Arc<Endpoint>,
    names: Arc<HashMap<Alpn, DestinationName>>,
    app_data: Arc<HashMap<Alpn, Vec<u8>>>,
    period: Duration,
) {
    let mut timer = interval(period);
    loop {
        timer.tick().await;
        for (alpn, name) in names.iter() {
            if let Some(data) = app_data.get(alpn) {
                endpoint.announce(name, data);
            }
        }
    }
}

impl Transport for ReticulumTransport {
    type Stream = ReticulumStream;

    fn local_peer_id(&self) -> PeerID {
        self.local_peer_id
    }

    async fn connect(&self, peer: PeerID, alpn: Alpn) -> Result<ReticulumStream, TransportError> {
        // Resolve the peer's retinue identity for this ALPN from learned announces.
        let identity = self.resolve_peer(&peer, &alpn).await?;
        let dest = destination_name_for_alpn(&alpn).destination_hash(&identity);

        let open = async {
            if self.reliable_links {
                self.endpoint.open_reliable(dest, identity).await
            } else {
                self.endpoint.open(dest, identity).await
            }
        };
        let stream = timeout(self.connect_timeout, open)
            .await
            .map_err(|_| TransportError::Backend("link setup timed out".into()))??;

        Ok(ReticulumStream::new(stream))
    }

    async fn connect_session(
        &self,
        peer: PeerID,
        alpn: Alpn,
    ) -> Result<crate::OutboundSession<ReticulumStream>, TransportError> {
        let stream = self.connect(peer, alpn.clone()).await?;
        let link = stream.link_id();
        Ok(crate::OutboundSession::new(stream, alpn, None, Some(link)))
    }

    async fn accept(&self, alpn: Alpn) -> Result<AcceptedSession<ReticulumStream>, TransportError> {
        let queue = self
            .inbound
            .get(&alpn)
            .cloned()
            .ok_or(TransportError::AlpnNotRegistered)?;
        let mut rx = queue.lock().await;
        match rx.recv().await {
            Some(inbound) => Ok(AcceptedSession::new(
                ReticulumStream::new(inbound.stream),
                alpn,
                // Best-effort Reticulum acceptance cannot identify its
                // initiator. Reporting `None` is the honest answer; a session
                // proof supplies the application identity later (plan D6).
                None,
                IngressContext::reticulum(inbound.interface, *inbound.link.as_bytes()),
            )),
            None => Err(TransportError::Closed),
        }
    }
}

#[cfg(test)]
mod tests;
