//! Opening links: setup and its retries, and the outbound session kinds.

use alloc::vec::Vec;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::hash::AddressHash;
use crate::identity::Identity;
use crate::link::{self, Link, LinkMode, LinkTrailer};
use crate::link_liveness::Liveness;
use crate::request::{Request, Response};

use super::entropy::{ephemeral_seed, next_iv};
use super::facts::{LinkDirection, LinkRemoteFact};
use super::interface::InterfaceId;
use super::reliable_driver::register_reliable_stream;
use super::resource_requests::ReceivedRawResponse;
use super::resource_session::{
    PayloadMode, ResourceSession, ResourceTransferConfig, register_resource_session,
};
use super::runtime::{Endpoint, endpoint_closed};
use super::shared::Shared;
use super::stream::{LinkStream, register_stream, write_chunk_for_mtu};

impl Endpoint {
    /// Open a best-effort link to a destination and return its stream. `peer` is the
    /// destination's identity (learned from an announce, e.g. via [`resolve`](Self::resolve)).
    pub async fn open(&self, dest: AddressHash, peer: Identity) -> io::Result<LinkStream> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        register_stream(
            &self.shared,
            link,
            iface,
            liveness,
            LinkDirection::Outbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(peer),
            },
        )
        .ok_or_else(endpoint_closed)
    }

    /// Open a **reliable** link to a destination — the Channel/Buffer path with proof acks,
    /// for lossy interfaces — and return its stream. `peer` is the destination's identity: the
    /// handshake authenticates it, and the peer's proofs of our packets are validated against
    /// it. As the initiator, the reliable driver IDENTIFYs us to the responder so it can
    /// validate our proofs in turn.
    pub async fn open_reliable(&self, dest: AddressHash, peer: Identity) -> io::Result<LinkStream> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        register_reliable_stream(
            &self.shared,
            link,
            iface,
            liveness,
            Some(peer),
            LinkDirection::Outbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(peer),
            },
        )
        .ok_or_else(endpoint_closed)
    }

    /// Open a link whose packets are driven by the resource transfer state machines.
    pub async fn open_resource(
        &self,
        dest: AddressHash,
        peer: Identity,
    ) -> io::Result<ResourceSession> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        register_resource_session(
            &self.shared,
            link,
            iface,
            liveness,
            LinkDirection::Outbound,
            LinkRemoteFact {
                destination: Some(dest),
                identity: Some(peer),
            },
        )
        .ok_or_else(endpoint_closed)
    }

    /// Open a resource link and publish one payload over it.
    pub async fn publish_resource(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
    ) -> io::Result<()> {
        self.publish_resource_with_config(dest, peer, data, ResourceTransferConfig::default())
            .await
    }

    /// Open and publish with explicit retry and total-time policy.
    pub async fn publish_resource_with_config(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
        config: ResourceTransferConfig,
    ) -> io::Result<()> {
        let mut session = self.open_resource(dest, peer).await?;
        session.set_config(config);
        session.publish(data).await
    }

    /// Send one indivisible payload using the form that fits the negotiated link.
    ///
    /// A payload that fits one encrypted data packet takes the low-overhead path.
    /// Larger payloads use a proved Resource on the same established link. This
    /// does not split a logical message into several independent data packets.
    pub async fn send_payload(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
    ) -> io::Result<PayloadMode> {
        self.send_payload_with_config(dest, peer, data, ResourceTransferConfig::default())
            .await
    }

    /// Send one indivisible payload, with explicit policy for the Resource
    /// path used when it does not fit one encrypted data packet.
    ///
    /// The policy is ignored for a payload that takes the Data path.
    pub async fn send_payload_with_config(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
        config: ResourceTransferConfig,
    ) -> io::Result<PayloadMode> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        if data.len() <= write_chunk_for_mtu(link.mtu()) {
            let mut stream = register_stream(
                &self.shared,
                link,
                iface,
                liveness,
                LinkDirection::Outbound,
                LinkRemoteFact {
                    destination: Some(dest),
                    identity: Some(peer),
                },
            )
            .ok_or_else(endpoint_closed)?;
            stream.write_all(data).await?;
            stream.shutdown().await?;
            drop(stream);
            Ok(PayloadMode::Data)
        } else {
            let mut session = register_resource_session(
                &self.shared,
                link,
                iface,
                liveness,
                LinkDirection::Outbound,
                LinkRemoteFact {
                    destination: Some(dest),
                    identity: Some(peer),
                },
            )
            .ok_or_else(endpoint_closed)?;
            session.set_config(config);
            session.publish(data).await?;
            Ok(PayloadMode::Resource)
        }
    }

    /// Open a link, send one request, and return its matching response.
    pub async fn request(
        &self,
        dest: AddressHash,
        peer: Identity,
        request: &Request,
    ) -> io::Result<Response> {
        let mut session = self.open_resource(dest, peer).await?;
        session.request(request).await
    }

    /// Open a link, send one already-packed request, and retain the raw
    /// matching response.
    pub async fn request_raw(
        &self,
        dest: AddressHash,
        peer: Identity,
        packed_request: &[u8],
    ) -> io::Result<ReceivedRawResponse> {
        let mut session = self.open_resource(dest, peer).await?;
        session.request_raw(packed_request).await
    }

    /// Open a resource link and fetch one payload published by the peer.
    pub async fn fetch_resource(&self, dest: AddressHash, peer: Identity) -> io::Result<Vec<u8>> {
        self.fetch_resource_with_config(dest, peer, ResourceTransferConfig::default())
            .await
    }

    /// Open and fetch with explicit retry and total-time policy.
    pub async fn fetch_resource_with_config(
        &self,
        dest: AddressHash,
        peer: Identity,
        config: ResourceTransferConfig,
    ) -> io::Result<Vec<u8>> {
        let mut session = self.open_resource(dest, peer).await?;
        session.set_config(config);
        session.fetch().await
    }

    /// How long a link request to `dest` waits for its proof: Node's deadline for the
    /// route's hop count plus the first-hop airtime of the interface it leaves by, or of the
    /// slowest interface when it is broadcast for want of a route.
    fn link_setup_timeout(&self, dest: AddressHash) -> Duration {
        let route = self.route_to(dest);
        let first_hop = {
            let airtime = self.shared.first_hop_airtime_ms.lock().unwrap();
            match route {
                Some((iface, _)) => airtime.get(&iface).copied().unwrap_or(0),
                None => airtime.values().copied().max().unwrap_or(0),
            }
        };
        let hops = route.map_or(0, |(_, hops)| hops);
        Duration::from_millis(crate::node::link_request_timeout(hops).saturating_add(first_hop))
    }

    /// Establish a link to `dest` (whose identity is `peer`), returning it with the interface
    /// its proof arrived on and its liveness timers. The stream discipline is chosen by the
    /// caller.
    ///
    /// Setup waits [`crate::node::link_request_timeout`] for the route's hop count, plus the
    /// outgoing interface's first-hop airtime ([`Endpoint::set_first_hop_airtime`]). On
    /// timeout an endpoint that is not a transport forgets the route and asks for a new path,
    /// within the path-request budget, as RNS does (`Transport.py` 697-725).
    async fn establish(
        &self,
        dest: AddressHash,
        peer: Identity,
    ) -> io::Result<(Link, InterfaceId, Liveness)> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        let ephemeral = ephemeral_seed();
        let link_mtu = self.shared.link_mtu.load(Ordering::Relaxed);
        let (pending, request) = link::PendingLink::open(
            dest,
            peer,
            &ephemeral,
            LinkTrailer {
                mode: LinkMode::Aes256Cbc,
                mtu: link_mtu,
            },
        );

        let link_id = pending.link_id();
        let (tx, rx) = oneshot::channel();
        self.shared.pending.lock().unwrap().insert(link_id, tx);
        // Stash the pending link so the router can prove it.
        self.shared
            .pending_links
            .lock()
            .unwrap()
            .insert(link_id, pending);
        // If setup does not complete — it times out below, or the caller drops this future —
        // remove both entries on the way out so a failed setup never leaks router state.
        let mut guard = PendingGuard {
            shared: Arc::clone(&self.shared),
            link_id,
            armed: true,
        };

        // Send the request toward the destination: on the interface the path table names
        // (addressed via its transport node if remote), or broadcast if we have no route yet
        // (a directly-connected peer).
        let send_request = || {
            match self.shared.path_iface(dest) {
                Some(iface) => self.shared.send_on(iface, request.clone()),
                None => self.shared.broadcast(request.clone()),
            }
            Instant::now()
        };
        let setup_timeout = self.link_setup_timeout(dest);
        // The proof's arrival measures the RTT from this first transmission. A proof after a
        // retry cannot say which copy it answers (Karn), so the measurement keeps this
        // conservative bound rather than guessing the shorter one.
        let sent_at = tokio::time::Instant::now();
        send_request();

        let retry = Duration::from_millis(self.shared.link_setup_retry_ms.load(Ordering::Relaxed));
        let mut retries = tokio::time::interval_at(tokio::time::Instant::now() + retry, retry);
        retries.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let deadline = tokio::time::sleep(setup_timeout);
        let closed = self.shared.closed_notify.notified();
        tokio::pin!(deadline);
        tokio::pin!(rx);
        tokio::pin!(closed);
        if self.shared.is_closed() {
            return Err(endpoint_closed());
        }

        loop {
            tokio::select! {
                result = &mut rx => match result {
                    Ok((link, iface)) => {
                        guard.armed = false; // router removed both entries on success
                        if self.shared.is_running() {
                            // The responder does not activate an inbound link until the
                            // initiator reports its measured RTT. Keep this ahead of any
                            // application packet emitted by the returned session.
                            let rtt = sent_at.elapsed();
                            self.shared
                                .send_on(iface, link.rtt_packet(rtt.as_secs_f32(), &next_iv()));
                            let rtt_ms = rtt.as_millis().min(u128::from(u64::MAX)) as u64;
                            let liveness =
                                Liveness::initiator(rtt_ms, self.shared.link_clock_ms());
                            return Ok((link, iface, liveness));
                        }
                        self.shared.send_on(iface, link.close_packet(&next_iv()));
                        return Err(endpoint_closed());
                    }
                    Err(_) => return Err(io::Error::new(
                        io::ErrorKind::ConnectionReset,
                        "link setup dropped",
                    )),
                },
                _ = retries.tick() => {
                    send_request();
                }
                _ = &mut deadline => {
                    if !self.shared.routing.lock().unwrap().forward_packets {
                        self.shared.forget_path(dest);
                        self.request_path(dest);
                    }
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "link setup timed out"));
                }
                _ = &mut closed => return Err(endpoint_closed()),
            }
        }
    }
}

/// Removes a link's pending-setup state — the `pending` waker and the `pending_links`
/// half-open link — if setup does not complete: a timeout, or the caller dropping the `open`
/// future. Without it, a setup that never receives its proof leaks both entries. Disarmed
/// once the proof establishes the link, since the router has already removed them.
struct PendingGuard {
    shared: Arc<Shared>,
    link_id: AddressHash,
    armed: bool,
}

impl Drop for PendingGuard {
    fn drop(&mut self) {
        if self.armed {
            self.shared.pending.lock().unwrap().remove(&self.link_id);
            self.shared
                .pending_links
                .lock()
                .unwrap()
                .remove(&self.link_id);
        }
    }
}
