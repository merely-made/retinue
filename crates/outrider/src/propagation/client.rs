//! Client lanes: prepare and submit entries.

use std::borrow::Cow;

use retinue::endpoint::{
    Endpoint, LinkDelivery, PayloadMode, PeerAnnounce, ResourceTransferConfig,
};
use retinue::hash::{AddressHash, NameHash};
use retinue::identity::PrivateIdentity;
use rmpv::Value;

use super::msgpack::ERROR_INVALID_STAMP;
use super::{
    PropagationAnnounce, PropagationBatch, PropagationEntry, PropagationError, PropagationMessage,
    propagation_destination,
};
use crate::announce::delivery_destination;
use crate::codec::{LxmfPayload, prepare};
use crate::stamp::{
    MESSAGE_WORKBLOCK_ROUNDS, PROPAGATION_WORKBLOCK_ROUNDS, STAMP_LEN, find_streamed,
};

#[derive(Clone, Debug)]
pub struct PreparedPropagation {
    pub message_id: [u8; 32],
    pub transient_id: [u8; 32],
    pub stamp_value: u16,
    /// The recipient ratchet the message was encrypted to, `None` for its identity key.
    pub ratchet_id: Option<NameHash>,
    pub packed_message: Vec<u8>,
    pub entry: PropagationEntry,
}

/// The proof of work a propagated message carries.
#[derive(Clone, Copy, Debug)]
pub struct PropagationStamps {
    /// The recipient's announced delivery cost. When set, a delivery stamp is minted on the
    /// message id and travels inside the encrypted message (`LXMRouter.py` 1822-1826).
    pub delivery_cost: Option<u8>,
    /// The node's propagation cost, minted on the transient id.
    pub propagation_cost: u16,
    /// Where both searches start. It need not be secret.
    pub seed: [u8; STAMP_LEN],
    /// The attempt budget of each search.
    pub max_attempts: u64,
}

/// Build, sign, stamp, and encrypt one message for an announced recipient, to its
/// advertised ratchet when it has one (`LXMessage.py` 434-436).
pub fn prepare_propagation(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    recipient: AddressHash,
    payload: &LxmfPayload,
    stamps: &PropagationStamps,
) -> Result<PreparedPropagation, PropagationError> {
    if sender.public() != endpoint.identity() {
        return Err(PropagationError::LocalIdentityMismatch);
    }
    prepare_propagation_with(sender, recipient, payload, stamps, |plaintext| {
        endpoint
            .encrypt_for(recipient, plaintext)
            .map_err(|_| PropagationError::UnknownRecipient(recipient))
    })
}

/// [`prepare_propagation`] with the encryption supplied by the caller, which returns the
/// token and the ratchet it used. This keeps receipts reproducible and lets a host without
/// an endpoint seal to a known identity.
pub fn prepare_propagation_with(
    sender: &PrivateIdentity,
    recipient: AddressHash,
    payload: &LxmfPayload,
    stamps: &PropagationStamps,
    seal: impl FnOnce(&[u8]) -> Result<(Vec<u8>, Option<NameHash>), PropagationError>,
) -> Result<PreparedPropagation, PropagationError> {
    let destination = *recipient.as_bytes();
    let source = *delivery_destination(sender.public()).as_bytes();
    let mut payload = Cow::Borrowed(payload);
    if let Some(cost) = stamps.delivery_cost {
        // The message id excludes the stamp, so mint on it before the stamped prepare.
        let message_id = prepare(destination, source, &payload)?.message_id;
        let (stamp, _) = find_streamed(
            &message_id,
            MESSAGE_WORKBLOCK_ROUNDS,
            u16::from(cost),
            stamps.seed,
            stamps.max_attempts,
        )
        .ok_or(PropagationError::StampBudgetExhausted)?;
        payload.to_mut().stamp = Some(stamp.to_vec());
    }
    let prepared = prepare(destination, source, &payload)?;
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed_message = prepared.finish(signature);
    let (encrypted, ratchet_id) = seal(&packed_message[16..])?;
    let message = PropagationMessage {
        destination,
        encrypted,
    };
    let transient_id = message.transient_id();
    let (stamp, stamp_value) = find_streamed(
        &transient_id,
        PROPAGATION_WORKBLOCK_ROUNDS,
        stamps.propagation_cost,
        stamps.seed,
        stamps.max_attempts,
    )
    .ok_or(PropagationError::StampBudgetExhausted)?;
    Ok(PreparedPropagation {
        message_id,
        transient_id,
        stamp_value,
        ratchet_id,
        packed_message,
        entry: PropagationEntry { message, stamp },
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropagationSubmitReceipt {
    pub transient_ids: Vec<[u8; 32]>,
    pub mode: PayloadMode,
    pub packed_batch: Vec<u8>,
    /// Whether the node proved the transfer, stock's SENT. A node proves a data packet only
    /// once every stamp validates, but RNS proves a Resource before the node reads it, so a
    /// proved Resource whose stamps fail is dropped without a word (`LXMRouter.py` 2321-2329,
    /// 2515-2523). `false` is a data packet whose proof did not arrive in time.
    pub proved: bool,
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
    let receipt = endpoint
        .deliver_payload(
            node.destination,
            node.identity,
            &packed_batch,
            resource_config,
        )
        .await?;
    let proved = match receipt.delivery {
        LinkDelivery::Proved { .. } => true,
        LinkDelivery::Answered(signal) if is_stamp_refusal(&signal) => {
            return Err(PropagationError::Rejected);
        }
        LinkDelivery::Answered(_) | LinkDelivery::Unproved => false,
    };
    Ok(PropagationSubmitReceipt {
        transient_ids,
        mode: receipt.mode,
        packed_batch,
        proved,
    })
}

/// Whether a node's answer is `[ERROR_INVALID_STAMP, ...]` (`LXMRouter.py` 2739-2749).
fn is_stamp_refusal(signal: &[u8]) -> bool {
    match rmpv::decode::read_value(&mut &signal[..]) {
        Ok(Value::Array(items)) => {
            items.first().and_then(Value::as_u64) == Some(ERROR_INVALID_STAMP.into())
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_stamp_refusal;

    #[test]
    fn only_an_invalid_stamp_signal_is_a_refusal() {
        // `msgpack.packb([0xf5])` as stock sends it, and with a trailing element.
        assert!(is_stamp_refusal(&[0x91, 0xcc, 0xf5]));
        assert!(is_stamp_refusal(&[0x92, 0xcc, 0xf5, 0xc0]));
        assert!(!is_stamp_refusal(&[0x91, 0xcc, 0xf6]));
        assert!(!is_stamp_refusal(&[0xcc, 0xf5]));
        assert!(!is_stamp_refusal(&[0x90]));
        assert!(!is_stamp_refusal(b""));
    }
}
