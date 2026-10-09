//! Peers heard, events surfaced, and send outcomes.

use outrider::{DeliveryAnnounce, LxmfPayload, delivery_destination};
use retinue::endpoint::PeerAnnounce;
use retinue::hash::AddressHash;

/// Someone heard announcing.
#[derive(Clone, Debug)]
pub struct Peer {
    /// The peer's delivery destination: the address a person is told and `/peers` lists.
    pub destination: AddressHash,
    /// The stamp cost it advertises, if any.
    pub stamp_cost: Option<u8>,
    /// The display name from its announce, if it carried one.
    pub name: Option<String>,
    /// The whole announce, because that is what sending takes.
    pub announce: PeerAnnounce,
}

impl Peer {
    pub(crate) fn from_announce(announce: PeerAnnounce) -> Self {
        let decoded = (announce.destination == delivery_destination(&announce.identity))
            .then(|| DeliveryAnnounce::decode(&announce.app_data).ok())
            .flatten();
        Self {
            destination: announce.destination,
            stamp_cost: decoded.as_ref().and_then(|delivery| delivery.stamp_cost),
            name: decoded
                .and_then(|delivery| delivery.display_name)
                .and_then(|bytes| String::from_utf8(bytes).ok()),
            announce,
        }
    }
}

/// Something that happened, for an application to render however it likes.
///
/// Kept flat despite `Peer`'s size: boxing would add a heap allocation per event to save a
/// few hundred bytes on a low-rate channel.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum Event {
    /// A peer was heard for the first time.
    PeerAppeared(Peer),
    /// An authenticated message arrived.
    Message {
        /// The authenticated LXMF object identity. Replaying this value is how
        /// an application suppresses the same object after reconnect or restart.
        message_id: [u8; 32],
        /// The sender's delivery destination, not its identity hash, so it matches the peer
        /// table.
        from: AddressHash,
        /// The public signing key proven by the LXMF signature and Retinue link.
        /// Applications may address this sender without silently adding it to a contact book.
        sender_identity: [u8; 32],
        /// Whether the authenticated object arrived inline or through a Resource transfer.
        mode: retinue::endpoint::PayloadMode,
        /// The complete authenticated LXMF payload. Applications that own a
        /// typed field, such as Signalman's field-7 voice clip, receive it
        /// without copying field bytes into the title or content body.
        payload: LxmfPayload,
    },
    /// Something arrived and was refused, most often from a sender never heard announcing.
    /// Surfaced so it is not mistaken for a dead radio.
    Dropped(String),
}

impl Event {
    /// Preserve every authenticated fact Outrider proved at the host boundary.
    /// A message whose sender did not verify becomes [`Event::Dropped`].
    pub fn authenticated_message(received: outrider::ReceivedDirect) -> Self {
        let Some(identity) = received.source_identity else {
            return Self::Dropped(format!(
                "message from {} is unverified",
                AddressHash::from_bytes(received.message.source)
            ));
        };
        Self::Message {
            message_id: received.message.message_id,
            from: delivery_destination(&identity),
            sender_identity: *identity.ed25519_bytes(),
            mode: received.mode,
            payload: received.message.payload,
        }
    }
}

/// What became of a send.
#[derive(Clone, Debug)]
pub enum Sent {
    /// Retinue accepted the object for carriage. This is not an end-recipient
    /// delivery receipt.
    HandedToRadio {
        message_id: [u8; 32],
        mode: retinue::endpoint::PayloadMode,
    },
    /// Nobody matching the prefix announced inside the wait.
    NoSuchPeer,
}

impl Sent {
    /// Report precisely the acceptance Outrider returned, without promoting it
    /// to an end-recipient delivery claim.
    pub fn handed_to_radio(receipt: outrider::DirectReceipt) -> Self {
        Self::HandedToRadio {
            message_id: receipt.message_id,
            mode: receipt.mode,
        }
    }
}
