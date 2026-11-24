// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! p2panda-net-backed [`Transport`] implementation — the production transport.
//!
//! [`P2pandaTransport`] makes `p2panda-net`'s `Endpoint` the endpoint authority:
//! it runs a real iroh QUIC endpoint (via p2panda-net), demultiplexes incoming
//! connections by ALPN through p2panda-net's `Endpoint::accept` (iroh
//! `ProtocolHandler`) into per-ALPN queues that [`Transport::accept`] drains, and
//! opens a fresh bidirectional stream per [`Transport::connect`]. It optionally
//! serves the `iroh-blobs` protocol off the same endpoint.
//!
//! Compared to the retired hand-rolled `IrohTransport` Router, this gains
//! p2panda-net's discovery (`Discovery` / `MdnsDiscovery`), relay/hole-punching,
//! NAT port-mapping, and actor supervision for free, and keeps the same
//! one-endpoint / many-protocols / raw-ALPN-stream behavior the `Transport`
//! trait promises.
//!
//! ## Identity
//!
//! Constructed from an [`identity::Ed25519Keypair`] or a provider-neutral raw
//! 32-byte Ed25519 signing seed. The latter lets sibling applications use an
//! external identity provider such as Personae without coupling this crate to
//! it. In either case the seed bridges the ed25519-dalek major-version boundary
//! without forcing either side to upgrade.
//!
//! ## Discovery
//!
//! Three mechanisms share the one endpoint, each opt-in via the builder:
//!
//! - **Explicit peer** (tests, LAN): a peer's [`endpoint_addr`] (its real
//!   interface addresses, with loopback covering wildcard binds) is shared
//!   out-of-band and inserted via [`add_peer`].
//! - **mDNS** (`builder().mdns(..)`): same-network peers auto-populate the
//!   address book.
//! - **Random-walk** (`builder().discovery()`): walkers explore outward from
//!   bootstrap nodes already in the address book to reach internet peers; the
//!   handle is held for the transport's life.
//!
//! [`Ed25519Keypair::to_seed`]: identity::Ed25519Keypair::to_seed
//! [`endpoint_addr`]: P2pandaTransport::endpoint_addr
//! [`add_peer`]: P2pandaTransport::add_peer

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::Duration;

use identity::CarrierKeypair;
#[cfg(any(test, feature = "test-identity"))]
use identity::Ed25519Keypair;
use iroh::EndpointAddr;
use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler};
use iroh_tickets::endpoint::EndpointTicket;
use p2panda_core::{SigningKey, Topic, VerifyingKey};
pub use p2panda_net::Endpoint;
use p2panda_net::addrs::NodeInfo;
use p2panda_net::discovery::DiscoveryConfig;
pub use p2panda_net::gossip::Gossip;

use p2panda_net::gossip::GossipHandle;
// Re-exported: [`P2pandaTransportBuilder::mdns`] is public but its argument
// type was not, so no consumer outside this crate could call it.
pub use p2panda_net::iroh_mdns::MdnsDiscoveryMode;
// Re-exported so a consumer can configure a relay without taking a direct iroh
// dependency of its own.
pub use iroh::RelayUrl;
use muniment::{JsonCodec, MemoryBackend};
use murm_sync::MunimentAddressBook;
use p2panda_net::address_book::AddressBookStoreHandle;
use p2panda_net::{AddressBook, Discovery, MdnsDiscovery};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::{Mutex as TokioMutex, mpsc};

mod bounded_accept;
use bounded_accept::AcceptReceiver;
pub use bounded_accept::{BoundedAcceptHandle, BoundedAcceptStats};
use zeroize::{Zeroize, Zeroizing};

#[cfg(feature = "noise")]
mod native_security;
mod open_connections;

use open_connections::OpenConnections;

use crate::blob_source::{SharedBlobSource, StoreBlobSource, plan_request};
use crate::blobs::{BlobPeerAuthorizer, BlobReadAuthorizer, BlobScope, BlobStore};
use crate::peer_route::{PeerPath, peer_addr};
use crate::{AcceptedSession, Alpn, PeerID, Transport, TransportError};

/// A bidirectional p2panda-net QUIC stream presented as `AsyncRead + AsyncWrite`.
///
/// `_connection` is held so the QUIC connection stays open for the stream's life.
pub struct P2pandaStream {
    #[cfg(feature = "noise")]
    local_peer: PeerID,
    #[cfg(feature = "noise")]
    service_protocol: Alpn,
    send: SendStream,
    recv: RecvStream,
    _connection: Connection,
    shutdown_ack: Option<Pin<Box<dyn Future<Output = std::io::Result<()>> + Send>>>,
    shutdown_finished: bool,
    shutdown_complete: bool,
}

#[cfg(feature = "sync-lane")]
impl P2pandaStream {
    pub(crate) fn connection(&self) -> &Connection {
        &self._connection
    }
}

impl std::fmt::Debug for P2pandaStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("P2pandaStream").finish_non_exhaustive()
    }
}

impl AsyncRead for P2pandaStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // Disambiguate to the tokio AsyncRead impl (RecvStream also has an
        // inherent read with a different error type).
        <RecvStream as AsyncRead>::poll_read(Pin::new(&mut self.recv), cx, buf)
    }
}

impl AsyncWrite for P2pandaStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        <SendStream as AsyncWrite>::poll_write(Pin::new(&mut self.send), cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        <SendStream as AsyncWrite>::poll_flush(Pin::new(&mut self.send), cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        if self.shutdown_complete {
            return Poll::Ready(Ok(()));
        }
        if self.shutdown_ack.is_none() {
            // Create this before finishing so `stopped` cannot miss the
            // acknowledgement that all stream bytes reached the peer. The
            // connection handle is the stream's last handle in the usual
            // one-stream case; dropping it immediately after `finish` can
            // otherwise race the peer reading a small final frame.
            let stopped = self.send.stopped();
            self.shutdown_ack = Some(Box::pin(async move {
                match stopped.await {
                    Ok(None) => Ok(()),
                    Ok(Some(code)) => Err(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        format!("peer stopped stream before acknowledging final bytes: {code}"),
                    )),
                    Err(error) => Err(std::io::Error::from(error)),
                }
            }));
        }
        if !self.shutdown_finished {
            match <SendStream as AsyncWrite>::poll_shutdown(Pin::new(&mut self.send), cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Ready(Ok(())) => self.shutdown_finished = true,
            }
        }
        let acknowledgement = self
            .shutdown_ack
            .as_mut()
            .expect("shutdown acknowledgement initialized");
        match acknowledgement.as_mut().poll(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.shutdown_ack = None;
                self.shutdown_complete = result.is_ok();
                Poll::Ready(result)
            },
        }
    }
}

