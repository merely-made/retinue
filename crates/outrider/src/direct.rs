//! Direct LXMF delivery over Retinue links and Resources.

use retinue::endpoint::{
    AcceptedResource, Endpoint, InterfaceId, LinkDelivery, PayloadMode, PeerAnnounce,
    ReceivedPayload, ResourceTransferConfig,
};
use retinue::hash::AddressHash;
use retinue::identity::{Identity, PrivateIdentity};

use crate::announce::{AnnounceError, DeliveryAnnounce, delivery_destination, delivery_name};
use crate::codec::{
    CodecError, DEFAULT_MAX_MESSAGE_BYTES, DecodedLxmf, LxmfPayload, PreparedLxmf, decode_bounded,
    prepare,
};
use crate::stamp::{MESSAGE_WORKBLOCK_ROUNDS, STAMP_LEN, find_parallel, valid_streamed};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectReceipt {
    pub message_id: [u8; 32],
    pub mode: PayloadMode,
    /// The complete signed LXMF object handed to Retinue.
    pub packed: Vec<u8>,
    /// Whether the recipient proved the link packet or Resource, stock's DELIVERED. Stock
    /// proves before it validates, so this means received rather than accepted. `false` is
    /// a data packet whose proof did not arrive in time, which stock retries.
    pub delivered: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedDirect {
    pub message: DecodedLxmf,
    pub source_identity: Identity,
    pub mode: PayloadMode,
    pub interface: InterfaceId,
    /// The complete signed LXMF object received from Retinue.
    pub packed: Vec<u8>,
}

/// Register this endpoint's `lxmf.delivery` destination for both direct data
/// packets and Resource-backed messages.
pub fn register(
    endpoint: &Endpoint,
    announce: &DeliveryAnnounce,
) -> Result<AddressHash, DirectError> {
    let app_data = announce.encode()?;
    let name = delivery_name();
    let destination = name.destination_hash(endpoint.identity());
    endpoint.register_resource(name, &app_data);
    Ok(destination)
}

/// Re-announce an already registered delivery destination.
pub fn announce(endpoint: &Endpoint, announce: &DeliveryAnnounce) -> Result<(), DirectError> {
    endpoint.announce(&delivery_name(), &announce.encode()?);
    Ok(())
}

/// Send one signed LXMF object to a validated delivery announce.
pub async fn send(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
) -> Result<DirectReceipt, DirectError> {
    send_with_resource_config(
        endpoint,
        sender,
        peer,
        payload,
        ResourceTransferConfig::default(),
    )
    .await
}

/// Send one signed LXMF object with explicit timing and window policy for a
/// Resource-backed message. Small Data messages ignore the Resource policy.
pub async fn send_with_resource_config(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
    resource_config: ResourceTransferConfig,
) -> Result<DirectReceipt, DirectError> {
    let (announce, prepared) = prepare_for(endpoint, sender, peer, payload)?;
    if let Some(target) = announce.stamp_cost {
        let Some(stamp) = payload
            .stamp
            .as_deref()
            .and_then(|stamp| <&[u8; STAMP_LEN]>::try_from(stamp).ok())
        else {
            return Err(DirectError::StampRequired(target));
        };
        if !valid_streamed(
            &prepared.message_id,
            MESSAGE_WORKBLOCK_ROUNDS,
            stamp,
            u16::from(target),
        ) {
            return Err(DirectError::InvalidStamp);
        }
    }
    finish_send(endpoint, sender, peer, prepared, resource_config).await
}

/// Generate any stamp required by the peer's delivery announce, then send.
pub async fn send_stamped(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
    stamp_seed: [u8; STAMP_LEN],
    max_stamp_attempts: u64,
) -> Result<DirectReceipt, DirectError> {
    send_stamped_with_resource_config(
        endpoint,
        sender,
        peer,
        payload,
        stamp_seed,
        max_stamp_attempts,
        ResourceTransferConfig::default(),
    )
    .await
}

/// Generate any required stamp, then send with explicit policy for the
/// Resource path. The minted stamp is attached as found rather than checked again.
pub async fn send_stamped_with_resource_config(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
    stamp_seed: [u8; STAMP_LEN],
    max_stamp_attempts: u64,
    resource_config: ResourceTransferConfig,
) -> Result<DirectReceipt, DirectError> {
    let (announce, prepared) = prepare_for(endpoint, sender, peer, payload)?;
    let prepared = match announce.stamp_cost {
        Some(target) => {
            let (stamp, _) = find_parallel(
                &prepared.message_id,
                MESSAGE_WORKBLOCK_ROUNDS,
                u16::from(target),
                stamp_seed,
                max_stamp_attempts,
            )
            .ok_or(DirectError::StampBudgetExhausted)?;
            prepared.with_stamp(&stamp)
        }
        None => prepared,
    };
    finish_send(endpoint, sender, peer, prepared, resource_config).await
}

/// Check the sender and peer, and prepare the message for the peer's delivery destination.
fn prepare_for(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    payload: &LxmfPayload,
) -> Result<(DeliveryAnnounce, PreparedLxmf), DirectError> {
    if sender.public() != endpoint.identity() {
        return Err(DirectError::LocalIdentityMismatch);
    }
    if peer.destination != delivery_destination(&peer.identity) {
        return Err(DirectError::WrongDestination);
    }
    let announce = DeliveryAnnounce::decode(&peer.app_data)?;
    let source = delivery_destination(sender.public());
    let prepared = prepare(*peer.destination.as_bytes(), *source.as_bytes(), payload)?;
    Ok((announce, prepared))
}

/// Sign and deliver, waiting for the recipient's proof (`LXMessage.py` 481-497).
async fn finish_send(
    endpoint: &Endpoint,
    sender: &PrivateIdentity,
    peer: &PeerAnnounce,
    prepared: PreparedLxmf,
    resource_config: ResourceTransferConfig,
) -> Result<DirectReceipt, DirectError> {
    let message_id = prepared.message_id;
    let signature = sender.sign(prepared.signing_bytes());
    let packed = prepared.finish(signature);
    let receipt = endpoint
        .deliver_payload(peer.destination, peer.identity, &packed, resource_config)
        .await?;
    Ok(DirectReceipt {
        message_id,
        mode: receipt.mode,
        packed,
        delivered: matches!(receipt.delivery, LinkDelivery::Proved { .. }),
    })
}

/// Decode and authenticate one accepted direct-delivery link.
///
/// The caller remains the endpoint's accept dispatcher. This function refuses
/// a session for any destination other than this endpoint's `lxmf.delivery`
/// destination, rather than consuming unrelated Resource protocols.
pub async fn receive(
    endpoint: &Endpoint,
    accepted: AcceptedResource,
    max_message_bytes: usize,
) -> Result<ReceivedDirect, DirectError> {
    receive_with_stamp_cost_and_resource_config(
        endpoint,
        accepted,
        max_message_bytes,
        None,
        ResourceTransferConfig::default(),
    )
    .await
}

/// Decode and authenticate direct delivery with explicit policy for receiving
/// a Resource-backed message.
pub async fn receive_with_resource_config(
    endpoint: &Endpoint,
    accepted: AcceptedResource,
    max_message_bytes: usize,
    resource_config: ResourceTransferConfig,
) -> Result<ReceivedDirect, DirectError> {
    receive_with_stamp_cost_and_resource_config(
        endpoint,
        accepted,
        max_message_bytes,
        None,
        resource_config,
    )
    .await
}

/// Receive direct delivery and enforce this destination's announced stamp
/// cost.
pub async fn receive_with_stamp_cost(
    endpoint: &Endpoint,
    accepted: AcceptedResource,
    max_message_bytes: usize,
    stamp_cost: Option<u8>,
) -> Result<ReceivedDirect, DirectError> {
    receive_with_stamp_cost_and_resource_config(
        endpoint,
        accepted,
        max_message_bytes,
        stamp_cost,
        ResourceTransferConfig::default(),
    )
    .await
}

/// Decode and authenticate direct delivery, enforcing its announced stamp
/// cost and applying explicit policy if the message arrives as a Resource.
pub async fn receive_with_stamp_cost_and_resource_config(
    endpoint: &Endpoint,
    mut accepted: AcceptedResource,
    max_message_bytes: usize,
    stamp_cost: Option<u8>,
    resource_config: ResourceTransferConfig,
) -> Result<ReceivedDirect, DirectError> {
    let local_destination = delivery_destination(endpoint.identity());
    if accepted.destination != local_destination {
        return Err(DirectError::WrongDestination);
    }
    let interface = accepted.interface;
    accepted.session.set_config(resource_config);
    let (mode, packed) = match accepted.session.receive().await? {
        ReceivedPayload::Data(bytes) => (PayloadMode::Data, bytes),
        ReceivedPayload::Resource(bytes) => (PayloadMode::Resource, bytes),
    };
    let message = decode_bounded(&packed, max_message_bytes.min(DEFAULT_MAX_MESSAGE_BYTES))?;
    if message.destination != *local_destination.as_bytes() {
        return Err(DirectError::WrongDestination);
    }
    let source = AddressHash::from_bytes(message.source);
    let identified = accepted.session.identified_peer();
    let source_identity = crate::announce::resolve_source_with_link(endpoint, source, identified)
        .ok_or(DirectError::UnknownSource {
        address: source,
        identified: identified.is_some(),
    })?;
    // Belt and braces once a link-proven identity can reach here: `resolve_source_with_link`
    // already refuses one that does not derive to `source`, and this says the same thing about
    // the address-book path, where it used to be implicit in the lookup key.
    if source != delivery_destination(&source_identity) {
        return Err(DirectError::WrongSource);
    }
    if !message.verify_with(|bytes, signature| source_identity.verify(bytes, signature)) {
        return Err(DirectError::BadSignature);
    }
    if let Some(target) = stamp_cost {
        let Some(stamp) = message
            .payload
            .stamp
            .as_deref()
            .and_then(|stamp| <&[u8; STAMP_LEN]>::try_from(stamp).ok())
        else {
            return Err(DirectError::StampRequired(target));
        };
        if !valid_streamed(
            &message.message_id,
            MESSAGE_WORKBLOCK_ROUNDS,
            stamp,
            u16::from(target),
        ) {
            return Err(DirectError::InvalidStamp);
        }
    }
    Ok(ReceivedDirect {
        message,
        source_identity,
        mode,
        interface,
        packed,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum DirectError {
    #[error("Retinue delivery failed: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Announce(#[from] AnnounceError),
    #[error(transparent)]
    Codec(#[from] CodecError),
    #[error("the sender identity is not the endpoint identity")]
    LocalIdentityMismatch,
    #[error("the session or announce is not for the expected lxmf.delivery destination")]
    WrongDestination,
    /// Carries whether the peer identified itself on the link, because that is what
    /// separates the two causes: a sender that never announced and never said who it was,
    /// against one that identified as somebody other than the source its message claims.
    #[error(
        "the message source {address} has no validated delivery announce \
         (peer identified on the link: {identified})"
    )]
    UnknownSource {
        // Not named `source`: thiserror reads that name as the error's own cause.
        address: AddressHash,
        identified: bool,
    },
    #[error("the resolved identity does not derive to the source the message names")]
    WrongSource,
    #[error("the LXMF signature does not verify against the announced source identity")]
    BadSignature,
    #[error("the peer requires a direct-delivery stamp with cost {0}")]
    StampRequired(u8),
    #[error("the direct-delivery stamp is invalid")]
    InvalidStamp,
    #[error("the configured proof-of-work attempt budget was exhausted")]
    StampBudgetExhausted,
}
