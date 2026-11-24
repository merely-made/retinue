// Copyright 2026 Mark Alan Boykin
// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.
// SPDX-License-Identifier: MPL-2.0

//! Catch-up sync over any carrier, set up through the shared security gate.
//!
//! A [`SyncLane`] is `murm-sync`'s carrier-neutral [`StreamSession`] behind the
//! same setup discipline as Murm's session lane: one reservation from the
//! lane's [`SetupGate`] before any untrusted byte, one deadline through
//! establishment and security, and a stream whose setup budgets last until the
//! owner admits the peer. The owner picks the profile by the method called:
//! native iroh security on p2panda, Noise on every other ordered carrier. A
//! profile that fails is a refusal; nothing retries a weaker one.
//!
//! The lane admits on what the channel proved: it hands `allow` the
//! authenticated peer and runs the session only if that returns true. The
//! session then ingests each operation through the caller's `accept` closure
//! (see `murm_sync::processor_accept`), so a peer admitted here still cannot
//! write an operation the domain policy refuses.
//!
//! Addresses for Noise over TCP are explicit input. [`NoiseListener`] is not
//! used here: it handshakes on its own, outside the gate.
//!
//! [`NoiseListener`]: crate::noise::NoiseListener

use std::net::SocketAddr;

use identity::Ed25519Keypair;
use murm_sync::{StreamSession, StreamSyncError, StreamSyncOutcome};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;

use crate::security::{
    PreparedStream, ServiceSecurityPolicy, SessionSecurityFacts, SetupError, SetupGate, SetupGuard,
    SetupLimits, SetupStats,
};
use crate::{
    AcceptedSession, Alpn, P2pandaStream, P2pandaTransport, PeerID, Transport, TransportError,
};