/// A queued inbound p2panda stream and the peer the transport authenticated.
///
/// The peer is captured in the handler, where the [`Connection`] is still in
/// hand: `accept` drains a queue and no longer has it. `None` only if the
/// remote key fails to decode, which would mean the connection is not usable
/// as an authenticated peer anyway.
struct QueuedStream {
    stream: P2pandaStream,
    peer: Option<PeerID>,
    arrived_at: tokio::time::Instant,
    _queue_permit: Option<tokio::sync::OwnedSemaphorePermit>,
}

/// Registered per ALPN on the p2panda-net endpoint; pushes each accepted
/// bi-stream to a queue that [`Transport::accept`] drains.
#[derive(Debug, Clone)]
struct StreamQueueHandler {
    tx: mpsc::UnboundedSender<QueuedStream>,
    #[cfg(feature = "noise")]
    local_peer: PeerID,
    #[cfg(feature = "noise")]
    service_protocol: Alpn,
}

impl ProtocolHandler for StreamQueueHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let arrived_at = tokio::time::Instant::now();
        // p2panda authenticates its connections, so the remote id is a
        // transport fact (plan D4) and may be reported as the peer.
        let peer = PeerID::from_bytes(connection.remote_id().as_bytes()).ok();
        let (send, recv) = connection
            .accept_bi()
            .await
            .map_err(AcceptError::from_err)?;
        let _ = self.tx.send(QueuedStream {
            stream: P2pandaStream {
                #[cfg(feature = "noise")]
                local_peer: self.local_peer,
                #[cfg(feature = "noise")]
                service_protocol: self.service_protocol.clone(),
                send,
                recv,
                _connection: connection,
                shutdown_ack: None,
                shutdown_finished: false,
                shutdown_complete: false,
            },
            peer,
            arrived_at,
            _queue_permit: None,
        });
        Ok(())
    }
}

/// Domain-authorized wrapper around the ordinary iroh-blobs protocol.
///
/// Authorization is evaluated from the transport-authenticated remote key
/// before the peer can name or read any hash in the store.
struct AuthorizedBlobsProtocol {
    inner: iroh_blobs::BlobsProtocol,
    authorizer: BlobPeerAuthorizer,
}

/// Hash-scoped provider for iroh-blobs requests.
///
/// iroh authenticates the remote endpoint and parses each request. Murm asks
/// the [`BlobSource`](crate::blob_source::BlobSource) about every named hash
/// before any byte is written, then writes the source's verified stream for
/// each blob in wire order.
struct ScopedBlobsProtocol {
    source: SharedBlobSource,
    scope: BlobScope,
}

impl std::fmt::Debug for ScopedBlobsProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedBlobsProtocol")
            .field("scope", &self.scope)
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for ScopedBlobsProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        use iroh_blobs::protocol::Request;
        use iroh_blobs::util::SendStream as _;

        let peer = *connection.remote_id().as_bytes();
        while let Ok(mut pair) = iroh_blobs::provider::StreamPair::accept(
            &connection,
            iroh_blobs::provider::events::EventSender::DEFAULT,
        )
        .await
        {
            let request = match pair.read_request().await {
                Ok(request) => request,
                Err(_) => continue,
            };
            let Some(wanted) = plan_request(&*self.source, self.scope, &peer, &request).await
            else {
                connection.close(
                    iroh_blobs::protocol::ERR_PERMISSION,
                    b"hash is not authorized in this blob scope",
                );
                break;
            };
            let source = self.source.clone();
            let scope = self.scope;
            tokio::spawn(async move {
                let tracker = match &request {
                    Request::Get(get) => pair.get_request(|| get.clone()).await,
                    Request::GetMany(many) => pair.get_many_request(|| many.clone()).await,
                    _ => return,
                };
                let Ok(tracker) = tracker else { return };
                let Ok(mut writer) = pair.into_writer(tracker).await else {
                    return;
                };
                let sent: std::io::Result<()> = async {
                    for item in wanted {
                        let mut stream = source
                            .export_bao(scope, &peer, item.hash, item.ranges)
                            .await?;
                        while let Some(piece) = stream.next().await? {
                            writer.inner.send_bytes(piece).await?;
                        }
                    }
                    writer.inner.sync().await
                }
                .await;
                if sent.is_err() {
                    let _ = writer.inner.reset(iroh_blobs::protocol::ERR_INTERNAL);
                }
            });
        }
        Ok(())
    }
}

impl std::fmt::Debug for AuthorizedBlobsProtocol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthorizedBlobsProtocol")
            .field("admitted_peers", &self.authorizer.peers().len())
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for AuthorizedBlobsProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if !self.authorizer.allows(connection.remote_id().as_bytes()) {
            return Err(AcceptError::from_err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "peer is not authorized for this blob store",
            )));
        }
        self.inner.accept(connection).await
    }

    async fn shutdown(&self) {
        self.inner.shutdown().await;
    }
}

type AlpnQueues = Arc<StdMutex<HashMap<Alpn, Arc<TokioMutex<AcceptReceiver>>>>>;

type AddressStore = MunimentAddressBook<MemoryBackend, JsonCodec>;

/// A peer the address book associates with a topic.
///
/// Identity and reachability are separate facts and this type keeps them so. A
/// paired device is named by its peer id forever; whether it can be reached
/// right now is a property of what discovery has currently resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KnownPeer {
    /// The peer's durable identity. Never changes, unlike its address.
    pub peer: PeerID,
    /// False when the address book holds the identity but no transport
    /// information yet: discovery has named this peer without yet saying how
    /// to reach it.
    ///
    /// This is knowledge of an address, NOT a live link. A peer can be
    /// `reachable` for hours while every packet to it is dropped; read
    /// [`connected`](Self::connected) for the question most callers mean.
    pub reachable: bool,
    /// Whether this node is configured as a discovery bootstrap locally.
    pub bootstrap: bool,
    /// Whether this node is talking to the peer now.
    ///
    /// While this node is subscribed to the topic's gossip, this is whether
    /// the peer is a direct gossip neighbour there (a `NeighbourUp` not yet
    /// followed by a `NeighbourDown`), whatever iroh's path says. While it is
    /// not subscribed, this is whether at least one connection to the peer is
    /// open on this endpoint, whichever protocol opened it.
    ///
    /// The honest answer to "are we talking to it", as opposed to "do we know
    /// where it lives". Distinguishing these is not pedantry: a firewall rule
    /// silently dropped every inbound packet to one of these devices for
    /// hours, and because the address book still held an address, the host
    /// reported the peer as reachable throughout and looked healthy while
    /// nothing whatsoever replicated (2026-08-03).
    ///
    /// Neither answer reads iroh's path state, which is wrong both ways: when
    /// both sides dial at once it can mark every address of a delivering peer
    /// inactive for 45 s and more, and it keeps a dead peer's path active for
    /// a minute or more after its last connection closes. iroh's own view
    /// stays readable, unmixed, in [`peer_paths`](P2pandaTransport::peer_paths).
    pub connected: bool,
    /// Whether this node is subscribed to the topic's gossip, so `connected`
    /// is gossip's answer; false means the open-connection count gave it.
    pub on_overlay: bool,
}

