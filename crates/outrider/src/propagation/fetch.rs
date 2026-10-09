//! The client side of a propagation fetch (`LXMRouter.py` 1571-1651).

use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceTransferConfig};
use retinue::hash::{AddressHash, NameHash, full_hash};
use retinue::identity::Identity;
use rmpv::Value;

use super::msgpack::{decode_entry_response, decode_id_response, encode_fetch_request};
use super::{
    DEFAULT_MAX_PROPAGATION_BATCH_BYTES, FETCH_LIMIT, PropagationAnnounce, PropagationError,
    PropagationMessage, propagation_destination,
};
use crate::announce::{delivery_destination, delivery_name, resolve_source};
use crate::codec::{DEFAULT_MAX_MESSAGE_BYTES, DecodedLxmf};
use crate::stamp::{MESSAGE_WORKBLOCK_ROUNDS, STAMP_LEN, valid_streamed};

/// How much one fetch asks for and what it leaves on the node.
#[derive(Clone, Debug)]
pub struct FetchPolicy {
    /// Most new messages requested; `usize::MAX` asks for all (`PR_ALL_MESSAGES`).
    pub max_messages: usize,
    /// The per-transfer budget sent to the node, in KB of 1000 bytes.
    pub transfer_limit_kb: u64,
    /// Leave messages on the node: report no held ids and send no final acknowledgement.
    pub retain_on_node: bool,
    /// The local delivery stamp cost to enforce, as for direct delivery.
    pub stamp_cost: Option<u8>,
    pub max_entry_bytes: usize,
    pub max_message_bytes: usize,
    pub resource: ResourceTransferConfig,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            max_messages: usize::MAX,
            transfer_limit_kb: FETCH_LIMIT,
            retain_on_node: false,
            stamp_cost: None,
            max_entry_bytes: DEFAULT_MAX_PROPAGATION_BATCH_BYTES,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            resource: ResourceTransferConfig::default(),
        }
    }
}

/// Whether a message's signature could be checked (LXMessage `signature_validated`).
// Identities travel by value throughout; boxing one here would only add an allocation.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verification {
    /// The source's announced identity verified the signature.
    Verified(Identity),
    /// No validated announce names the source, so the signature is unchecked
    /// (`SOURCE_UNKNOWN`). A path request for the source has been sent.
    SourceUnknown,
}

#[derive(Clone, Debug)]
pub struct FetchedPropagation {
    pub transient_id: [u8; 32],
    pub entry: PropagationMessage,
    pub message: DecodedLxmf,
    pub verification: Verification,
    /// The local ratchet that decrypted the message, `None` for the identity key.
    pub ratchet_id: Option<NameHash>,
}

/// A received entry that could not be accepted. It is still acknowledged, as stock does,
/// so it cannot occupy the node's queue forever.
#[derive(Debug)]
pub struct RejectedPropagation {
    pub transient_id: [u8; 32],
    pub error: PropagationError,
}

/// The final `[nil, haves]` request that deletes received messages from the node.
#[derive(Debug)]
pub enum Acknowledgement {
    /// Nothing was received, or the policy retains messages on the node.
    NotSent,
    Confirmed,
    /// The messages were received, but the node may serve them again.
    Failed(PropagationError),
}

#[derive(Debug)]
pub struct PropagationFetchReceipt {
    pub offered: Vec<[u8; 32]>,
    /// Offered ids already held locally, reported to the node for deletion.
    pub haves: Vec<[u8; 32]>,
    pub wants: Vec<[u8; 32]>,
    pub messages: Vec<FetchedPropagation>,
    pub rejected: Vec<RejectedPropagation>,
    pub acknowledgement: Acknowledgement,
}

