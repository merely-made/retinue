//! Receipted link data: one data packet whose sender waits for the peer's proof, as an RNS
//! `PacketReceipt` on a link does (`Packet.py` 413-417, 444-475).

use alloc::vec::Vec;

use std::io;
use std::time::Duration;

use crate::hash::AddressHash;
use crate::identity::Identity;
use crate::link::Inbound;

use super::entropy::next_iv;
use super::facts::{LinkDirection, LinkRemoteFact};
use super::resource_session::{
    PayloadMode, ResourceSession, ResourceTransferConfig, register_resource_session,
};
use super::runtime::{Endpoint, endpoint_closed};
use super::stream::write_chunk_for_mtu;

/// A link receipt's allowance per round trip (RNS `Link.TRAFFIC_TIMEOUT_FACTOR`).
const TRAFFIC_TIMEOUT_FACTOR: u32 = 6;

/// The shortest a link receipt waits. RNS computes `max(rtt * 6, 5 ms)` but checks receipts
/// once a second (`Transport.py` 252, 740), so no stock sender gives up sooner than this.
const LINK_RECEIPT_FLOOR: Duration = Duration::from_secs(1);

/// How one receipted link data packet concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkDelivery {
    /// The peer proved the transfer. `elapsed` runs from sending to the valid proof: a
    /// round trip for a data packet, the whole transfer for a Resource.
    Proved { elapsed: Duration },
    /// The peer answered with data of its own rather than a proof, as an LXMF propagation
    /// node does to refuse a submission.
    Answered(Vec<u8>),
    /// No proof arrived before the receipt timed out or the link closed.
    Unproved,
}

/// What [`Endpoint::deliver_payload`] sent and how it concluded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PayloadReceipt {
    pub mode: PayloadMode,
    /// A Resource fails rather than concluding unproved, so it is always `Proved`.
    pub delivery: LinkDelivery,
}

impl ResourceSession {
    /// Send `data` as one link data packet and wait for the peer's proof of it, or the
    /// receipt's timeout: six round trips, and never under the one second RNS takes to
    /// notice a lapsed receipt.
    pub async fn send_proved(&mut self, data: &[u8]) -> io::Result<LinkDelivery> {
        if data.len() > write_chunk_for_mtu(self.link.mtu()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "link data exceeds one packet",
            ));
        }
        let packet = self.link.data_packet(data, &next_iv());
        let hash = packet.full_hash();
        let rtt = Duration::from_millis(self.timing().rtt);
        let sent_at = tokio::time::Instant::now();
        let deadline = sent_at + (rtt * TRAFFIC_TIMEOUT_FACTOR).max(LINK_RECEIPT_FLOOR);
        self.shared.send_on(self.iface, packet);
        loop {
            let packet = match tokio::time::timeout_at(deadline, self.packets.recv()).await {
                Ok(Some(packet)) => packet,
                Ok(None) | Err(_) => return Ok(LinkDelivery::Unproved),
            };
            if self.link.validate_proof(&packet) == Some(hash) {
                return Ok(LinkDelivery::Proved {
                    elapsed: sent_at.elapsed(),
                });
            }
            match self.link.receive(&packet) {
                Some(Inbound::Data(answer)) => return Ok(LinkDelivery::Answered(answer)),
                Some(Inbound::Close) => return Ok(LinkDelivery::Unproved),
                _ => {}
            }
        }
    }
}

impl Endpoint {
    /// Send one indivisible payload and learn whether the peer proved it.
    ///
    /// [`send_payload_with_config`](Self::send_payload_with_config) chooses the form the same
    /// way, but a data packet there is best effort. Here it is receipted, so the call returns
    /// once the peer proves it, answers it, or lets its receipt time out. A Resource returns
    /// on its proof, as it always has.
    pub async fn deliver_payload(
        &self,
        dest: AddressHash,
        peer: Identity,
        data: &[u8],
        config: ResourceTransferConfig,
    ) -> io::Result<PayloadReceipt> {
        let (link, iface, liveness) = self.establish(dest, peer).await?;
        let fits = data.len() <= write_chunk_for_mtu(link.mtu());
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
        if fits {
            let delivery = session.send_proved(data).await?;
            return Ok(PayloadReceipt {
                mode: PayloadMode::Data,
                delivery,
            });
        }
        let started = tokio::time::Instant::now();
        session.publish(data).await?;
        Ok(PayloadReceipt {
            mode: PayloadMode::Resource,
            delivery: LinkDelivery::Proved {
                elapsed: started.elapsed(),
            },
        })
    }
}
