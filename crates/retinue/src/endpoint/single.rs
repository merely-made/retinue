//! Link-less single packets: sending with receipts, delivery, and proofs.

use alloc::format;
use alloc::vec::Vec;

use std::io;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::sync::oneshot;

use crate::destination::DestinationName;
use crate::hash::{AddressHash, NameHash};
use crate::identity::Identity;
use crate::packet::{DestinationType, Packet, PacketType};

use super::interface::{InterfaceId, QueueAdmission};
use super::queue::TrafficClass;
use super::runtime::{Endpoint, endpoint_closed, recv_until_closed};
use super::shared::Shared;
use super::transit::make_room;

/// Outstanding single-packet receipts kept for proof matching (RNS `Transport.MAX_RECEIPTS`).
/// At capacity the oldest is culled.
#[cfg(not(test))]
pub(super) const SINGLE_RECEIPTS: usize = 1024;
#[cfg(test)]
pub(super) const SINGLE_RECEIPTS: usize = 4;

/// A receipt's allowance per hop, and for the first hop (RNS `DEFAULT_PER_HOP_TIMEOUT`).
const RECEIPT_TIMEOUT_PER_HOP: Duration = Duration::from_secs(6);

/// The hop count a receipt assumes when no path is known (RNS `PATHFINDER_M`).
const UNKNOWN_PATH_HOPS: u32 = 128;

/// One authenticated link-less asymmetric packet received by a registered destination.
#[derive(Clone, Debug)]
pub struct ReceivedSingle {
    pub destination: AddressHash,
    pub interface: InterfaceId,
    pub data: Vec<u8>,
    /// The retained receive ratchet that authenticated it. `None` means the long-term
    /// identity key authenticated the token: the destination has no ratchets, or does not
    /// enforce them and the sender knew none.
    pub ratchet_id: Option<NameHash>,
    packet_hash: [u8; 32],
}

impl ReceivedSingle {
    /// The full packet hash, which a delivery proof signs.
    pub fn packet_hash(&self) -> [u8; 32] {
        self.packet_hash
    }
}

/// When a registered destination proves the single packets it receives (RNS
/// `Destination.set_proof_strategy`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProofStrategy {
    /// Never prove. The default, as in RNS.
    #[default]
    None,
    /// The application decides per packet, by calling [`Endpoint::prove_single`].
    App,
    /// Prove every packet that decrypts.
    All,
}

/// How a single packet's delivery receipt concluded (RNS `PacketReceipt` status).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SingleDelivery {
    /// The destination proved receipt; `rtt` runs from queueing to the valid proof.
    Delivered { rtt: Duration },
    /// No valid proof arrived within the receipt's timeout. A destination that does not
    /// prove, the RNS default, always ends here.
    TimedOut,
    /// The receipt was dropped for newer ones before it concluded.
    Culled,
}

/// Evidence that a link-less packet was encrypted and accepted by local interface queues,
/// and the handle that learns whether the destination proved it.
#[derive(Debug)]
pub struct SinglePacketReceipt {
    pub destination: AddressHash,
    /// The advertised ratchet the packet was encrypted to. `None` means the destination
    /// advertised none, so its identity key was used.
    pub ratchet_id: Option<NameHash>,
    /// One for a learned route; possibly several when an expired route requires broadcast.
    pub queued_interfaces: usize,
    /// The full packet hash. The destination's proof signs it, and is addressed to its
    /// truncation.
    pub packet_hash: [u8; 32],
    /// How long [`Self::delivery`] waits for a proof: a first-hop allowance plus one per hop
    /// on the known path, or per hop of the protocol's ceiling when none is known.
    pub timeout: Duration,
    sent_at: tokio::time::Instant,
    proved: oneshot::Receiver<SingleDelivery>,
}

impl SinglePacketReceipt {
    /// Wait for the destination's proof, or the receipt's timeout.
    pub async fn delivery(self) -> SingleDelivery {
        match tokio::time::timeout_at(self.sent_at + self.timeout, self.proved).await {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(_)) | Err(_) => SingleDelivery::TimedOut,
        }
    }
}