/// How, and to whom, this transport serves iroh-blobs.
///
/// One choice rather than a store plus two optional authorizers: the three were
/// mutually exclusive in every builder method that set them, and the reader of
/// the bind path had to reconstruct that from a priority chain of `if let`s.
enum BlobServing<'a> {
    /// Do not register the blobs protocol at all.
    None,
    /// Serve the store to any peer that connects.
    Plain(&'a BlobStore),
    /// Serve only peers the domain-owned authorizer admits.
    Authorized(&'a BlobStore, BlobPeerAuthorizer),
    /// Serve only hashes retained by one scope, to that scope's readers.
    Scoped(&'a BlobStore, BlobScope, BlobReadAuthorizer),
    /// Serve only what a source allows for one scope, with no store in this
    /// process.
    Source(SharedBlobSource, BlobScope),
}

/// The seed a builder holds (vault lock ruling 51): boxed, so moving the
/// builder through `bind`'s futures moves a pointer, and the bytes stay in
/// one allocation, cleared when it drops. A `[u8; 32]` is `Copy`, and every
/// future frame that took one by value kept a copy.
type HeldSeed = Box<Zeroizing<[u8; 32]>>;

fn hold(seed: &[u8; 32]) -> HeldSeed {
    let mut held = Box::new(Zeroizing::new([0u8; 32]));
    held.copy_from_slice(seed);
    held
}

/// Builder for [`P2pandaTransport`]. Use the appropriate scoped-identity constructor on [`P2pandaTransport`].
pub struct P2pandaTransportBuilder<'a> {
    signing_seed: HeldSeed,
    alpns: Vec<Alpn>,
    blobs: BlobServing<'a>,
    mdns: Option<MdnsDiscoveryMode>,
    discovery: Option<DiscoveryConfig>,
    gossip: bool,
    relay_urls: Vec<iroh::RelayUrl>,
    /// Off only in the control test that proves the count matters.
    #[cfg(test)]
    connection_hook: bool,
}

impl<'a> P2pandaTransportBuilder<'a> {
    /// Bind without the open-connection hook, for the control test only.
    #[cfg(test)]
    fn without_connection_hook(mut self) -> Self {
        self.connection_hook = false;
        self
    }

    /// Mere-defined ALPNs to register; each gets its own accept queue.
    pub fn alpns(mut self, alpns: Vec<Alpn>) -> Self {
        self.alpns = alpns;
        self
    }

    /// Serve the iroh-blobs protocol against the given store. Peers can then
    /// `fetch_from` this transport's PeerID.
    pub fn blobs<'b>(self, store: &'b BlobStore) -> P2pandaTransportBuilder<'b> {
        self.serving(BlobServing::Plain(store))
    }

    /// Serve blobs only to peers admitted by the domain-owned authorizer.
    ///
    /// The peer identity comes from the authenticated QUIC connection. The
    /// transport does not infer pairing, membership, or capability authority.
    pub fn authorized_blobs<'b>(
        self,
        store: &'b BlobStore,
        authorizer: BlobPeerAuthorizer,
    ) -> P2pandaTransportBuilder<'b> {
        self.serving(BlobServing::Authorized(store, authorizer))
    }

    /// Serve only hashes retained by `scope` to that scope's current readers.
    pub fn scoped_blobs<'b>(
        self,
        store: &'b BlobStore,
        scope: BlobScope,
        authorizer: BlobReadAuthorizer,
    ) -> P2pandaTransportBuilder<'b> {
        self.serving(BlobServing::Scoped(store, scope, authorizer))
    }

    /// Serve `scope` from a [`BlobSource`](crate::blob_source::BlobSource)
    /// instead of a store held here: the provider asks the source who may
    /// read what and takes each blob's verified stream from it. This is how a
    /// supervised child serves blobs whose custody is another process's.
    pub fn scoped_source<'b>(
        self,
        source: SharedBlobSource,
        scope: BlobScope,
    ) -> P2pandaTransportBuilder<'b> {
        self.serving(BlobServing::Source(source, scope))
    }

    /// Rebind the builder to one blob-serving choice, replacing any earlier
    /// one. The lifetime moves with the store, which is the only borrow the
    /// builder holds.
    fn serving<'b>(self, blobs: BlobServing<'b>) -> P2pandaTransportBuilder<'b> {
        P2pandaTransportBuilder {
            signing_seed: self.signing_seed,
            alpns: self.alpns,
            blobs,
            mdns: self.mdns,
            discovery: self.discovery,
            gossip: self.gossip,
            relay_urls: self.relay_urls,
            #[cfg(test)]
            connection_hook: self.connection_hook,
        }
    }

    /// Register an iroh relay, so peers that cannot reach each other directly
    /// still connect.
    ///
    /// p2panda registers no relay by default: the relay map is built purely
    /// from what is added here, and without one a peer is reachable only at a
    /// directly routable address that the other side already knows. mDNS
    /// supplies that on a shared link and nothing supplies it off one, which
    /// is why an unrelayed transport is a LAN-only transport.
    ///
    /// A relay carries connection metadata for whoever operates it. Which
    /// relay to trust is therefore an owner's decision, not a default worth
    /// baking in.
    pub fn relay_url(mut self, url: iroh::RelayUrl) -> Self {
        self.relay_urls.push(url);
        self
    }

    /// Enable mDNS discovery so peers on the same local network auto-populate
    /// the address book (no explicit `add_peer` needed on a LAN).
    /// `MdnsDiscoveryMode::Active` actively queries; `Passive` only responds.
    pub fn mdns(mut self, mode: MdnsDiscoveryMode) -> Self {
        self.mdns = Some(mode);
        self
    }

    /// Enable random-walk discovery for internet peers: walkers explore the
    /// network outward from the bootstrap nodes already in the address book
    /// (added via [`add_peer`](P2pandaTransport::add_peer) or surfaced by
    /// mDNS), resolving more peers' transport info over time. Uses the default
    /// [`DiscoveryConfig`] (2 walkers); see
    /// [`discovery_config`](Self::discovery_config) to tune it.
    pub fn discovery(mut self) -> Self {
        self.discovery = Some(DiscoveryConfig::default());
        self
    }

    /// Enable random-walk discovery with an explicit [`DiscoveryConfig`]
    /// (e.g. more walkers, a different reset-walk probability).
    pub fn discovery_config(mut self, config: DiscoveryConfig) -> Self {
        self.discovery = Some(config);
        self
    }

    /// Enable the gossip overlay, so [`subscribe`](P2pandaTransport::subscribe)
    /// can join space topics and broadcast/receive ephemeral messages (the
    /// live-sync path: peers subscribed to a space converge by publishing their
    /// operations to each other).
    pub fn gossip(mut self) -> Self {
        self.gossip = true;
        self
    }

    /// Bind the p2panda-net endpoint. Consumes the builder.
    pub async fn bind(self) -> Result<P2pandaTransport, TransportError> {
        P2pandaTransport::bind_inner(self).await
    }
}

