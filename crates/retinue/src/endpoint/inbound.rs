//! Inbound link admission: the caps, the accept backlog, and the accept calls.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::hash::AddressHash;
use crate::link::Link;
use crate::packet::Packet;

use super::entropy::next_iv;
use super::interface::InterfaceId;
use super::resource_session::ResourceSession;
use super::runtime::{Endpoint, recv_until_closed};
use super::shared::Shared;
use super::stream::LinkStream;

/// Accepted links waiting for the application, per destination. A link request that would
/// overflow its destination's backlog is refused before any link state is created.
const ACCEPT_BACKLOG: usize = 64;

/// Recent accepted requests whose proof can be replayed idempotently when the initiator
/// retries after losing a proof.
pub(super) const LINK_REQUEST_CACHE: usize = 1_024;
pub(super) const LINK_REQUEST_CACHE_TTL: Duration = Duration::from_secs(30);

/// Caps on live inbound links, so a flood of requests cannot spawn tasks and buffers without
/// bound. At a cap, a new request displaces the oldest link not yet activated (nothing from
/// its initiator has decrypted), and is refused only when every counted link has activated.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InboundLinkLimits {
    /// Live inbound links across every destination this endpoint serves.
    pub total: usize,
    /// Live inbound links to any one destination.
    pub per_destination: usize,
}

impl Default for InboundLinkLimits {
    fn default() -> Self {
        Self {
            total: 256,
            per_destination: 64,
        }
    }
}

/// Live inbound links, counted against [`InboundLinkLimits`], and the per-destination
/// backlog of links queued for `accept`.
#[derive(Default)]
pub(super) struct InboundLinks {
    limits: InboundLinkLimits,
    /// Link id to its slot. Entries leave with the link (`remove_link`).
    pub(super) slots: HashMap<AddressHash, InboundSlot>,
    /// Admission order, so eviction can find the oldest.
    next_order: u64,
    /// Links handed to an accept queue and not yet taken, per destination.
    pub(super) backlog: HashMap<AddressHash, usize>,
}

pub(super) struct InboundSlot {
    destination: AddressHash,
    order: u64,
    /// Whether a packet from the initiator has decrypted on this link.
    active: bool,
}

/// What to do with a new inbound link request.
pub(super) enum Admission {
    Admit,
    /// Admit after evicting this link, which never activated.
    Evict(AddressHash),
    Refuse,
}

impl InboundLinks {
    pub(super) fn admission(&self, destination: AddressHash) -> Admission {
        if self.backlog.get(&destination).copied().unwrap_or(0) >= ACCEPT_BACKLOG {
            return Admission::Refuse;
        }
        let here = |slot: &InboundSlot| slot.destination == destination;
        let count = self.slots.values().filter(|slot| here(slot)).count();
        let destination_full = count >= self.limits.per_destination;
        if !destination_full && self.slots.len() < self.limits.total {
            return Admission::Admit;
        }
        let oldest_waiting = |same: bool| {
            self.slots
                .iter()
                .filter(|(_, slot)| !slot.active && (!same || here(slot)))
                .min_by_key(|(_, slot)| slot.order)
                .map(|(id, slot)| (*id, here(slot)))
        };
        // Prefer a stale request to the same destination, so a flood displaces its own.
        let victim =
            oldest_waiting(true).or_else(|| oldest_waiting(false).filter(|_| !destination_full));
        match victim {
            Some((id, same))
                if count - usize::from(same) < self.limits.per_destination
                    && self.slots.len() - 1 < self.limits.total =>
            {
                Admission::Evict(id)
            }
            _ => Admission::Refuse,
        }
    }

    pub(super) fn admit(&mut self, id: AddressHash, destination: AddressHash) {
        self.next_order += 1;
        self.slots.insert(
            id,
            InboundSlot {
                destination,
                order: self.next_order,
                active: false,
            },
        );
    }

    pub(super) fn queued(&mut self, destination: AddressHash) {
        *self.backlog.entry(destination).or_default() += 1;
    }

    fn taken(&mut self, destination: AddressHash) {
        if let Some(n) = self.backlog.get_mut(&destination) {
            *n -= 1;
            if *n == 0 {
                self.backlog.remove(&destination);
            }
        }
    }
}