/// An outbound single packet awaiting its proof.
pub(super) struct PendingReceipt {
    packet_hash: [u8; 32],
    identity: Identity,
    sent_at: tokio::time::Instant,
    deadline: tokio::time::Instant,
    proved: oneshot::Sender<SingleDelivery>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct SingleQueueResult {
    pub(super) queued: usize,
    frame_capable: bool,
    offline: bool,
    frame_limit_rejection: Option<(usize, usize)>,
}

impl SingleQueueResult {
    fn observe(&mut self, admission: QueueAdmission) {
        match admission {
            QueueAdmission::Queued => {
                self.queued += 1;
                self.frame_capable = true;
            }
            QueueAdmission::Full => self.frame_capable = true,
            QueueAdmission::Offline => self.offline = true,
            QueueAdmission::FrameLimit { actual, limit } => {
                if self
                    .frame_limit_rejection
                    .is_none_or(|(_, recorded_limit)| limit > recorded_limit)
                {
                    self.frame_limit_rejection = Some((actual, limit));
                }
            }
        }
    }
}

impl Shared {
    fn queue_single_on(&self, iface: InterfaceId, pkt: Packet) -> QueueAdmission {
        let addressed = self.address_for(iface, pkt);
        self.interfaces
            .lock()
            .unwrap()
            .iter()
            .find(|candidate| candidate.id == iface)
            .map_or(QueueAdmission::Full, |candidate| {
                candidate.push(addressed, TrafficClass::Interactive)
            })
    }

    /// Queue a local single packet on its learned route, or broadcast when the cached route
    /// has expired. Records whether a candidate queue could carry the complete encoded frame,
    /// so the caller can distinguish carrier refusal from temporary queue pressure.
    pub(super) fn queue_single(&self, dest: AddressHash, pkt: Packet) -> SingleQueueResult {
        let mut result = SingleQueueResult::default();
        if let Some(iface) = self.path_iface(dest) {
            result.observe(self.queue_single_on(iface, pkt));
            return result;
        }
        let interfaces: Vec<_> = self
            .interfaces
            .lock()
            .unwrap()
            .iter()
            .map(|interface| interface.id)
            .collect();
        for interface in interfaces {
            result.observe(self.queue_single_on(interface, pkt.clone()));
        }
        result
    }

    /// Prove a received single packet back out the interface it arrived on.
    fn send_single_proof(&self, iface: InterfaceId, packet_hash: &[u8; 32]) {
        let implicit = self.implicit_proofs.load(Ordering::Relaxed);
        let proof = crate::proof::proof_packet(&self.identity, packet_hash, implicit);
        self.send_on_class(iface, proof, TrafficClass::Control);
    }

    /// Conclude the receipt a valid proof answers. A proof that does not validate leaves the
    /// receipt in place, so a forgery cannot strand the genuine proof behind it.
    pub(super) fn conclude_single_receipt(&self, proof: &Packet) -> bool {
        let receipt = {
            let mut receipts = self.single_receipts.lock().unwrap();
            let valid = receipts.get(&proof.destination).is_some_and(|receipt| {
                crate::proof::validate(&proof.payload, &receipt.packet_hash, &receipt.identity)
            });
            if !valid {
                return false;
            }
            receipts.remove(&proof.destination)
        };
        if let Some(receipt) = receipt {
            let rtt = receipt.sent_at.elapsed();
            let _ = receipt.proved.send(SingleDelivery::Delivered { rtt });
        }
        true
    }
}

impl Endpoint {
    /// Set when a registered destination proves the single packets it receives.
    pub fn set_proof_strategy(
        &self,
        name: &DestinationName,
        strategy: ProofStrategy,
    ) -> io::Result<()> {
        self.with_registration(name, |registration| {
            registration.proof_strategy = strategy;
            Ok(())
        })
    }

