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

use retinue::endpoint::{Endpoint, InterfaceId, PeerAnnounce, ReceivedSingle, SinglePacketReceipt};
use retinue::hash::{AddressHash, NameHash};
use retinue::identity::{Identity, PrivateIdentity};
use retinue::ratchet::RatchetStore;

use crate::announce::{AnnounceError, DeliveryAnnounce, delivery_destination, delivery_name};
use crate::codec::{
    CodecError, DEFAULT_MAX_MESSAGE_BYTES, DESTINATION_LEN, DecodedLxmf, LxmfPayload,
    decode_bounded, prepare,
};
use crate::stamp::{MESSAGE_WORKBLOCK_ROUNDS, STAMP_LEN, find_streamed, valid_streamed};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpportunisticReceipt {
    pub message_id: [u8; 32],
    pub ratchet_id: NameHash,
    pub queued_interfaces: usize,
    /// The complete signed LXMF object. The on-wire plaintext omits its first 16 bytes.
    pub packed: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedOpportunistic {
    pub message: DecodedLxmf,
    pub source_identity: Identity,
    pub interface: InterfaceId,
    pub ratchet_id: NameHash,
    /// The reconstructed complete signed LXMF object.
    pub packed: Vec<u8>,
}

/// Register `lxmf.delivery` for link, Resource, and ratcheted opportunistic delivery.
///
/// The caller owns and persists `ratchets`; call [`Endpoint::update_ratchets`] after rotating
/// it. Ordinary delivery re-announces through [`Endpoint::announce`], which automatically
/// includes the registered current ratchet.
pub fn register(
    endpoint: &Endpoint,
    announce: &DeliveryAnnounce,
    ratchets: &RatchetStore,
) -> Result<AddressHash, OpportunisticError> {
    let app_data = announce.encode()?;
    let name = delivery_name();
    let destination = name.destination_hash(endpoint.identity());
    endpoint.register_resource_with_ratchets(name, &app_data, ratchets)?;
    Ok(destination)
}

/// Send one signed LXMF object as a ratcheted link-less packet.
pub fn send(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
) -> Result<OpportunisticReceipt, OpportunisticError> {
    if sender.public() != endpoint.identity() {
        return Err(OpportunisticError::LocalIdentityMismatch);
    }
    if peer.destination != delivery_destination(&peer.identity) {
        return Err(OpportunisticError::WrongDestination);
    }
    let announce = DeliveryAnnounce::decode(&peer.app_data)?;
    let source = delivery_destination(sender.public());
    let prepared = prepare(*peer.destination.as_bytes(), *source.as_bytes(), payload)?;
    enforce_stamp(&announce, payload, prepared.message_id)?;
    finish_send(endpoint, sender, peer.destination, prepared)
}

/// Generate any stamp required by the peer, then send opportunistically.
pub fn send_stamped(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
    stamp_seed: [u8; STAMP_LEN],
    max_stamp_attempts: u64,
) -> Result<OpportunisticReceipt, OpportunisticError> {
    let announce = DeliveryAnnounce::decode(&peer.app_data)?;
    let Some(target) = announce.stamp_cost else {
        return send(endpoint, sender, peer, payload);
    };
    let source = delivery_destination(sender.public());
    let initial = prepare(*peer.destination.as_bytes(), *source.as_bytes(), payload)?;
    let (stamp, _) = find_streamed(
        &initial.message_id,
        MESSAGE_WORKBLOCK_ROUNDS,
        u16::from(target),
        stamp_seed,
        max_stamp_attempts,
    )
    .ok_or(OpportunisticError::StampBudgetExhausted)?;
    let mut stamped = payload.clone();
    stamped.stamp = Some(stamp.to_vec());
    send(endpoint, sender, peer, &stamped)
}

fn finish_send(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    destination: AddressHash,
    prepared: crate::codec::PreparedLxmf,
) -> Result<OpportunisticReceipt, OpportunisticError> {
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed = prepared.finish(signature);
    let single_payload = &packed[DESTINATION_LEN..];
    if single_payload.len() > retinue::packet::ENCRYPTED_MDU {
        return Err(OpportunisticError::TooLarge);
    }
    let SinglePacketReceipt {
        ratchet_id,
        queued_interfaces,
        ..
    } = endpoint.send_single(destination, single_payload)?;
    Ok(OpportunisticReceipt {
        message_id,
        ratchet_id,
        queued_interfaces,
        packed,
    })
}

/// Decode and authenticate one accepted opportunistic packet.
pub fn receive(
    endpoint: &Endpoint,
    received: ReceivedSingle,
    max_message_bytes: usize,
) -> Result<ReceivedOpportunistic, OpportunisticError> {
    receive_with_stamp_cost(endpoint, received, max_message_bytes, None)
}

/// Decode and authenticate one opportunistic packet, enforcing the local advertised cost.
pub fn receive_with_stamp_cost(
    endpoint: &Endpoint,
    received: ReceivedSingle,
    max_message_bytes: usize,
    stamp_cost: Option<u8>,
) -> Result<ReceivedOpportunistic, OpportunisticError> {
    let local_destination = delivery_destination(endpoint.identity());
    if received.destination != local_destination {
        return Err(OpportunisticError::WrongDestination);
    }
    let ratchet_id = received
        .ratchet_id
        .ok_or(OpportunisticError::UnratchetedPacket)?;
    let mut packed = Vec::with_capacity(DESTINATION_LEN + received.data.len());
    packed.extend_from_slice(received.destination.as_slice());
    packed.extend_from_slice(&received.data);

    let message = decode_bounded(&packed, max_message_bytes.min(DEFAULT_MAX_MESSAGE_BYTES))?;
    if message.destination != *local_destination.as_bytes() {
        return Err(OpportunisticError::WrongDestination);
    }
    let source = AddressHash::from_bytes(message.source);
    let source_identity = crate::announce::resolve_source(endpoint, source)
        .ok_or(OpportunisticError::UnknownSource(source))?;
    if source != delivery_destination(&source_identity) {
        return Err(OpportunisticError::WrongSource);
    }
    if !message.verify_with(|bytes, signature| source_identity.verify(bytes, signature)) {
        return Err(OpportunisticError::BadSignature);
    }
    enforce_received_stamp(&message, stamp_cost)?;

    Ok(ReceivedOpportunistic {
        message,
        source_identity,
        interface: received.interface,
        ratchet_id,
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

fn enforce_received_stamp(
    message: &DecodedLxmf,
    stamp_cost: Option<u8>,
) -> Result<(), OpportunisticError> {
    let Some(target) = stamp_cost else {
        return Ok(());
    };
    let stamp = message
        .payload
        .stamp
        .as_deref()
        .and_then(|stamp| <&[u8; STAMP_LEN]>::try_from(stamp).ok())
        .ok_or(OpportunisticError::StampRequired(target))?;
    if !valid_streamed(
        &message.message_id,
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
    #[error("the LXMF source is not that identity's lxmf.delivery destination")]
    WrongSource,
    #[error("the message source {0} has no validated delivery announce")]
    UnknownSource(AddressHash),
    #[error("the LXMF signature does not verify against the announced source identity")]
    BadSignature,
    #[error("an opportunistic packet arrived without a receive ratchet")]
    UnratchetedPacket,
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