/// p2panda-net-backed [`Transport`].
pub struct P2pandaTransport {
    /// Boxed, as are the overlay and discovery handles below: p2panda-net's
    /// endpoint handle carries the signing key by value (its
    /// `IrohEndpointArgs`), and those handles hold clones of it, so none is
    /// moved once made, and moving the transport moves pointers (ruling 51).
    endpoint: Box<Endpoint>,
    /// iroh's endpoint, asked of p2panda's actor once, at bind: its handle
    /// holds no key, and nothing after bind sends the actor a message for it
    /// (rulings 52, 54). ractor boxes such a message whole, unused bytes
    /// included, and those can carry key bytes bind left on the stack.
    /// The actor is spawned unsupervised and binds once, so this stays its
    /// endpoint for the transport's life.
    iroh: iroh::Endpoint,
    address_book: AddressBook,
    /// The store under `address_book`, for the one write its actor cannot
    /// make atomically: a record only if none exists.
    address_store: AddressStore,
    peer_id: PeerID,
    queues: AlpnQueues,
    /// mDNS discovery handle, held so the service keeps running while the
    /// transport is alive.
    _mdns: Option<Box<MdnsDiscovery>>,
    /// Random-walk discovery handle, held so the walkers keep running while the
    /// transport is alive (dropping it stops the discovery actor).
    _discovery: Option<Box<Discovery>>,
    /// Gossip overlay handle (the endpoint authority for space topics). `None`
    /// unless built with [`builder().gossip()`](P2pandaTransportBuilder::gossip);
    /// `subscribe`/`set_topics` use it.
    gossip: Option<Box<Gossip>>,
    /// Connections open per remote on this endpoint, from the handshake hook.
    open: OpenConnections,
}