/// Fetch messages for this endpoint's registered `lxmf.delivery` destination.
///
/// The node lists what it holds; offered ids that `held` reports are returned as haves, and
/// up to `max_messages` others are requested. Each received entry is opened on its own, and
/// all received entries are then acknowledged.
pub async fn fetch(
    endpoint: &Endpoint,
    node: &PeerAnnounce,
    request_time: f64,
    held: impl Fn(&[u8; 32]) -> bool,
    policy: &FetchPolicy,
) -> Result<PropagationFetchReceipt, PropagationError> {
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
    session.set_config(policy.resource);
    session.identify();
    let list = encode_fetch_request(request_time, Value::Array(vec![Value::Nil, Value::Nil]))?;
    let offered = decode_id_response(&session.request_raw(&list).await?.packed)?;
    let mut receipt = PropagationFetchReceipt {
        offered,
        haves: Vec::new(),
        wants: Vec::new(),
        messages: Vec::new(),
        rejected: Vec::new(),
        acknowledgement: Acknowledgement::NotSent,
    };
    if receipt.offered.is_empty() {
        return Ok(receipt);
    }
    for id in &receipt.offered {
        if held(id) {
            if !policy.retain_on_node {
                receipt.haves.push(*id);
            }
        } else if receipt.wants.len() < policy.max_messages {
            receipt.wants.push(*id);
        }
    }

    let get = encode_fetch_request(
        request_time,
        Value::Array(vec![
            id_list(&receipt.wants),
            id_list(&receipt.haves),
            Value::from(policy.transfer_limit_kb),
        ]),
    )?;
    let entries = decode_entry_response(&session.request_raw(&get).await?.packed)?;
    let local = delivery_destination(endpoint.identity());
    let mut received = Vec::with_capacity(entries.len());
    for bytes in entries {
        let transient_id = full_hash(&bytes);
        received.push(transient_id);
        match open(endpoint, local, &bytes, &receipt.wants, policy) {
            Ok(fetched) => receipt.messages.push(fetched),
            Err(error) => receipt.rejected.push(RejectedPropagation {
                transient_id,
                error,
            }),
        }
    }

    if !policy.retain_on_node && !received.is_empty() {
        let ack = encode_fetch_request(
            request_time,
            Value::Array(vec![Value::Nil, id_list(&received)]),
        )?;
        receipt.acknowledgement = match session.request_raw(&ack).await {
            Ok(response) => match decode_entry_response(&response.packed) {
                Ok(_) => Acknowledgement::Confirmed,
                Err(error) => Acknowledgement::Failed(error),
            },
            Err(error) => Acknowledgement::Failed(error.into()),
        };
    }
    Ok(receipt)
}

fn open(
    endpoint: &Endpoint,
    local: AddressHash,
    bytes: &[u8],
    wants: &[[u8; 32]],
    policy: &FetchPolicy,
) -> Result<FetchedPropagation, PropagationError> {
    let entry = PropagationMessage::decode(bytes, policy.max_entry_bytes)?;
    let transient_id = entry.transient_id();
    if !wants.contains(&transient_id) {
        return Err(PropagationError::UnexpectedTransientId);
    }
    if entry.destination != *local.as_bytes() {
        return Err(PropagationError::WrongDestination);
    }
    let (remainder, ratchet_id) = endpoint
        .decrypt_for(&delivery_name(), &entry.encrypted)
        .map_err(PropagationError::Decrypt)?;
    let message = entry.open(&remainder, policy.max_message_bytes)?;
    let source = AddressHash::from_bytes(message.source);
    let verification = match resolve_source(endpoint, source) {
        Some(identity)
            if delivery_destination(&identity) == source
                && message.verify_with(|bytes, signature| identity.verify(bytes, signature)) =>
        {
            Verification::Verified(identity)
        }
        Some(_) => return Err(PropagationError::BadSignature),
        None => Verification::SourceUnknown,
    };
    if let Some(cost) = policy.stamp_cost
        && !delivery_stamp_valid(&message, cost)
    {
        return Err(PropagationError::InvalidDeliveryStamp);
    }
    Ok(FetchedPropagation {
        transient_id,
        entry,
        message,
        verification,
        ratchet_id,
    })
}

/// Whether a message carries a delivery stamp worth `cost` on its message id
/// (`LXMRouter.py` 1930-1946).
pub fn delivery_stamp_valid(message: &DecodedLxmf, cost: u8) -> bool {
    message
        .payload
        .stamp
        .as_deref()
        .and_then(|stamp| <&[u8; STAMP_LEN]>::try_from(stamp).ok())
        .is_some_and(|stamp| {
            valid_streamed(
                &message.message_id,
                MESSAGE_WORKBLOCK_ROUNDS,
                stamp,
                u16::from(cost),
            )
        })
}

fn id_list(ids: &[[u8; 32]]) -> Value {
    Value::Array(ids.iter().map(|id| Value::Binary(id.to_vec())).collect())
}
