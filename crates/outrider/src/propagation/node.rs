//! Node lanes: receive submissions, serve fetches, and announce the node.

use retinue::destination::DestinationName;
use retinue::endpoint::{AcceptedResource, Endpoint, InterfaceId, PayloadMode, ReceivedPayload};
use retinue::hash::AddressHash;
use retinue::identity::Identity;
use rmpv::Value;

use super::msgpack::{decode_fetch_selection, decode_offer_request, encode_value};
use super::{
    PropagationAnnounce, PropagationBatch, PropagationError, PropagationMessage, PropagationStore,
};
use crate::announce::delivery_destination;

#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedPropagationBatch {
    pub batch: PropagationBatch,
    pub mode: PayloadMode,
    pub interface: InterfaceId,
    pub packed_batch: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServedFetch {
    pub owner: Identity,
    pub offered: Vec<[u8; 32]>,
    pub served: Vec<[u8; 32]>,
    pub acknowledged: usize,
    /// How the offer response crossed the node's Retinue link.
    pub offer_mode: PayloadMode,
    /// How the selected message batch crossed, or `None` when nothing was
    /// offered. This is a node-side carriage fact, not an inferred size tier.
    pub message_mode: Option<PayloadMode>,
}

/// Serve one stock-compatible two-request fetch session.
pub async fn serve_fetch(
    endpoint: &Endpoint,
    accepted: &mut AcceptedResource,
    store: &mut PropagationStore,
    now: f64,
) -> Result<ServedFetch, PropagationError> {
    if accepted.destination != propagation_destination(endpoint.identity()) {
        return Err(PropagationError::WrongDestination);
    }
    store.prune(now);
    let offer_request = accepted
        .session
        .receive_raw_request()
        .await
        .map_err(PropagationError::OfferRequest)?;
    let owner = offer_request
        .peer
        .ok_or(PropagationError::UnidentifiedFetch)?;
    decode_offer_request(&offer_request.packed)?;
    let destination = *delivery_destination(&owner).as_bytes();
    let offered = store.offer(destination, store.limits.max_per_fetch);
    let offer_value = Value::Array(
        offered
            .iter()
            .map(|id| Value::Binary(id.to_vec()))
            .collect(),
    );
    let packed_offer = encode_value(&offer_value)?;
    let offer_mode = accepted
        .session
        .respond_value_auto(offer_request.request_id, &packed_offer)
        .await?;

    if offered.is_empty() {
        return Ok(ServedFetch {
            owner,
            offered,
            served: Vec::new(),
            acknowledged: 0,
            offer_mode,
            message_mode: None,
        });
    }

    let fetch_request = accepted
        .session
        .receive_raw_request()
        .await
        .map_err(PropagationError::SelectionRequest)?;
    if fetch_request.peer != Some(owner) {
        return Err(PropagationError::FetchIdentityChanged);
    }
    let (wanted, handled, limit) = decode_fetch_selection(&fetch_request.packed)?;
    let acknowledged = store.acknowledge(destination, &handled);
    let wanted: Vec<[u8; 32]> = wanted
        .into_iter()
        .filter(|id| offered.contains(id))
        .take(
            usize::try_from(limit)
                .unwrap_or(usize::MAX)
                .min(store.limits.max_per_fetch),
        )
        .collect();
    let messages = store.messages(destination, &wanted);
    let served: Vec<[u8; 32]> = messages
        .iter()
        .map(PropagationMessage::transient_id)
        .collect();
    let packed_messages = encode_value(&Value::Array(
        messages
            .iter()
            .map(|message| Value::Binary(message.encode()))
            .collect(),
    ))?;
    let message_mode = accepted
        .session
        .respond_value_auto(fetch_request.request_id, &packed_messages)
        .await?;
    Ok(ServedFetch {
        owner,
        offered,
        served,
        acknowledged,
        offer_mode,
        message_mode: Some(message_mode),
    })
}

pub async fn receive_submission(
    endpoint: &Endpoint,
    mut accepted: AcceptedResource,
    target_cost: u16,
    max_batch_bytes: usize,
    max_entries: usize,
) -> Result<ReceivedPropagationBatch, PropagationError> {
    if accepted.destination != propagation_destination(endpoint.identity()) {
        return Err(PropagationError::WrongDestination);
    }
    let interface = accepted.interface;
    let (mode, packed_batch) = match accepted.session.receive().await? {
        ReceivedPayload::Data(bytes) => (PayloadMode::Data, bytes),
        ReceivedPayload::Resource(bytes) => (PayloadMode::Resource, bytes),
    };
    let batch = PropagationBatch::decode(&packed_batch, max_batch_bytes, max_entries)?;
    if batch
        .entries
        .iter()
        .any(|entry| !entry.validate_stamp(target_cost))
    {
        return Err(PropagationError::InvalidStamp);
    }
    Ok(ReceivedPropagationBatch {
        batch,
        mode,
        interface,
        packed_batch,
    })
}

/// Register the node's destination and announce it. Path responses rebuild the app data
/// with the timebase of the moment (`LXMRouter.py` 193, 332-346); `announce.unix_time` is
/// not used.
pub fn register_propagation(
    endpoint: &Endpoint,
    announce: &PropagationAnnounce,
) -> Result<AddressHash, PropagationError> {
    let app_data = announce_at(announce, unix_now())?;
    let name = propagation_name();
    let destination = name.destination_hash(endpoint.identity());
    endpoint.register_resource(name.clone(), &app_data);
    install_app_data(endpoint, &name, announce)?;
    Ok(destination)
}

/// Announce the node with the timebase set to now, and make `announce` what later path
/// responses carry, as stock re-reads its state at each announce.
pub fn announce_propagation(
    endpoint: &Endpoint,
    announce: &PropagationAnnounce,
) -> Result<(), PropagationError> {
    let name = propagation_name();
    let app_data = announce_at(announce, unix_now())?;
    // An unregistered node has no path responses to keep current.
    let _ = install_app_data(endpoint, &name, announce);
    endpoint.announce(&name, &app_data);
    Ok(())
}

fn announce_at(
    announce: &PropagationAnnounce,
    unix_time: u64,
) -> Result<Vec<u8>, PropagationError> {
    PropagationAnnounce {
        unix_time,
        ..announce.clone()
    }
    .encode()
}

fn install_app_data(
    endpoint: &Endpoint,
    name: &DestinationName,
    announce: &PropagationAnnounce,
) -> Result<(), PropagationError> {
    let announce = announce.clone();
    // `announce` encoded once already, so this cannot fail.
    endpoint.set_app_data_source(name, move |seconds| {
        announce_at(&announce, seconds).unwrap_or_default()
    })?;
    Ok(())
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

pub fn propagation_name() -> DestinationName {
    DestinationName::new("lxmf", ["propagation"])
}

pub fn propagation_destination(identity: &Identity) -> AddressHash {
    propagation_name().destination_hash(identity)
}