/// Why a lane session did not complete.
#[derive(Debug, thiserror::Error)]
pub enum SyncLaneError {
    /// The setup gate or the selected security profile refused the session.
    #[error(transparent)]
    Setup(#[from] SetupError),
    /// The carrier could not open the stream.
    #[error(transparent)]
    Carrier(#[from] TransportError),
    /// The owner's `allow` rule refused the authenticated peer.
    #[error("peer refused by the lane owner")]
    Refused,
    /// The admitted session failed.
    #[error(transparent)]
    Sync(#[from] StreamSyncError),
}

type Secured<S> = (PreparedStream<S>, SessionSecurityFacts, SetupGuard);

/// One sync service: its protocol, setup limits and shared setup gate.
///
/// Keep one per service and clone it into accept loops. Clones share the gate,
/// so sessions still in setup on every carrier meet one ceiling.
#[derive(Clone, Debug)]
pub struct SyncLane {
    protocol: Alpn,
    gate: SetupGate,
    limits: SetupLimits,
}

impl SyncLane {
    /// A lane for `protocol` with at most `max_pending` sessions in setup.
    pub fn new(protocol: Alpn, max_pending: usize, limits: SetupLimits) -> Self {
        Self {
            protocol,
            gate: SetupGate::new(max_pending),
            limits,
        }
    }

    /// Counter-only setup observations, shared by every clone.
    pub fn stats(&self) -> SetupStats {
        self.gate.stats()
    }

    fn noise_policy(&self, expected: Option<PeerID>) -> ServiceSecurityPolicy {
        ServiceSecurityPolicy::noise(self.protocol.clone(), expected, self.limits.clone())
    }

    /// Serve an accepted p2panda session through native iroh security.
    /// After the local session finishes, allow one lane timeout for the dialer
    /// to finish its stream and close this connection normally.
    pub async fn serve_native(
        &self,
        accepted: AcceptedSession<P2pandaStream>,
        allow: impl FnOnce(PeerID) -> bool,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError> {
        let policy =
            ServiceSecurityPolicy::native(self.protocol.clone(), None, self.limits.clone());
        let connection = accepted.stream.connection().clone();
        let secured = self.gate.reserve(&policy)?.native(accepted.into_stream())?;
        let mut stream = admit(secured, allow)?;
        let outcome = session.run(&mut stream).await?;
        // Acknowledgement of our FIN only completes the sending half. The
        // dialer can still be applying operations and closing its own sink.
        // Keep the receive half and connection alive until the dialer releases them.
        let close = tokio::time::timeout(self.limits.timeout, connection.closed())
            .await
            .map_err(|_| StreamSyncError::Deadline)?;
        match close {
            iroh::endpoint::ConnectionError::ApplicationClosed(reason)
                if reason.error_code.into_inner() == 0 =>
            {
                Ok(outcome)
            },
            other => Err(TransportError::Io(other.into()).into()),
        }
    }

    /// Serve an established stream on any ordered carrier through Noise,
    /// proving `key` as this end. Pass `accepted.into_stream()` from a
    /// [`Transport`], or a socket from a listener the caller owns.
    pub async fn serve_noise<S>(
        &self,
        key: &Ed25519Keypair,
        stream: S,
        allow: impl FnOnce(PeerID) -> bool,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let secured = self
            .gate
            .noise_responder(key, stream, &self.noise_policy(None))
            .await?;
        Ok(session.run(admit(secured, allow)?).await?)
    }

    /// Dial `server` over p2panda, require that endpoint, then sync.
    pub async fn dial_native(
        &self,
        carrier: &P2pandaTransport,
        server: PeerID,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError> {
        let policy =
            ServiceSecurityPolicy::native(self.protocol.clone(), Some(server), self.limits.clone());
        let guard = self.gate.reserve(&policy)?;
        let outbound = guard
            .run(carrier.connect_session(server, self.protocol.clone()))
            .await??;
        let secured = guard.native(outbound.stream)?;
        Ok(session.run(admit(secured, |_| true)?).await?)
    }

    /// Dial `server` on any [`Transport`], run Noise as `key`, require the
    /// responder to prove `expected`, then sync.
    ///
    /// `expected` comes from whoever named the peer, not from the carrier: on
    /// Reticulum the destination identity and the Noise key differ.
    pub async fn dial_noise<T: Transport>(
        &self,
        carrier: &T,
        server: PeerID,
        key: &Ed25519Keypair,
        expected: PeerID,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError> {
        self.dial_noise_any(carrier, server, key, Some(expected), session)
            .await
    }

    /// [`dial_noise`](Self::dial_noise) with the responder's Noise key optional.
    ///
    /// `None` encrypts and gates the stream without naming who answers. That is
    /// right when the bytes authenticate themselves, as a release log does
    /// against its key: a holder is any peer with a copy, and the reader has no
    /// key to expect.
    pub async fn dial_noise_any<T: Transport>(
        &self,
        carrier: &T,
        server: PeerID,
        key: &Ed25519Keypair,
        expected: Option<PeerID>,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError> {
        let guard = self.gate.reserve(&self.noise_policy(expected))?;
        let outbound = guard
            .run(carrier.connect_session(server, self.protocol.clone()))
            .await??;
        let secured = guard.noise_initiator(key, outbound.stream).await?;
        Ok(session.run(admit(secured, |_| true)?).await?)
    }

    /// Run Noise as `key` over a stream the caller opened (an I2P or onion
    /// stream, for instance), optionally require the responder to prove
    /// `expected`, then sync.
    pub async fn dial_noise_stream<S>(
        &self,
        stream: S,
        key: &Ed25519Keypair,
        expected: Option<PeerID>,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let guard = self.gate.reserve(&self.noise_policy(expected))?;
        let secured = guard.noise_initiator(key, stream).await?;
        Ok(session.run(admit(secured, |_| true)?).await?)
    }

    /// Dial a Noise-over-TCP peer at an address the caller supplies, require
    /// the responder to prove `expected`, then sync.
    pub async fn dial_noise_tcp(
        &self,
        addr: SocketAddr,
        key: &Ed25519Keypair,
        expected: PeerID,
        session: impl StreamSession,
    ) -> Result<StreamSyncOutcome, SyncLaneError> {
        let guard = self.gate.reserve(&self.noise_policy(Some(expected)))?;
        let tcp = guard
            .run(TcpStream::connect(addr))
            .await?
            .map_err(|error| TransportError::Backend(format!("connect: {error}")))?;
        let secured = guard.noise_initiator(key, tcp).await?;
        Ok(session.run(admit(secured, |_| true)?).await?)
    }
}

/// Apply the owner's rule and lift the setup limits. A refusal poisons the
/// stream and frees the lease.
fn admit<S>(
    (stream, facts, guard): Secured<S>,
    allow: impl FnOnce(PeerID) -> bool,
) -> Result<PreparedStream<S>, SyncLaneError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Some(peer) = facts.peer() else {
        guard.refused();
        return Err(SetupError::Security.into());
    };
    if !allow(peer) {
        guard.refused();
        return Err(SyncLaneError::Refused);
    }
    guard.admitted()?;
    Ok(stream)
}