    /// Refuse, or again accept, single packets a ratcheted destination receives encrypted to
    /// its identity key rather than one of its ratchets (RNS `Destination.enforce_ratchets`).
    /// Enforcement needs ratchets, so it is refused for a destination registered without.
    pub fn set_enforce_ratchets(&self, name: &DestinationName, enforce: bool) -> io::Result<()> {
        self.with_registration(name, |registration| {
            if enforce && registration.ratchets.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "destination has no ratchets to enforce",
                ));
            }
            registration.enforce_ratchets = enforce;
            Ok(())
        })
    }

    /// Whether this endpoint's proofs carry the signature alone (`true`, RNS's default) or
    /// the proved packet's hash as well. Senders accept both.
    pub fn set_implicit_proofs(&self, implicit: bool) {
        self.shared
            .implicit_proofs
            .store(implicit, Ordering::Relaxed);
    }

    /// Prove receipt of a single packet to its sender, for a destination whose strategy is
    /// [`ProofStrategy::App`] (or `All`, which already proved it). A destination that does
    /// not prove refuses.
    pub fn prove_single(&self, received: &ReceivedSingle) -> io::Result<()> {
        let strategy = self
            .shared
            .registered
            .lock()
            .unwrap()
            .iter()
            .find(|registration| registration.dest == received.destination)
            .map(|registration| registration.proof_strategy)
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "destination is not registered")
            })?;
        if strategy == ProofStrategy::None {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination does not prove",
            ));
        }
        self.shared
            .send_single_proof(received.interface, &received.packet_hash);
        Ok(())
    }

    /// Encrypt and queue one link-less packet to a destination: to its advertised ratchet, or
    /// to its identity key when it advertises none, as RNS does.
    ///
    /// Success means the packet was encrypted to the latest validated announce and accepted
    /// by at least one local interface queue; the receipt's
    /// [`delivery`](SinglePacketReceipt::delivery) learns whether the destination proved it.
    /// No queue accepting it fails with `WouldBlock`, or with `NotConnected` when the route's
    /// interfaces are all offline (a dialed hub reconnecting).
    pub fn send_single(&self, dest: AddressHash, data: &[u8]) -> io::Result<SinglePacketReceipt> {
        if !self.shared.is_running() {
            return Err(endpoint_closed());
        }
        if data.len() > crate::packet::ENCRYPTED_MDU {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "single-packet plaintext exceeds ENCRYPTED_MDU",
            ));
        }
        let sealed = super::sealing::seal(&self.shared, dest, data)?;
        let packet = Packet {
            ifac: false,
            header_type: crate::packet::HeaderType::Type1,
            context_flag: false,
            propagation: crate::packet::Propagation::Broadcast,
            destination_type: DestinationType::Single,
            packet_type: PacketType::Data,
            hops: 0,
            transport: None,
            destination: dest,
            context: 0,
            payload: sealed.token,
        };
        debug_assert!(packet.within_mtu());

        // Track the receipt before queueing, so a proof cannot outrun it (RNS `Packet.py`
        // 413-433: a first-hop allowance plus one per hop).
        let packet_hash = packet.full_hash();
        let proof_destination = crate::proof::truncated(&packet_hash);
        let hops = self
            .route_to(dest)
            .map_or(UNKNOWN_PATH_HOPS, |(_, hops)| u32::from(hops) + 1);
        let timeout = RECEIPT_TIMEOUT_PER_HOP * (1 + hops);
        let sent_at = tokio::time::Instant::now();
        let (proved_tx, proved) = oneshot::channel();
        {
            let mut receipts = self.shared.single_receipts.lock().unwrap();
            if let Some(culled) = make_room(
                &mut receipts,
                SINGLE_RECEIPTS,
                |receipt| receipt.deadline <= sent_at || receipt.proved.is_closed(),
                |receipt| receipt.sent_at,
            ) {
                let _ = culled.proved.send(SingleDelivery::Culled);
            }
            receipts.insert(
                proof_destination,
                PendingReceipt {
                    packet_hash,
                    identity: sealed.peer,
                    sent_at,
                    deadline: sent_at + timeout,
                    proved: proved_tx,
                },
            );
        }

        let queued = self.shared.queue_single(dest, packet);
        if queued.queued == 0 {
            self.shared
                .single_receipts
                .lock()
                .unwrap()
                .remove(&proof_destination);
            if !queued.frame_capable
                && let Some((actual, limit)) = queued.frame_limit_rejection
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "single packet is {actual} bytes after encryption, interface frame limit is {limit}"
                    ),
                ));
            }
            if !queued.frame_capable && queued.offline {
                return Err(io::Error::new(
                    io::ErrorKind::NotConnected,
                    "no interface on the route is online",
                ));
            }
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "no interface accepted the single packet",
            ));
        }
        Ok(SinglePacketReceipt {
            destination: dest,
            ratchet_id: sealed.ratchet_id,
            queued_interfaces: queued.queued,
            packet_hash,
            timeout,
            sent_at,
            proved,
        })
    }

    /// Wait for the next authenticated link-less single packet.
    pub async fn accept_single(&self) -> io::Result<ReceivedSingle> {
        recv_until_closed(&self.shared, &self.single_rx).await
    }
}

/// Decrypt and hand over a single packet whose full hash the packet filter already took.
pub(super) fn deliver_single(
    shared: &Arc<Shared>,
    iface: InterfaceId,
    pkt: &Packet,
    packet_hash: [u8; 32],
) {
    if !shared.is_running() {
        return;
    }
    let registration = shared
        .registered
        .lock()
        .unwrap()
        .iter()
        .find(|registration| registration.dest == pkt.destination)
        .map(|registration| {
            (
                registration.ratchets.clone(),
                registration.enforce_ratchets,
                registration.proof_strategy,
            )
        });
    let Some((ratchets, enforce_ratchets, proof_strategy)) = registration else {
        return;
    };

    let Ok((data, ratchet_id)) =
        super::sealing::open(shared, ratchets.as_deref(), enforce_ratchets, &pkt.payload)
    else {
        return;
    };
    let _ = shared.single_tx.send(ReceivedSingle {
        destination: pkt.destination,
        interface: iface,
        data,
        ratchet_id,
        packet_hash,
    });
    if proof_strategy == ProofStrategy::All {
        shared.send_single_proof(iface, &packet_hash);
    }
}
