// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! I2P as a [`Transport`]: SAM streams carrying a [`noise`](crate::noise)
//! session, so a peer is a proven [`PeerID`] rather than an I2P destination.
//!
//! An I2P destination is a routing alias ([`crate::i2p`]); it never becomes an
//! identity here. Each stream runs the Noise `XX` handshake with this
//! transport's carrier key and declares its ALPN inside the session, so
//! `accept` reports the initiator Noise proved and `connect` refuses a
//! responder that is not the peer it dialled.
//!
//! [`NoiseListener`](crate::noise::NoiseListener) is not a `Transport` because
//! it owns no discovery. This one does, the way iroh's tickets do: a ticket
//! pairs a peer with its destination ([`I2pTransport::ticket`]), and
//! [`I2pTransport::add_peer_ticket`] learns it, so `connect` dials by `PeerID`.
//!
//! One task owns the session's accept (SAM asks that an accept is not
//! repeatedly cancelled and reissued) and hands each handshaken stream to the
//! queue for its ALPN. A handshake that stalls is dropped after
//! [`HANDSHAKE_TIMEOUT`], so one silent peer cannot hold the door.

use std::collections::HashMap;
use std::sync::{Arc, Mutex as StdMutex};
use std::time::Duration;

use identity::{CarrierKeypair, Ed25519Keypair};
use tokio::sync::{Mutex, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use zeroize::Zeroize;

use crate::i2p::{I2pAddress, I2pSession, I2pStream};
use crate::noise::{NoiseStream, secure_initiator_with_facts, secure_responder_with_facts};
use crate::{
    AcceptedSession, Alpn, IngressContext, OutboundSession, PeerID, Transport, TransportError,
};

/// The prefix of an I2P peer ticket.
pub const TICKET_PREFIX: &str = "mi2p1.";
/// How long an inbound stream may take to finish its Noise handshake.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);
/// How long `connect` keeps retrying a destination that is not yet reachable:
/// a newly published service takes a while to reach the dialler's router.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(240);
/// Inbound sessions held for an ALPN before more are refused.
const QUEUE: usize = 16;

type Inbound = AcceptedSession<NoiseStream<I2pStream>>;

fn backend(context: &str, error: impl std::fmt::Display) -> TransportError {
    TransportError::Backend(format!("i2p: {context}: {error}"))
}

/// A [`Transport`] over one SAM session.
pub struct I2pTransport {
    session: I2pSession,
    identity: Arc<Ed25519Keypair>,
    local: PeerID,
    peers: StdMutex<HashMap<PeerID, I2pAddress>>,
    inbound: HashMap<Alpn, Arc<Mutex<mpsc::Receiver<Inbound>>>>,
    connect_timeout: Duration,
    acceptor: Option<JoinHandle<()>>,
}

impl I2pTransport {
    /// Serve `alpns` over `session` as `key`. A session that does not publish
    /// only dials; `accept` then refuses every ALPN.
    pub fn new(session: I2pSession, key: &impl CarrierKeypair, alpns: Vec<Alpn>) -> Self {
        let mut seed = key.keypair().to_seed();
        let identity = Arc::new(Ed25519Keypair::from_seed(seed));
        seed.zeroize();
        let local = PeerID::from_public_key(identity.public_key());
        let mut inbound = HashMap::new();
        let mut routes = HashMap::new();
        if session.profile().publishes() {
            for alpn in alpns {
                let (send, receive) = mpsc::channel(QUEUE);
                routes.insert(alpn.clone(), send);
                inbound.insert(alpn, Arc::new(Mutex::new(receive)));
            }
        }
        let acceptor = (!routes.is_empty())
            .then(|| tokio::spawn(accept_loop(session.clone(), Arc::clone(&identity), routes)));
        Self {
            session,
            identity,
            local,
            peers: StdMutex::new(HashMap::new()),
            inbound,
            connect_timeout: CONNECT_TIMEOUT,
            acceptor,
        }
    }

    /// How long `connect` retries an unreachable destination.
    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    /// This peer and its destination, for a dialler's [`Self::add_peer_ticket`].
    ///
    /// # Errors
    ///
    /// The session does not publish, or has ended.
    pub fn ticket(&self) -> Result<String, TransportError> {
        let address = self
            .session
            .address()
            .map_err(|error| backend("address", error))?;
        Ok(format!(
            "{TICKET_PREFIX}{}.{}",
            to_hex(&self.local.to_bytes()),
            address.as_str()
        ))
    }

    /// Learn a peer's destination from its ticket, returning the peer.
    ///
    /// # Errors
    ///
    /// The ticket is not an I2P peer ticket.
    ///
    /// # Panics
    ///
    /// Never: the peer map's lock is only held for an insert or a lookup.
    pub fn add_peer_ticket(&self, ticket: &str) -> Result<PeerID, TransportError> {
        let (peer, address) = parse_ticket(ticket)?;
        self.peers
            .lock()
            .expect("the peer map is never poisoned")
            .insert(peer, address);
        Ok(peer)
    }

    async fn open(
        &self,
        peer: PeerID,
        alpn: &Alpn,
    ) -> Result<NoiseStream<I2pStream>, TransportError> {
        let address = self
            .peers
            .lock()
            .expect("the peer map is never poisoned")
            .get(&peer)
            .cloned()
            .ok_or_else(|| backend("connect", "no destination is known for this peer"))?;
        let stream = timeout(self.connect_timeout, async {
            loop {
                match self.session.connect(&address).await {
                    Ok(stream) => return Ok(stream),
                    Err(error) if !self.session.is_alive() => return Err(error),
                    Err(_) => sleep(Duration::from_secs(2)).await,
                }
            }
        })
        .await
        .map_err(|_| backend("connect", "the destination stayed unreachable"))?
        .map_err(|error| backend("connect", error))?;
        let (stream, facts) = timeout(
            HANDSHAKE_TIMEOUT,
            secure_initiator_with_facts(&self.identity, stream, alpn),
        )
        .await
        .map_err(|_| backend("handshake", "timed out"))??;
        if facts.peer() != peer {
            return Err(backend(
                "handshake",
                "the destination answered as another peer",
            ));
        }
        Ok(stream)
    }
}

impl Drop for I2pTransport {
    fn drop(&mut self) {
        if let Some(acceptor) = self.acceptor.take() {
            acceptor.abort();
        }
    }
}

/// Split a ticket into its peer and destination.
///
/// # Errors
///
/// The ticket is not an I2P peer ticket.
pub fn parse_ticket(ticket: &str) -> Result<(PeerID, I2pAddress), TransportError> {
    let invalid = || backend("ticket", "not an I2P peer ticket");
    let (peer, address) = ticket
        .strip_prefix(TICKET_PREFIX)
        .and_then(|rest| rest.split_once('.'))
        .ok_or_else(invalid)?;
    let bytes = from_hex(peer).ok_or_else(invalid)?;
    let peer = PeerID::from_bytes(&bytes).map_err(|_| invalid())?;
    let address = I2pAddress::parse(address).map_err(|_| invalid())?;
    Ok((peer, address))
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64
        || !text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

async fn accept_loop(
    session: I2pSession,
    identity: Arc<Ed25519Keypair>,
    routes: HashMap<Alpn, mpsc::Sender<Inbound>>,
) {
    let routes = Arc::new(routes);
    while let Ok(stream) = session.accept().await {
        let identity = Arc::clone(&identity);
        let routes = Arc::clone(&routes);
        tokio::spawn(async move {
            let Ok(Ok((stream, facts))) = timeout(
                HANDSHAKE_TIMEOUT,
                secure_responder_with_facts(&identity, stream),
            )
            .await
            else {
                return;
            };
            let alpn = facts.protocol().clone();
            if let Some(route) = routes.get(&alpn) {
                let _ = route.try_send(AcceptedSession::new(
                    stream,
                    alpn,
                    // Proven by the Noise identity proof, never by the destination.
                    Some(facts.peer()),
                    IngressContext::noise(),
                ));
            }
        });
    }
}

impl Transport for I2pTransport {
    type Stream = NoiseStream<I2pStream>;

    fn local_peer_id(&self) -> PeerID {
        self.local
    }

    async fn connect(&self, peer: PeerID, alpn: Alpn) -> Result<Self::Stream, TransportError> {
        self.open(peer, &alpn).await
    }

    async fn connect_session(
        &self,
        peer: PeerID,
        alpn: Alpn,
    ) -> Result<OutboundSession<Self::Stream>, TransportError> {
        let stream = self.open(peer, &alpn).await?;
        // The responder learns this side's key from the same proof.
        Ok(OutboundSession::new(stream, alpn, Some(self.local), None))
    }

    async fn accept(&self, alpn: Alpn) -> Result<Inbound, TransportError> {
        let queue = self
            .inbound
            .get(&alpn)
            .cloned()
            .ok_or(TransportError::AlpnNotRegistered)?;
        let mut receive = queue.lock().await;
        receive.recv().await.ok_or(TransportError::Closed)
    }
}

#[cfg(all(test, feature = "test-identity"))]
#[path = "i2p_transport_tests.rs"]
mod tests;