/// An accepted inbound link and the destination it arrived on.
pub struct Accepted {
    /// The stream carrying the link.
    pub stream: LinkStream,
    /// The destination hash the link request targeted (an ALPN maps to one).
    pub destination: AddressHash,
    /// The interface the router actually received the link request on: a transport fact a
    /// policy layer can use to tell the local mesh from TCP.
    pub interface: InterfaceId,
}

/// An accepted resource link and the destination it arrived on.
pub struct AcceptedResource {
    /// The session that publishes or fetches one resource over the link.
    pub session: ResourceSession,
    /// The destination hash the link request targeted.
    pub destination: AddressHash,
    /// The interface the link request arrived on.
    pub interface: InterfaceId,
}

impl Shared {
    /// Mark an inbound link active once a packet on it decrypts under its keys: the
    /// initiator holds the keys, so it answered our proof (RNS activates on the RTT
    /// packet). Only a link still waiting for that pays the decrypt.
    pub(super) fn note_inbound_traffic(&self, link: &Link, pkt: &Packet) {
        let waiting = self
            .inbound
            .lock()
            .unwrap()
            .slots
            .get(&pkt.destination)
            .is_some_and(|slot| !slot.active);
        // Decrypt with the lock released; the router is the only writer of `active`.
        if waiting && link.decrypt(pkt).is_ok() {
            let mut inbound = self.inbound.lock().unwrap();
            if let Some(slot) = inbound.slots.get_mut(&pkt.destination) {
                slot.active = true;
            }
        }
    }

    /// Drop an inbound link that never activated, to make room for a newer request: close
    /// it toward the initiator and forget its cached proof.
    pub(super) fn evict_inbound(&self, id: AddressHash) {
        let entry = self
            .links
            .lock()
            .unwrap()
            .get(&id)
            .map(|e| (e.link.clone(), e.iface));
        if let Some((link, iface)) = entry {
            self.send_on(iface, link.close_packet(&next_iv()));
        }
        self.inbound_link_proofs.lock().unwrap().remove(&id);
        self.remove_link(id);
        self.routing_stats
            .inbound_links_evicted
            .fetch_add(1, Ordering::Relaxed);
    }
}

impl Endpoint {
    /// Cap live inbound links, in total and per destination. Links already up are kept;
    /// the caps govern new requests.
    pub fn set_inbound_link_limits(&self, limits: InboundLinkLimits) {
        self.shared.inbound.lock().unwrap().limits = limits;
    }

    /// The current inbound link caps.
    pub fn inbound_link_limits(&self) -> InboundLinkLimits {
        self.shared.inbound.lock().unwrap().limits
    }

    /// Wait for the next inbound link, surfaced as a stream.
    pub async fn accept(&self) -> io::Result<LinkStream> {
        Ok(self.accept_on_any().await?.stream)
    }

    /// Wait for the next inbound link, with the destination it targeted (an ALPN maps to a
    /// destination, so a host can dispatch by protocol).
    pub async fn accept_on_any(&self) -> io::Result<Accepted> {
        let accepted = recv_until_closed(&self.shared, &self.accepted_rx).await?;
        self.shared
            .inbound
            .lock()
            .unwrap()
            .taken(accepted.destination);
        Ok(accepted)
    }

    /// Wait for the next inbound **reliable** link (to a destination registered with
    /// [`register_reliable`](Self::register_reliable)) and return its stream. The driver
    /// learns the initiator's identity from the IDENTIFY it sends.
    pub async fn accept_reliable(&self) -> io::Result<LinkStream> {
        Ok(self.accept_reliable_on_any().await?.stream)
    }

    /// Wait for the next inbound reliable link, retaining the destination and
    /// physical interface on which its request arrived.
    pub async fn accept_reliable_on_any(&self) -> io::Result<Accepted> {
        let accepted = recv_until_closed(&self.shared, &self.reliable_accepted_rx).await?;
        self.shared
            .inbound
            .lock()
            .unwrap()
            .taken(accepted.destination);
        Ok(accepted)
    }

    /// Wait for an inbound resource link, including the destination it targeted.
    pub async fn accept_resource(&self) -> io::Result<AcceptedResource> {
        let accepted = recv_until_closed(&self.shared, &self.resource_accepted_rx).await?;
        self.shared
            .inbound
            .lock()
            .unwrap()
            .taken(accepted.destination);
        Ok(accepted)
    }
}