impl P2pandaTransport {
    /// Start a builder for a new p2panda-net transport. The key is read once,
    /// here, into the builder's held seed.
    #[cfg(any(test, feature = "test-identity"))]
    pub fn builder(master: &Ed25519Keypair) -> P2pandaTransportBuilder<'static> {
        let mut seed = master.to_seed();
        let builder = Self::builder_from_seed_ref(&seed);
        seed.zeroize();
        builder
    }

    /// Start a transport from a custodied key: a derived persona key or an
    /// unassociated one (release log D3). The production constructor: a
    /// master cannot be passed here (vault lock plan, ruling 96), and the
    /// keypair builder is for tests only.
    pub fn builder_for(key: &impl CarrierKeypair) -> P2pandaTransportBuilder<'static> {
        let mut seed = key.keypair().to_seed();
        let builder = Self::builder_from_seed_ref(&seed);
        seed.zeroize();
        builder
    }

    /// Start a builder from borrowed Ed25519 seed material: the borrowing
    /// path (ruling 51). The seed is copied once into the builder's held
    /// seed, and no future frame ever holds it by value. External providers
    /// own the seed's identity scope.
    pub fn builder_from_seed_ref(signing_seed: &[u8; 32]) -> P2pandaTransportBuilder<'static> {
        P2pandaTransportBuilder {
            signing_seed: hold(signing_seed),
            alpns: Vec::new(),
            blobs: BlobServing::None,
            mdns: None,
            discovery: None,
            relay_urls: Vec::new(),
            gossip: false,
            #[cfg(test)]
            connection_hook: true,
        }
    }

    /// Start a builder from raw Ed25519 signing-key seed material.
    ///
    /// This is the identity-provider-neutral boundary for sibling apps. A
    /// Personae vault or another provider derives a protocol-scoped key and
    /// passes its 32-byte seed; transport never needs that provider's type.
    ///
    /// Kept for callers that pass the seed by value; it routes through
    /// [`Self::builder_from_seed_ref`] and clears its own copy. Prefer that.
    pub fn builder_from_seed(mut signing_seed: [u8; 32]) -> P2pandaTransportBuilder<'static> {
        let builder = Self::builder_from_seed_ref(&signing_seed);
        signing_seed.zeroize();
        builder
    }

    /// Bind with just the given Mere ALPNs (no discovery; explicit `add_peer`).
    /// The seed is taken before the future exists, so no frame holds it.
    #[cfg(any(test, feature = "test-identity"))]
    pub fn bind(
        master: &Ed25519Keypair,
        alpns: Vec<Alpn>,
    ) -> impl Future<Output = Result<Self, TransportError>> + 'static {
        Self::builder(master).alpns(alpns).bind()
    }

    /// Bind from raw protocol-scoped Ed25519 seed material, by value; kept for
    /// existing callers, routed through the held seed. Prefer
    /// [`Self::bind_seed_ref`].
    pub fn bind_seed(
        signing_seed: [u8; 32],
        alpns: Vec<Alpn>,
    ) -> impl Future<Output = Result<Self, TransportError>> + 'static {
        Self::builder_from_seed(signing_seed).alpns(alpns).bind()
    }

    /// Bind from borrowed protocol-scoped Ed25519 seed material.
    pub fn bind_seed_ref(
        signing_seed: &[u8; 32],
        alpns: Vec<Alpn>,
    ) -> impl Future<Output = Result<Self, TransportError>> + 'static {
        Self::builder_from_seed_ref(signing_seed)
            .alpns(alpns)
            .bind()
    }

    /// Bind with the given ALPNs and serve iroh-blobs against the provided store.
    #[cfg(any(test, feature = "test-identity"))]
    pub async fn bind_with_blobs(
        master: &Ed25519Keypair,
        alpns: Vec<Alpn>,
        blobs: Option<&BlobStore>,
    ) -> Result<Self, TransportError> {
        let builder = Self::builder(master).alpns(alpns);
        match blobs {
            Some(store) => builder.blobs(store).bind().await,
            None => builder.bind().await,
        }
    }

    /// Bind and serve blobs only to transport-authenticated admitted peers.
    #[cfg(any(test, feature = "test-identity"))]
    pub async fn bind_with_authorized_blobs(
        master: &Ed25519Keypair,
        alpns: Vec<Alpn>,
        blobs: &BlobStore,
        authorizer: BlobPeerAuthorizer,
    ) -> Result<Self, TransportError> {
        Self::builder(master)
            .alpns(alpns)
            .authorized_blobs(blobs, authorizer)
            .bind()
            .await
    }

    /// The one bind path. Takes the builder itself rather than restating its
    /// fields positionally, so adding a builder option cannot silently
    /// mis-order an existing one at any of the call sites.
    async fn bind_inner(builder: P2pandaTransportBuilder<'_>) -> Result<Self, TransportError> {
        let P2pandaTransportBuilder {
            signing_seed,
            alpns,
            blobs,
            mdns,
            discovery,
            gossip,
            relay_urls,
            #[cfg(test)]
            connection_hook,
        } = builder;
        // The key is made where it is handed over, never kept as a local
        // across an await; the held seed is cleared once p2panda has it.
        let peer_id = PeerID::from_bytes(
            SigningKey::from_bytes(&signing_seed)
                .verifying_key()
                .as_bytes(),
        )
        .map_err(|error| TransportError::Backend(format!("transport key: {error}")))?;
        // p2panda's own default here was an in-memory SQLite store; this is the
        // same lifetime over muniment instead, which keeps sqlx out of the graph.
        // A caller wanting the address book to survive a restart hands a durable
        // muniment backend in place of `MemoryBackend`.
        let address_store = AddressStore::new(MemoryBackend::new());
        let store = AddressBookStoreHandle::new(address_store.clone());
        let address_book = AddressBook::builder()
            .store(store)
            .spawn()
            .await
            .map_err(|e| TransportError::Backend(format!("address book: {e}")))?;
        // Always on (ruling 51): every connection on the endpoint, whichever
        // protocol opens it, is counted for `connected` off the overlay.
        let open = OpenConnections::default();
        // p2panda's builder and its spawn future carry the signing key by
        // value. They are built in a statement with no await and boxed, so
        // they live in their own allocation, never in this frame; the held
        // seed is cleared once p2panda has the key (ruling 51; the seed
        // residue test).
        let spawn = {
            let mut endpoint_builder = Endpoint::builder(address_book.clone())
                .signing_key(SigningKey::from_bytes(&signing_seed));
            #[cfg(not(test))]
            {
                endpoint_builder = endpoint_builder.hooks(open.clone());
            }
            #[cfg(test)]
            if connection_hook {
                endpoint_builder = endpoint_builder.hooks(open.clone());
            }
            for url in relay_urls {
                endpoint_builder = endpoint_builder.relay_url(url);
            }
            Box::pin(endpoint_builder.spawn())
        };
        drop(signing_seed);
        // p2panda-net's handle carries the key by value: boxed where it lands,
        // and lent from there, so no frame holds a moved copy.
        let endpoint = Box::new(
            spawn
                .await
                .map_err(|e| TransportError::Backend(format!("endpoint: {e}")))?,
        );
        let iroh = endpoint
            .endpoint()
            .await
            .map_err(|e| TransportError::Backend(format!("endpoint(): {e}")))?;
        let queues: AlpnQueues = Arc::new(StdMutex::new(HashMap::new()));
        for alpn in &alpns {
            let (tx, rx) = mpsc::unbounded_channel();
            endpoint
                .accept(
                    alpn.as_bytes(),
                    StreamQueueHandler {
                        tx,
                        #[cfg(feature = "noise")]
                        local_peer: peer_id,
                        #[cfg(feature = "noise")]
                        service_protocol: alpn.clone(),
                    },
                )
                .await
                .map_err(|e| TransportError::Backend(format!("accept register: {e}")))?;
            queues.lock().unwrap().insert(
                alpn.clone(),
                Arc::new(TokioMutex::new(AcceptReceiver::Unbounded(rx))),
            );
        }

        // One total match: each arm builds only the protocol it registers. The
        // earlier priority chain built the plain protocol up front and then
        // discarded it unused whenever a scoped authorizer was present.
        match blobs {
            BlobServing::None => {},
            BlobServing::Plain(store) => {
                endpoint
                    .accept(
                        iroh_blobs::ALPN,
                        iroh_blobs::BlobsProtocol::new(store.store(), None),
                    )
                    .await
                    .map_err(|e| TransportError::Backend(format!("blobs register: {e}")))?;
            },
            BlobServing::Authorized(store, authorizer) => {
                endpoint
                    .accept(
                        iroh_blobs::ALPN,
                        AuthorizedBlobsProtocol {
                            inner: iroh_blobs::BlobsProtocol::new(store.store(), None),
                            authorizer,
                        },
                    )
                    .await
                    .map_err(|e| {
                        TransportError::Backend(format!("authorized blobs register: {e}"))
                    })?;
            },
            BlobServing::Scoped(store, scope, authorizer) => {
                let source: SharedBlobSource = Arc::new(StoreBlobSource::new(store, authorizer));
                endpoint
                    .accept(iroh_blobs::ALPN, ScopedBlobsProtocol { source, scope })
                    .await
                    .map_err(|e| TransportError::Backend(format!("scoped blobs register: {e}")))?;
            },
            BlobServing::Source(source, scope) => {
                endpoint
                    .accept(iroh_blobs::ALPN, ScopedBlobsProtocol { source, scope })
                    .await
                    .map_err(|e| TransportError::Backend(format!("scoped blobs register: {e}")))?;
            },
        }

        // Optional LAN discovery: mDNS populates the address book so peers on
        // the same network are reachable without an explicit `add_peer`.
        // Each builder below takes a clone of that handle, key included: built
        // and boxed in a statement with no await, it is p2panda's allocation.
        let mdns_handle = match mdns {
            Some(mode) => {
                let spawn = Box::pin(
                    MdnsDiscovery::builder(address_book.clone(), (*endpoint).clone())
                        .mode(mode)
                        .spawn(),
                );
                Some(Box::new(spawn.await.map_err(|e| {
                    TransportError::Backend(format!("mdns: {e}"))
                })?))
            },
            None => None,
        };

        // Optional internet discovery: random walkers explore outward from the
        // bootstrap nodes already known to the address book, resolving more
        // peers' transport info over time. The handle is held so the walkers
        // keep running for the transport's life.
        let discovery_handle = match discovery {
            Some(config) => {
                let spawn = Box::pin(
                    Discovery::builder(address_book.clone(), (*endpoint).clone())
                        .config(config)
                        .spawn(),
                );
                Some(Box::new(spawn.await.map_err(|e| {
                    TransportError::Backend(format!("discovery: {e}"))
                })?))
            },
            None => None,
        };

        // Optional gossip overlay: ephemeral broadcast among peers subscribed to
        // the same space topic. Held so the overlay actor lives for the
        // transport; `subscribe`/`set_topics` drive it.
        let gossip_handle = if gossip {
            let spawn =
                Box::pin(Gossip::builder(address_book.clone(), (*endpoint).clone()).spawn());
            Some(Box::new(spawn.await.map_err(|e| {
                TransportError::Backend(format!("gossip: {e}"))
            })?))
        } else {
            None
        };

        Ok(Self {
            endpoint,
            iroh,
            address_book,
            address_store,
            peer_id,
            queues,
            _mdns: mdns_handle,
            _discovery: discovery_handle,
            gossip: gossip_handle,
            open,
        })
    }

    /// This node's dialable [`EndpointAddr`]: iroh's current candidates (the
    /// machine's real interface addresses, plus a relay when one is reached),
    /// with wildcard binds also rewritten to loopback so in-process peers —
    /// the test pattern — stay dialable even with no network up. Pass to a
    /// peer's [`add_peer`](Self::add_peer), or share as a
    /// [`ticket`](Self::ticket), so they can connect without discovery.
    pub async fn endpoint_addr(&self) -> Result<EndpointAddr, TransportError> {
        let iroh_ep = &self.iroh;
        // iroh discovers its direct (interface) addresses asynchronously just
        // after bind; give it a beat so the address carries the LAN
        // candidates a remote machine actually needs, not only the loopback
        // fallback below.
        let mut addr = iroh_ep.addr();
        for _ in 0..40 {
            if addr.ip_addrs().any(|a| !a.ip().is_loopback()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
            addr = iroh_ep.addr();
        }
        // Wildcard binds are not dialable as-is; add their loopback rewrite
        // so same-machine pairs connect even on an interface-less host.
        for sock in iroh_ep.bound_sockets() {
            let dial = if sock.ip().is_unspecified() {
                let ip = if sock.is_ipv4() {
                    IpAddr::V4(Ipv4Addr::LOCALHOST)
                } else {
                    IpAddr::V6(Ipv6Addr::LOCALHOST)
                };
                SocketAddr::new(ip, sock.port())
            } else {
                sock
            };
            addr = addr.with_ip_addr(dial);
        }
        Ok(addr)
    }

    /// Gracefully stop the underlying iroh endpoint.
    ///
    /// Call this when a bounded transport owner is finished. Dropping an open
    /// iroh endpoint aborts its remaining connections and makes the remote end
    /// report a lost connection even after all application data was delivered.
    ///
    /// Uses the handle kept at bind (ruling 52).
    pub async fn close(&self) -> Result<(), TransportError> {
        self.iroh.close().await;
        Ok(())
    }

    /// Register a peer's [`EndpointAddr`] so connect-by-PeerID resolves without
    /// discovery (DNS / mDNS / random-walk).
    pub async fn add_peer(&self, addr: EndpointAddr) -> Result<(), TransportError> {
        self.address_book
            .insert_node_info(NodeInfo::from(addr))
            .await
            .map_err(|e| TransportError::Backend(format!("insert_node_info: {e}")))?;
        Ok(())
    }

    /// This node's dialable address as a shareable **ticket** string. A peer
    /// pastes it into [`add_peer_ticket`](Self::add_peer_ticket) to dial this node
    /// without discovery — the string-friendly form of [`endpoint_addr`] for an
    /// out-of-band exchange (e.g. a host's "connect to peer" verb).
    ///
    /// [`endpoint_addr`]: Self::endpoint_addr
    pub async fn ticket(&self) -> Result<String, TransportError> {
        let addr = self.endpoint_addr().await?;
        Ok(EndpointTicket::from(addr).to_string())
    }

    /// Parse a peer's [`ticket`](Self::ticket), register its transport info
    /// ([`add_peer`](Self::add_peer)), and return its [`PeerID`] — so the caller
    /// can tag it ([`set_topics`](Self::set_topics)) to bootstrap an overlay.
    pub async fn add_peer_ticket(&self, ticket: &str) -> Result<PeerID, TransportError> {
        let ticket: EndpointTicket = ticket
            .trim()
            .parse()
            .map_err(|e| TransportError::Backend(format!("parse ticket: {e}")))?;
        let addr = EndpointAddr::from(ticket);
        let peer = PeerID::from_bytes(addr.id.as_bytes())
            .map_err(|e| TransportError::Backend(format!("ticket peer id: {e}")))?;
        self.add_peer(addr).await?;
        Ok(peer)
    }

    /// Join a space's gossip overlay (topic = e.g. a cabal / moot id) and return
    /// a [`GossipHandle`] to broadcast bytes to, and receive bytes from, peers
    /// subscribed to the same space. This is the **live-sync** path: peers
    /// converge by publishing their encoded operations here, and ingesting what
    /// they receive. Offline catch-up (RBSR) is `LogSync`, not yet wired.
    ///
    /// Requires [`builder().gossip()`](P2pandaTransportBuilder::gossip). Gossip
    /// bootstraps from address-book peers tagged with this topic (by discovery,
    /// or [`set_topics`](Self::set_topics) for explicit bootstrap), so
    /// tag/announce peers before subscribing.
    pub async fn subscribe(&self, topic: [u8; 32]) -> Result<GossipHandle, TransportError> {
        let gossip = self
            .gossip
            .as_ref()
            .ok_or_else(|| TransportError::Backend("gossip not enabled".to_string()))?;
        gossip
            .stream(Topic::from(topic))
            .await
            .map_err(|e| TransportError::Backend(format!("gossip subscribe: {e}")))
    }

    /// Peers the address book associates with `topic`, however they were
    /// learned: mDNS, a ticket, or an explicit [`add_peer`](Self::add_peer).
    ///
    /// The reporting half of [`set_topics`](Self::set_topics). A host that
    /// pairs on peer id needs to answer "which of my devices can I actually
    /// reach right now", and a peer id alone cannot answer that: the address
    /// book may hold the identity with no transport information yet.
    ///
    /// This node is excluded. The address book registers the local node
    /// against its own subscribed topics, so the raw query returns self as a
    /// reachable peer, which every caller would have to filter out.
    pub async fn peers_for_topic(&self, topic: [u8; 32]) -> Result<Vec<KnownPeer>, TransportError> {
        self.peers_for_topic_by(topic, true).await
    }

    /// [`peers_for_topic`](Self::peers_for_topic) with the rule switchable:
    /// `live` false lets iroh's path decide for every peer, so a test can show
    /// what the path says alone.
    async fn peers_for_topic_by(
        &self,
        topic: [u8; 32],
        live: bool,
    ) -> Result<Vec<KnownPeer>, TransportError> {
        let infos = self
            .address_book
            .node_infos_by_topics([Topic::from(topic)])
            .await
            .map_err(|e| TransportError::Backend(format!("node_infos_by_topics: {e}")))?;
        let local = self.peer_id.to_bytes();
        // The gossip manager tags this node with a topic when it subscribes and
        // untags it when it leaves, so the self record is the subscription.
        let subscribed = infos.iter().any(|info| info.node_id.as_bytes() == &local);
        let neighbours = if live && subscribed {
            self.gossip_neighbours(topic).await
        } else {
            HashSet::new()
        };
        let mut peers = Vec::new();
        for info in infos {
            if info.node_id.as_bytes() == &local {
                continue;
            }
            let peer = PeerID::from_bytes(info.node_id.as_bytes())
                .map_err(|e| TransportError::Backend(format!("peer id: {e}")))?;
            let connected = if live && subscribed {
                neighbours.contains(info.node_id.as_bytes())
            } else if live {
                self.open.count(info.node_id.as_bytes()) > 0
            } else {
                // Off the live rule iroh's path decides, read from the handle
                // kept at bind (ruling 54); a peer iroh does not know is not
                // connected.
                self.iroh
                    .remote_info(
                        iroh::PublicKey::from_bytes(info.node_id.as_bytes()).map_err(|e| {
                            TransportError::Backend(format!("peer key for remote_info: {e}"))
                        })?,
                    )
                    .await
                    .map(|remote| {
                        remote.addrs().any(|addr| {
                            matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active)
                        })
                    })
                    .unwrap_or(false)
            };
            peers.push(KnownPeer {
                peer,
                reachable: info.transports.is_some(),
                bootstrap: info.bootstrap,
                connected,
                on_overlay: subscribed,
            });
        }
        Ok(peers)
    }

    /// Direct gossip neighbours on `topic`'s overlay, from the gossip
    /// manager's own record of its `NeighbourUp` and `NeighbourDown` events,
    /// the record LogSync reads too.
    ///
    /// Empty without gossip, or when the overlay cannot be asked: read as
    /// nothing connected, the address-book half of
    /// [`peers_for_topic`](Self::peers_for_topic) still returned.
    async fn gossip_neighbours(&self, topic: [u8; 32]) -> HashSet<[u8; 32]> {
        let Some(gossip) = &self.gossip else {
            return HashSet::new();
        };
        gossip
            .events_with_neighbours(Topic::from(topic))
            .await
            .map(|(_, neighbours)| neighbours.iter().map(|node| *node.as_bytes()).collect())
            .unwrap_or_default()
    }

    /// Stop treating `peer` as a member of `topic`'s overlay: the inverse of
    /// [`set_topics`](Self::set_topics) for a single topic.
    ///
    /// The peer stays in the address book, so re-pairing later does not have
    /// to rediscover it. Only the overlay association goes, which is what
    /// unpairing means: the device is still a device, it is no longer on this
    /// graph.
    pub async fn remove_topic(&self, peer: PeerID, topic: [u8; 32]) -> Result<(), TransportError> {
        let node_id = VerifyingKey::from_bytes(&peer.to_bytes())
            .map_err(|e| TransportError::Backend(format!("peer key: {e}")))?;
        self.address_book
            .remove_topic(node_id, Topic::from(topic))
            .await
            .map_err(|e| TransportError::Backend(format!("remove_topic: {e}")))
    }

    /// Tag a known peer as interested in the given topics, so gossip can
    /// bootstrap its overlay to them. In production, discovery does this
    /// confidentially; this is the explicit-bootstrap path (pair with
    /// [`add_peer`](Self::add_peer), which supplies the peer's transport info).
    pub async fn set_topics(
        &self,
        peer: PeerID,
        topics: &[[u8; 32]],
    ) -> Result<(), TransportError> {
        let node_id = VerifyingKey::from_bytes(&peer.to_bytes())
            .map_err(|e| TransportError::Backend(format!("peer key: {e}")))?;
        self.ensure_record(node_id).await?;
        self.address_book
            .set_topics(node_id, topics.iter().map(|t| Topic::from(*t)))
            .await
            .map_err(|e| TransportError::Backend(format!("set_topics: {e}")))
    }

    /// Give a peer an address-book record before tagging it.
    ///
    /// A topic's members are the nodes holding both a record and the topic. A
    /// peer tagged by id alone is left out of gossip's bootstrap set, and the
    /// record mDNS writes later never re-joins it (pairing plan D1b). A bare
    /// record makes it a member now, and gossip's dial waits for its address.
    ///
    /// Written straight to the store, in one transaction, because the actor
    /// offers only an overwrite: an address discovered between a check and
    /// that overwrite would be lost.
    async fn ensure_record(&self, node_id: VerifyingKey) -> Result<(), TransportError> {
        self.address_store
            .insert_node_info_if_absent(NodeInfo::new(node_id))
            .await
            .map(|_| ())
            .map_err(|e| TransportError::Backend(format!("address record: {e}")))
    }

    /// Tag `peer` with additional overlay topics, keeping its existing set.
    ///
    /// The append form of [`set_topics`](Self::set_topics), which replaces.
    /// Two lanes sharing one endpoint (e.g. a personal graph and its carriage
    /// sibling) must use this for the second lane's tag, or each host clobbers
    /// the other's overlay and neither converges.
    pub async fn add_topics(
        &self,
        peer: PeerID,
        topics: &[[u8; 32]],
    ) -> Result<(), TransportError> {
        let node_id = VerifyingKey::from_bytes(&peer.to_bytes())
            .map_err(|e| TransportError::Backend(format!("peer key: {e}")))?;
        self.ensure_record(node_id).await?;
        for topic in topics {
            self.address_book
                .add_topic(node_id, Topic::from(*topic))
                .await
                .map_err(|e| TransportError::Backend(format!("add_topic: {e}")))?;
        }
        Ok(())
    }

    /// The endpoint + gossip handles a `LogSync` session needs, as owned clones.
    ///
    /// `None` if the transport was built without
    /// [`gossip`](P2pandaTransportBuilder::gossip) (LogSync rides the gossip
    /// overlay for peer-sampling). `murm` uses these to assemble a LogSync over a
    /// cabal's store for offline catch-up; pair with [`sync_overlay_topic`] +
    /// [`set_topics`](Self::set_topics) to bootstrap the overlay explicitly
    /// (discovery does this in production).
    pub fn sync_parts(&self) -> Option<(Endpoint, Gossip)> {
        self.gossip
            .as_ref()
            .map(|g| ((*self.endpoint).clone(), (**g).clone()))
    }

    /// The application-owned endpoint authority for mounting another Iroh
    /// protocol on this transport.
    ///
    /// This is available independently of gossip. A resident service such as
    /// Burn Remote registers its ALPN here so blobs, LogSync, and compute keep
    /// one transport identity and one endpoint lifetime.
    pub fn protocol_endpoint(&self) -> Endpoint {
        (*self.endpoint).clone()
    }

    /// The peer's current dialable address set as a ticket, if the endpoint
    /// holds any addresses for it. `None` when the peer is only a name.
    ///
    /// This is what makes an address survivable: the ticket a peer HANDS OUT
    /// describes itself as of its last bind, but the endpoint's own view of a
    /// peer it has talked to is fresher than any handed-out ticket, because
    /// every path change updates it. Persisting this after real contact is the
    /// cached-address rung of the resolver ladder: a device that has connected
    /// once can redial through the relay after both ends restart, with no
    /// discovery working at all.
    ///
    /// Addresses are sorted before serializing so the same address set always
    /// yields the same ticket string, letting a caller compare tickets to
    /// decide whether anything actually changed.
    pub async fn peer_ticket(&self, peer: PeerID) -> Result<Option<String>, TransportError> {
        let endpoint = &self.iroh;
        let id = iroh::PublicKey::from_bytes(&peer.to_bytes())
            .map_err(|e| TransportError::Backend(format!("peer key: {e}")))?;
        let Some(info) = endpoint.remote_info(id).await else {
            return Ok(None);
        };
        let mut addrs: Vec<_> = info.into_addrs().map(|addr| addr.into_addr()).collect();
        if addrs.is_empty() {
            return Ok(None);
        }
        addrs.sort_by_key(|addr| format!("{addr:?}"));
        let addr = EndpointAddr::from_parts(id, addrs);
        Ok(Some(EndpointTicket::from(addr).to_string()))
    }

    /// Every address the endpoint holds for `peer`, each marked active when it
    /// carries traffic now. Empty when the peer is only a name.
    ///
    /// The readable form of [`peer_ticket`](Self::peer_ticket)'s address set,
    /// plus the one fact a ticket drops: which address is the live path.
    ///
    /// iroh's path state alone, so a peer can be
    /// [`connected`](KnownPeer::connected) with no address active here, or not
    /// connected while one still is.
    pub async fn peer_paths(&self, peer: PeerID) -> Result<Vec<PeerPath>, TransportError> {
        let endpoint = &self.iroh;
        let id = iroh::PublicKey::from_bytes(&peer.to_bytes())
            .map_err(|e| TransportError::Backend(format!("peer key: {e}")))?;
        let Some(info) = endpoint.remote_info(id).await else {
            return Ok(Vec::new());
        };
        let mut paths: Vec<PeerPath> = info
            .addrs()
            .map(|addr| PeerPath {
                addr: peer_addr(addr.addr()),
                active: matches!(addr.usage(), iroh::endpoint::TransportAddrUsage::Active),
            })
            .collect();
        paths.sort_by(|a, b| a.addr.cmp(&b.addr));
        Ok(paths)
    }

    /// Open a raw iroh `Connection` to a peer for an arbitrary ALPN.
    ///
    /// Lower-level than [`Transport::connect`]; used by
    /// [`BlobStore::fetch_from`](crate::BlobStore::fetch_from) to drive the
    /// iroh-blobs fetch dance.
    pub async fn connect_raw(
        &self,
        peer: PeerID,
        alpn: &[u8],
    ) -> Result<Connection, TransportError> {
        let node_id = VerifyingKey::from_bytes(&peer.to_bytes())
            .map_err(|e| TransportError::Backend(format!("peer key: {e}")))?;
        self.endpoint
            .connect(node_id, alpn)
            .await
            .map_err(|e| TransportError::Backend(format!("connect: {e}")))
    }
}

