//! LXMF opportunistic delivery over ratcheted Reticulum single packets.
//!
//! The Reticulum packet header already carries the 16-byte LXMF destination. Stock LXMF
//! therefore strips that field from the encrypted plaintext:
//!
//! ```text
//! source(16) || signature(64) || msgpack_payload
//! ```
//!
//! Prepending the packet destination reconstructs the ordinary signed LXMF object used by
//! direct and propagation delivery. Signature and message-id rules do not fork here.

use retinue::endpoint::{
    Endpoint, InterfaceId, PeerAnnounce, ProofStrategy, ReceivedSingle, SinglePacketReceipt,
};
use retinue::hash::{AddressHash, NameHash};
use retinue::identity::{Identity, PrivateIdentity};
use retinue::ratchet::RatchetStore;

use crate::announce::{AnnounceError, DeliveryAnnounce, delivery_destination, delivery_name};
use crate::codec::{
    CodecError, DEFAULT_MAX_MESSAGE_BYTES, DESTINATION_LEN, DecodedLxmf, LxmfPayload, PreparedLxmf,
    decode_bounded, prepare,
};
use crate::delivered::DeliveredCache;
use crate::inbound::{Verification, check_stamp, unix_now, verify};
use crate::stamp::{MESSAGE_WORKBLOCK_ROUNDS, STAMP_LEN, find_parallel, valid_streamed};
use crate::ticket::{StampFault, StampOutcome, TICKET_LEN, is_ticket_stamp};

