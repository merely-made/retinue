//! Client lanes: prepare and submit entries, and fetch from a node.

use retinue::endpoint::{Endpoint, PayloadMode, PeerAnnounce, ResourceTransferConfig};
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::{Identity, PrivateIdentity};
use retinue::token::{IV_LEN, encrypt_to_identity};
use rmpv::Value;

use super::msgpack::{decode_entry_response, decode_id_response, encode_fetch_request};
use super::{
    FETCH_LIMIT, PropagationAnnounce, PropagationBatch, PropagationEntry, PropagationError,
    PropagationMessage, propagation_destination,
};
use crate::announce::delivery_destination;
use crate::codec::{DecodedLxmf, LxmfPayload, prepare};
use crate::stamp::{PROPAGATION_WORKBLOCK_ROUNDS, STAMP_LEN, find_streamed};

#[derive(Clone, Debug)]
pub struct PreparedPropagation {
    pub message_id: [u8; 32],
    pub transient_id: [u8; 32],
    pub stamp_value: u16,
    pub packed_message: Vec<u8>,
    pub entry: PropagationEntry,
}

/// Build, sign, encrypt, and stamp one message for propagation submission.
///
/// `ephemeral_secret` and `iv` must be fresh and unpredictable. The stamp seed
/// need not be secret. Keeping all three explicit makes deterministic receipt
/// tests possible and leaves entropy policy with the runtime.
#[allow(clippy::too_many_arguments)]
pub fn prepare_propagation(
    sender: &PrivateIdentity,
    recipient: &Identity,
    payload: &LxmfPayload,
    ephemeral_secret: &[u8; 32],
    iv: &[u8; IV_LEN],
    stamp_seed: [u8; STAMP_LEN],
    target_cost: u16,
    max_stamp_attempts: u64,
) -> Result<PreparedPropagation, PropagationError> {
    let destination = delivery_destination(recipient);
    let source = delivery_destination(sender.public());
    let prepared = prepare(*destination.as_bytes(), *source.as_bytes(), payload)?;
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed_message = prepared.finish(signature);
    let encrypted = encrypt_to_identity(recipient, ephemeral_secret, iv, &packed_message[16..]);
    let mut transient_input = Vec::with_capacity(16 + encrypted.len());
    transient_input.extend_from_slice(destination.as_slice());
    transient_input.extend_from_slice(&encrypted);
    let transient_id = full_hash(&transient_input);
    let (stamp, stamp_value) = find_streamed(
        &transient_id,
        PROPAGATION_WORKBLOCK_ROUNDS,
        target_cost,
        stamp_seed,
        max_stamp_attempts,
    )
    .ok_or(PropagationError::StampBudgetExhausted)?;
    let entry = PropagationEntry {
        message: PropagationMessage {
            destination: *destination.as_bytes(),
            encrypted,
        },
        stamp,
    };
    Ok(PreparedPropagation {
        message_id,
        transient_id,
        stamp_value,
        packed_message,
        entry,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagationSubmitReceipt {
    pub transient_ids: Vec<[u8; 32]>,
    pub mode: PayloadMode,
    pub packed_batch: Vec<u8>,
}

pub async fn submit(
    endpoint: &Endpoint,
    node: &PeerAnnounce,
    batch: &PropagationBatch,
) -> Result<PropagationSubmitReceipt, PropagationError> {
    submit_with_resource_config(endpoint, node, batch, ResourceTransferConfig::default()).await
}

/// Submit a propagation batch with explicit timing and window policy for a
/// Resource-backed transfer. Small Data batches ignore the Resource policy.
pub async fn submit_with_resource_config(
    endpoint: &Endpoint,
    node: &PeerAnnounce,
    batch: &PropagationBatch,
    resource_config: ResourceTransferConfig,
) -> Result<PropagationSubmitReceipt, PropagationError> {
    if node.destination != propagation_destination(&node.identity) {
        return Err(PropagationError::WrongDestination);
    }
    // An inactive node is not refused: stock submits to whichever node it is pointed at, and
    // `announce.active` is the caller's to weigh.
    let announce = PropagationAnnounce::decode(&node.app_data)?;
    let target = u16::from(announce.costs.propagation);
    if batch
        .entries
        .iter()
        .any(|entry| !entry.validate_stamp(target))
    {
        return Err(PropagationError::InvalidStamp);
    }
    let packed_batch = batch.encode()?;
    if packed_batch.len() as u64 > announce.transfer_limit_bytes() {
        return Err(PropagationError::BatchTooLarge);
    }
    let transient_ids = batch
        .entries
        .iter()
        .map(PropagationEntry::transient_id)
        .collect();
    let mode = endpoint
        .send_payload_with_config(
            node.destination,
            node.identity,
            &packed_batch,
            resource_config,
        )
        .await?;
    Ok(PropagationSubmitReceipt {
        transient_ids,
        mode,
        packed_batch,
    })
}

#[derive(Clone, Debug)]
pub struct FetchedPropagation {
    pub transient_id: [u8; 32],
    pub entry: PropagationMessage,
    pub message: DecodedLxmf,
    pub source_identity: Identity,
}

#[derive(Clone, Debug)]
pub struct PropagationFetchReceipt {
    pub offered: Vec<[u8; 32]>,
    pub messages: Vec<FetchedPropagation>,
}

/// Fetch messages for this endpoint's delivery identity from a propagation
/// node.
///
/// The two-stage exchange first asks what is available, then requests a
/// bounded subset while reporting already handled transient ids.
#[allow(clippy::too_many_arguments)]
pub async fn fetch(
    endpoint: &Endpoint,
    recipient: &PrivateIdentity,
    node: &PeerAnnounce,
    handled: &[[u8; 32]],
    max_messages: usize,
    request_time: f64,
    max_entry_bytes: usize,
    max_message_bytes: usize,
) -> Result<PropagationFetchReceipt, PropagationError> {
    fetch_with_resource_config(
        endpoint,
        recipient,
        node,
        handled,
        max_messages,
        request_time,
        max_entry_bytes,
        max_message_bytes,
        ResourceTransferConfig::default(),
    )
    .await
}

/// Fetch with explicit timing and window policy for request or response
/// Resources. This is the carrier-policy seam for slow half-duplex links.
#[allow(clippy::too_many_arguments)]
pub async fn fetch_with_resource_config(
    endpoint: &Endpoint,
    recipient: &PrivateIdentity,
    node: &PeerAnnounce,
    handled: &[[u8; 32]],
    max_messages: usize,
    request_time: f64,
    max_entry_bytes: usize,
    max_message_bytes: usize,
    resource_config: ResourceTransferConfig,
) -> Result<PropagationFetchReceipt, PropagationError> {
    if recipient.public() != endpoint.identity() {
        return Err(PropagationError::LocalIdentityMismatch);
    }
    if node.destination != propagation_destination(&node.identity) {
        return Err(PropagationError::WrongDestination);
    }
    PropagationAnnounce::decode(&node.app_data)?;
    if !request_time.is_finite() {
        return Err(PropagationError::InvalidTransferTime);
    }

    let mut session = endpoint
        .open_resource(node.destination, node.identity)
        .await?;
    session.set_config(resource_config);
    session.identify();
    let offer_request =
        encode_fetch_request(request_time, Value::Array(vec![Value::Nil, Value::Nil]))?;
    let offer_response = session.request_raw(&offer_request).await?;
    let offered = decode_id_response(&offer_response.packed)?;
    let wanted: Vec<[u8; 32]> = offered.iter().copied().take(max_messages).collect();
    if wanted.is_empty() {
        return Ok(PropagationFetchReceipt {
            offered,
            messages: Vec::new(),
        });
    }

    let fetch_data = Value::Array(vec![
        Value::Array(wanted.iter().map(|id| Value::Binary(id.to_vec())).collect()),
        Value::Array(
            handled
                .iter()
                .map(|id| Value::Binary(id.to_vec()))
                .collect(),
        ),
        Value::from(FETCH_LIMIT),
    ]);
    let fetch_request = encode_fetch_request(request_time, fetch_data)?;
    let fetch_response = session.request_raw(&fetch_request).await?;
    let entries = decode_entry_response(&fetch_response.packed, max_entry_bytes)?;
    let mut messages = Vec::with_capacity(entries.len());
    for entry in entries {
        let transient_id = entry.transient_id();
        if !wanted.contains(&transient_id) {
            return Err(PropagationError::UnexpectedTransientId);
        }
        let message = entry.decrypt(recipient, max_message_bytes)?;
        let source_destination = AddressHash::from_bytes(message.source);
        let source_identity = crate::announce::resolve_source(endpoint, source_destination)
            .ok_or(PropagationError::UnknownSource(source_destination))?;
        if message.source != *delivery_destination(&source_identity).as_bytes()
            || !message.verify_with(|bytes, signature| source_identity.verify(bytes, signature))
        {
            return Err(PropagationError::BadSignature);
        }
        messages.push(FetchedPropagation {
            transient_id,
            entry,
            message,
            source_identity,
        });
    }
    Ok(PropagationFetchReceipt { offered, messages })
}