impl Transport for P2pandaTransport {
    type Stream = P2pandaStream;

    fn local_peer_id(&self) -> PeerID {
        self.peer_id
    }

    async fn connect(&self, peer: PeerID, alpn: Alpn) -> Result<P2pandaStream, TransportError> {
        let conn = self.connect_raw(peer, alpn.as_bytes()).await?;
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| TransportError::Backend(format!("open_bi: {e}")))?;
        Ok(P2pandaStream {
            #[cfg(feature = "noise")]
            local_peer: self.peer_id,
            #[cfg(feature = "noise")]
            service_protocol: alpn,
            send,
            recv,
            _connection: conn,
            shutdown_ack: None,
            shutdown_finished: false,
            shutdown_complete: false,
        })
    }

    async fn accept(&self, alpn: Alpn) -> Result<AcceptedSession<P2pandaStream>, TransportError> {
        self.accept_observed(alpn).await.map(|(session, _)| session)
    }

    async fn connect_session(
        &self,
        peer: PeerID,
        alpn: Alpn,
    ) -> Result<crate::OutboundSession<P2pandaStream>, TransportError> {
        let stream = self.connect(peer, alpn.clone()).await?;
        Ok(crate::OutboundSession::new(
            stream,
            alpn,
            Some(self.peer_id),
            None,
        ))
    }
}