#[derive(Debug)]
pub struct OpportunisticReceipt {
    pub message_id: [u8; 32],
    /// The complete signed LXMF object. The on-wire plaintext omits its first 16 bytes.
    pub packed: Vec<u8>,
    /// Retinue's receipt for the packet: the peer ratchet it was encrypted to (`None` when
    /// the peer advertised none and its identity key was used, as stock LXMF does), the
    /// queues that took it, and [`SinglePacketReceipt::delivery`], which resolves once the
    /// recipient proves it. Stock proves before it validates, so a proof, stock's DELIVERED,
    /// means received rather than accepted.
    pub packet: SinglePacketReceipt,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedOpportunistic {
    pub message: DecodedLxmf,
    /// [`Verification::Verified`] or [`Verification::SourceUnknown`]; an invalid signature is
    /// refused.
    pub verification: Verification,
    /// The sender's identity, when it verified.
    pub source_identity: Option<Identity>,
    /// The stamp's worth, when this destination asks for one.
    pub stamp: Option<StampOutcome>,
    pub interface: InterfaceId,
    /// The receive ratchet that decrypted the packet, or `None` for the identity key, which
    /// stock accepts unless ratchets are enforced (`Endpoint::set_enforce_ratchets`).
    pub ratchet_id: Option<NameHash>,
    /// The reconstructed complete signed LXMF object.
    pub packed: Vec<u8>,
}

/// Register `lxmf.delivery` for link, Resource, and ratcheted opportunistic delivery.
///
/// The endpoint takes ownership of `ratchets` (empty, or restored from a snapshot). Each
/// [`Endpoint::announce`] rotates it when due and carries the current ratchet; install
/// [`Endpoint::set_ratchet_persistence`] first to keep retained epochs across restarts.
/// [`receive`] proves each packet it verifies.
pub fn register(
    endpoint: &Endpoint,
    announce: &DeliveryAnnounce,
    ratchets: RatchetStore,
) -> Result<AddressHash, OpportunisticError> {
    let app_data = announce.encode()?;
    let name = delivery_name();
    let destination = name.destination_hash(endpoint.identity());
    endpoint.register_resource_with_ratchets(name.clone(), &app_data, ratchets)?;
    endpoint.set_proof_strategy(&name, ProofStrategy::App)?;
    Ok(destination)
}

/// Send one signed LXMF object as a link-less packet, ratcheted when the peer advertises one.
pub fn send(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
) -> Result<OpportunisticReceipt, OpportunisticError> {
    let (announce, prepared) = prepare_for(endpoint, sender, peer, payload)?;
    enforce_stamp(&announce, payload, prepared.message_id)?;
    finish_send(endpoint, sender, peer.destination, prepared)
}

/// Generate any stamp required by the peer, then send opportunistically.
///
/// The size is checked before minting, so a message that cannot fit one packet costs no
/// work, and the minted stamp is attached as found rather than checked again.
pub fn send_stamped(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
    stamp_seed: [u8; STAMP_LEN],
    max_stamp_attempts: u64,
) -> Result<OpportunisticReceipt, OpportunisticError> {
    let (announce, prepared) = prepare_for(endpoint, sender, peer, payload)?;
    // A payload already carrying a ticket stamp needs no proof of work.
    let ticketed = is_ticket_stamp(payload.stamp.as_deref());
    let Some(target) = announce.stamp_cost.filter(|_| !ticketed) else {
        return finish_send(endpoint, sender, peer.destination, prepared);
    };
    if prepared.stamped_len() - DESTINATION_LEN > retinue::packet::ENCRYPTED_MDU {
        return Err(OpportunisticError::TooLarge);
    }
    let (stamp, _) = find_parallel(
        &prepared.message_id,
        MESSAGE_WORKBLOCK_ROUNDS,
        u16::from(target),
        stamp_seed,
        max_stamp_attempts,
    )
    .ok_or(OpportunisticError::StampBudgetExhausted)?;
    finish_send(
        endpoint,
        sender,
        peer.destination,
        prepared.with_stamp(&stamp),
    )
}

/// Check the sender and peer, and prepare the message for the peer's delivery destination.
fn prepare_for(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
) -> Result<(DeliveryAnnounce, PreparedLxmf), OpportunisticError> {
    if sender.public() != endpoint.identity() {
        return Err(OpportunisticError::LocalIdentityMismatch);
    }
    if peer.destination != delivery_destination(&peer.identity) {
        return Err(OpportunisticError::WrongDestination);
    }
    let announce = DeliveryAnnounce::decode(&peer.app_data)?;
    let source = delivery_destination(sender.public());
    let prepared = prepare(*peer.destination.as_bytes(), *source.as_bytes(), payload)?;
    Ok((announce, prepared))
}

fn finish_send(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    destination: AddressHash,
    prepared: PreparedLxmf,
) -> Result<OpportunisticReceipt, OpportunisticError> {
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed = prepared.finish(signature);
    let single_payload = &packed[DESTINATION_LEN..];
    if single_payload.len() > retinue::packet::ENCRYPTED_MDU {
        return Err(OpportunisticError::TooLarge);
    }
    let packet = endpoint.send_single(destination, single_payload)?;
    Ok(OpportunisticReceipt {
        message_id,
        packed,
        packet,
    })
}

/// Decode and authenticate one accepted opportunistic packet.
pub fn receive(
    endpoint: &Endpoint,
    received: ReceivedSingle,
    delivered: &DeliveredCache,
    max_message_bytes: usize,
) -> Result<ReceivedOpportunistic, OpportunisticError> {
    receive_with_stamp_cost(endpoint, received, delivered, max_message_bytes, None)
}

/// Decode and authenticate one opportunistic packet, enforcing the local advertised cost.
///
/// A verified packet is proved, a duplicate included, so the sender stops retrying
/// (`LXMRouter.py` 1995-1996). One from an unknown source is returned unproved: the sender
/// retries while the path request [`resolve_source`](crate::resolve_source) sent fetches its
/// keys. A verified message already in `delivered` is refused as
/// [`Duplicate`](OpportunisticError::Duplicate).
pub fn receive_with_stamp_cost(
    endpoint: &Endpoint,
    received: ReceivedSingle,
    delivered: &DeliveredCache,
    max_message_bytes: usize,
    stamp_cost: Option<u8>,
) -> Result<ReceivedOpportunistic, OpportunisticError> {
    let no_tickets = |_: &AddressHash| Vec::new();
    receive_with_tickets(
        endpoint,
        received,
        delivered,
        max_message_bytes,
        stamp_cost,
        no_tickets,
    )
}

/// As [`receive_with_stamp_cost`], also accepting a stamp made with one of the tickets
/// `inbound_tickets` returns for the source, such as
/// [`TicketBook::inbound`](crate::TicketBook::inbound).
pub fn receive_with_tickets(
    endpoint: &Endpoint,
    received: ReceivedSingle,
    delivered: &DeliveredCache,
    max_message_bytes: usize,
    stamp_cost: Option<u8>,
    inbound_tickets: impl Fn(&AddressHash) -> Vec<[u8; TICKET_LEN]>,
) -> Result<ReceivedOpportunistic, OpportunisticError> {
    let local_destination = delivery_destination(endpoint.identity());
    if received.destination != local_destination {
        return Err(OpportunisticError::WrongDestination);
    }
    let mut packed = Vec::with_capacity(DESTINATION_LEN + received.data.len());
    packed.extend_from_slice(received.destination.as_slice());
    packed.extend_from_slice(&received.data);

    let message = decode_bounded(&packed, max_message_bytes.min(DEFAULT_MAX_MESSAGE_BYTES))?;
    if message.destination != *local_destination.as_bytes() {
        return Err(OpportunisticError::WrongDestination);
    }
    let source = AddressHash::from_bytes(message.source);
    let source_identity = crate::announce::resolve_source(endpoint, source);
    let verification = verify(&message, source_identity.as_ref());
    match verification {
        Verification::SignatureInvalid => return Err(OpportunisticError::BadSignature),
        Verification::Verified => endpoint.prove_single(&received)?,
        Verification::SourceUnknown => {}
    }
    let stamp =
        check_stamp(&message, stamp_cost, &inbound_tickets(&source)).map_err(
            |fault| match fault {
                StampFault::Missing => {
                    OpportunisticError::StampRequired(stamp_cost.unwrap_or_default())
                }
                StampFault::Invalid => OpportunisticError::InvalidStamp,
            },
        )?;
    if verification == Verification::Verified && !delivered.admit(message.message_id, unix_now()) {
        return Err(OpportunisticError::Duplicate(message.message_id));
    }

    Ok(ReceivedOpportunistic {
        message,
        verification,
        source_identity,
        stamp,
        interface: received.interface,
        ratchet_id: received.ratchet_id,
        packed,
    })
}

fn enforce_stamp(
    announce: &DeliveryAnnounce,
    payload: &LxmfPayload,
    message_id: [u8; 32],
) -> Result<(), OpportunisticError> {
    let Some(target) = announce.stamp_cost else {
        return Ok(());
    };
    // A ticket stamp only its receiver can check.
    if is_ticket_stamp(payload.stamp.as_deref()) {
        return Ok(());
    }
    let stamp = payload
        .stamp
        .as_deref()
        .and_then(|stamp| <&[u8; STAMP_LEN]>::try_from(stamp).ok())
        .ok_or(OpportunisticError::StampRequired(target))?;
    if !valid_streamed(
        &message_id,
        MESSAGE_WORKBLOCK_ROUNDS,
        stamp,
        u16::from(target),
    ) {
        return Err(OpportunisticError::InvalidStamp);
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum OpportunisticError {
    #[error("Retinue delivery failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Announce(#[from] AnnounceError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("the sender identity is not the endpoint identity")]
    LocalIdentityMismatch,
    #[error("the packet or announce is not for the expected lxmf.delivery destination")]
    WrongDestination,
    #[error("the LXMF signature does not verify against the announced source identity")]
    BadSignature,
    #[error("the message was already delivered here")]
    Duplicate([u8; 32]),
    #[error("the signed LXMF object does not fit one encrypted Reticulum packet")]
    TooLarge,
    #[error("the peer requires a delivery stamp with cost {0}")]
    StampRequired(u8),
    #[error("the delivery stamp is invalid")]
    InvalidStamp,
    #[error("the configured proof-of-work attempt budget was exhausted")]
    StampBudgetExhausted,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn array<const N: usize>(hex_value: &str) -> [u8; N] {
        hex::decode(hex_value).unwrap().try_into().unwrap()
    }

    #[test]
    fn stock_capture_is_the_full_codec_with_only_destination_elided() {
        let doc: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/lxmf_opportunistic.json"))
                .unwrap();
        let destination: [u8; DESTINATION_LEN] = array(doc["destination"].as_str().unwrap());
        let single = hex::decode(doc["single_payload"].as_str().unwrap()).unwrap();
        assert!(single.len() <= retinue::packet::ENCRYPTED_MDU);

        let mut packed = destination.to_vec();
        packed.extend_from_slice(&single);
        let decoded = crate::decode(&packed).unwrap();
        assert_eq!(decoded.source, array(doc["source"].as_str().unwrap()));
        assert_eq!(
            decoded.message_id,
            array(doc["message_id"].as_str().unwrap())
        );
        assert_eq!(
            decoded.payload.title,
            hex::decode(doc["title"].as_str().unwrap()).unwrap()
        );
        assert_eq!(
            decoded.payload.content,
            hex::decode(doc["content"].as_str().unwrap()).unwrap()
        );

        let sender = PrivateIdentity::from_secret_bytes(&[0x77; 64]);
        assert_eq!(
            delivery_destination(sender.public()).as_bytes(),
            &decoded.source
        );
        let payload = LxmfPayload::text(
            doc["timestamp"].as_f64().unwrap(),
            decoded.payload.title,
            decoded.payload.content,
        );
        let prepared = prepare(destination, decoded.source, &payload).unwrap();
        assert_eq!(prepared.message_id, decoded.message_id);
        let signature = sender.sign(prepared.signing_bytes());
        let rebuilt = prepared.finish(signature);
        assert_eq!(&rebuilt[DESTINATION_LEN..], single);
    }
}