/// Mix value p2panda-net's sync manager uses to derive the gossip *overlay*
/// topic from a sync topic (replicated from its private `GOSSIP_TOPIC_MIX_VALUE`,
/// generated randomly upstream to avoid collisions between the membership overlay
/// and application gossip on the same topic).
const GOSSIP_TOPIC_MIX_VALUE: [u8; 32] = [
    253, 6, 251, 217, 173, 228, 215, 244, 130, 181, 150, 142, 220, 244, 49, 219, 35, 94, 163, 197,
    229, 93, 143, 227, 97, 61, 38, 202, 63, 250, 26, 233,
];

/// The gossip overlay topic a `LogSync` session actually joins for a given sync
/// topic: `BLAKE3(topic ++ mix)`.
///
/// p2panda-net's sync manager joins gossip on this *derived* topic, not the raw
/// sync topic, so explicit peer-tagging ([`P2pandaTransport::set_topics`]) must
/// use this value for the LogSync overlay to form (tagging the raw topic forms
/// no neighbour link). In production, discovery announces it; this is for
/// explicit / test bootstrap. The upstream constant is private, so it is
/// replicated here; see the redb-logstore probe.
pub fn sync_overlay_topic(sync_topic: [u8; 32]) -> [u8; 32] {
    let hash = p2panda_core::Hash::digest(
        [sync_topic.as_slice(), GOSSIP_TOPIC_MIX_VALUE.as_slice()].concat(),
    );
    *hash.as_bytes()
}

#[cfg(test)]
mod tests;
