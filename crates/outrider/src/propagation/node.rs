//! Node lanes: serve client links (fetches and submissions), and announce the node.

use std::io;

use retinue::destination::DestinationName;
use retinue::endpoint::{
    AcceptedResource, Endpoint, InterfaceId, PayloadMode, ReceivedRawRequest, ResourceSession,
    SessionInbound,
};
use retinue::hash::AddressHash;
use retinue::identity::Identity;
use rmpv::Value;

use super::msgpack::{
    ERROR_INVALID_STAMP, ERROR_NO_ACCESS, ERROR_NO_IDENTITY, GetRequest, decode_fetch_request,
    decode_get_request, decode_one, encode_value,
};
use super::policy::score_stamps;
use super::wire::number;
use super::{
    DEFAULT_MAX_PROPAGATION_BATCH_BYTES, DEFAULT_MAX_PROPAGATION_ENTRIES, PropagationAnnounce,
    PropagationBatch, PropagationEntry, PropagationError, PropagationNode, StoreReceipt,
};
use crate::announce::delivery_destination;

/// `[0xf5]` packed: the invalid-stamp signal a node sends on a link (`LXMRouter.py` 2327).
const INVALID_STAMP_SIGNAL: [u8; 3] = [0x91, 0xcc, ERROR_INVALID_STAMP];

#[derive(Clone, Debug, PartialEq)]
pub struct ReceivedPropagationBatch {
    /// The transfer, holding only the entries whose stamps passed.
    pub batch: PropagationBatch,
    pub mode: PayloadMode,
    pub interface: InterfaceId,
    pub packed_batch: Vec<u8>,
    /// Entries refused for a stamp under the node's floor, or too short to hold one.
    pub rejected: usize,
    pub stored: StoreReceipt,
}

/// What one client link did on the node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServedFetch {
    /// The identity the client proved, once it fetched.
    pub owner: Option<Identity>,
    /// The last offer.
    pub offered: Vec<[u8; 32]>,
    /// The messages the last response that was asked for any carried, in order.
    pub served: Vec<[u8; 32]>,
    /// Messages served over the whole link.
    pub served_total: usize,
    pub acknowledged: usize,
    /// How the last offer crossed the node's Retinue link.
    pub offer_mode: Option<PayloadMode>,
    /// How the last message response crossed, or `None` when none was asked for. This
    /// is a node-side carriage fact, not an inferred size tier.
    pub message_mode: Option<PayloadMode>,
    /// Submissions on the same link.
    pub stored: StoreReceipt,
    pub rejected: usize,
}

/// Serve one client link as a stock node does (`LXMRouter.py` 1488-1560, 2303-2329): answer
/// each `/get`, store submissions that arrive on the same link, until the link closes or
/// stays silent for the policy's `link_idle`. A submission with a stamp under the floor
/// ends the link: a packet is answered with 0xf5 first, and the identified sender of a
/// Resource is throttled. A packet whose message the store refuses as too large is not
/// proven, so its sender retries rather than counting it sent.
pub async fn serve_fetch(
    endpoint: &Endpoint,
    accepted: AcceptedResource,
    node: &PropagationNode,
    clock: impl Fn() -> f64 + Sync,
) -> Result<ServedFetch, PropagationError> {
    Ok(serve(endpoint, accepted, node, &clock, false).await?.0)
}

/// Serve a client link until its first submission, and return that submission. Requests
/// before it are answered as [`serve_fetch`] answers them.
pub async fn receive_submission(
    endpoint: &Endpoint,
    accepted: AcceptedResource,
    node: &PropagationNode,
    clock: impl Fn() -> f64 + Sync,
) -> Result<ReceivedPropagationBatch, PropagationError> {
    serve(endpoint, accepted, node, &clock, true)
        .await?
        .1
        .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "link closed").into())
}

async fn serve(
    endpoint: &Endpoint,
    mut accepted: AcceptedResource,
    node: &PropagationNode,
    clock: &(dyn Fn() -> f64 + Sync),
    first_submission: bool,
) -> Result<(ServedFetch, Option<ReceivedPropagationBatch>), PropagationError> {
    if accepted.destination != propagation_destination(endpoint.identity()) {
        return Err(PropagationError::WrongDestination);
    }
    let session = &mut accepted.session;
    session.set_max_resource_size(node.policy().max_transfer_bytes);
    let mut report = ServedFetch::default();
    loop {
        let (mode, packed, data) = match session.next_inbound(node.policy().link_idle).await {
            Ok(SessionInbound::Request(request)) => {
                answer(session, node, request, clock, &mut report).await?;
                continue;
            }
            Ok(SessionInbound::Data(data)) => (PayloadMode::Data, data.data.clone(), Some(data)),
            Ok(SessionInbound::Resource(bytes)) => (PayloadMode::Resource, bytes, None),
            Err(error) if is_end(&error) => return Ok((report, None)),
            Err(error) => return Err(error.into()),
        };
        let received = admit(session, node, mode, packed, clock).await?;
        report.rejected += received.rejected;
        add(&mut report.stored, &received.stored);
        if let Some(data) = &data {
            if received.rejected > 0 {
                session.send_data(&INVALID_STAMP_SIGNAL);
            } else if received.stored.rejected_too_large == 0 {
                session.prove(data);
            }
        }
        if first_submission {
            return Ok((report, Some(received)));
        }
        if received.rejected > 0 {
            return Ok((report, None));
        }
    }
}

fn is_end(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe | io::ErrorKind::TimedOut
    )
}

/// Decode, police and store one submission (`LXMRouter.py` 2303-2329, 2451-2523). Stock's
/// packet path takes any number of entries and throttles no one; its Resource path takes
/// one entry from a sender that is not a peer, and throttles an identified sender of a bad
/// stamp.
async fn admit(
    session: &ResourceSession,
    node: &PropagationNode,
    mode: PayloadMode,
    packed_batch: Vec<u8>,
    clock: &(dyn Fn() -> f64 + Sync),
) -> Result<ReceivedPropagationBatch, PropagationError> {
    let resource = mode == PayloadMode::Resource;
    // A packet is bounded by the link; only a Resource is held to the transfer ceiling.
    let max_bytes = if resource {
        node.policy().max_transfer_bytes
    } else {
        DEFAULT_MAX_PROPAGATION_BATCH_BYTES
    };
    let (mut batch, undecodable) = decode_submission(&packed_batch, max_bytes)?;
    // This node has no peers, so every sender is a client.
    if resource && batch.entries.len() + undecodable > 1 {
        return Err(PropagationError::UnpeeredBatch);
    }
    let floor = node.policy().stamp_floor();
    let entries = std::mem::take(&mut batch.entries);
    let (valid, invalid) = tokio::task::spawn_blocking(move || score_stamps(entries, floor))
        .await
        .map_err(io::Error::other)?;
    let rejected = invalid + undecodable;
    let now = clock();
    if resource
        && rejected > 0
        && let Some(sender) = session.identified_peer()
    {
        node.throttle(*sender.hash().as_bytes(), now);
    }
    batch.entries = valid.iter().map(|(entry, _)| entry.clone()).collect();
    let stored = node.store().ingest_scored(valid, now);
    Ok(ReceivedPropagationBatch {
        batch,
        mode,
        interface: session.interface(),
        packed_batch,
        rejected,
        stored,
    })
}

/// Decode `[timebase, [entry, ...]]`, counting an entry too short to hold a message and
/// its stamp as invalid, as stock's validator does (`LXStamper.py` 84-96), rather than
/// failing the transfer.
fn decode_submission(
    packed: &[u8],
    max_bytes: usize,
) -> Result<(PropagationBatch, usize), PropagationError> {
    if packed.len() > max_bytes.min(DEFAULT_MAX_PROPAGATION_BATCH_BYTES) {
        return Err(PropagationError::BatchTooLarge);
    }
    let Value::Array(parts) = decode_one(packed)? else {
        return Err(PropagationError::InvalidBatch);
    };
    let [transfer_time, Value::Array(packed_entries)] = parts.as_slice() else {
        return Err(PropagationError::InvalidBatch);
    };
    // Any finite number, as for a client's batch (`LXMRouter.py` 2410).
    let transfer_time = number(transfer_time).map_err(|_| PropagationError::InvalidTransferTime)?;
    if packed_entries.len() > DEFAULT_MAX_PROPAGATION_ENTRIES {
        return Err(PropagationError::TooManyEntries);
    }
    let mut entries = Vec::with_capacity(packed_entries.len());
    let mut undecodable = 0;
    for entry in packed_entries {
        let Value::Binary(entry) = entry else {
            return Err(PropagationError::InvalidBatch);
        };
        match PropagationEntry::decode(entry, max_bytes) {
            Ok(entry) => entries.push(entry),
            Err(_) => undecodable += 1,
        }
    }
    let batch = PropagationBatch {
        transfer_time,
        entries,
    };
    Ok((batch, undecodable))
}

/// Answer one request. A request on another path goes unanswered, as RNS ignores a path
/// with no handler; a malformed `/get` is answered with nil, as stock's handler is.
async fn answer(
    session: &mut ResourceSession,
    node: &PropagationNode,
    request: ReceivedRawRequest,
    clock: &(dyn Fn() -> f64 + Sync),
    report: &mut ServedFetch,
) -> Result<(), PropagationError> {
    let Ok(data) = decode_fetch_request(&request.packed) else {
        return Ok(());
    };
    // Which mode this response reports: the offer's, or the messages'.
    let mut carries = None;
    let response = match (request.peer, decode_get_request(&data)) {
        (None, _) => Value::from(ERROR_NO_IDENTITY),
        (Some(owner), _) if !node.policy().allows(owner.hash().as_bytes()) => {
            Value::from(ERROR_NO_ACCESS)
        }
        (Some(_), Err(_)) => Value::Nil,
        (Some(owner), Ok(get)) => {
            report.owner = Some(owner);
            carries = match &get {
                GetRequest::Offer => Some(true),
                GetRequest::Fetch { wanted, .. } => (!wanted.is_empty()).then_some(false),
            };
            let destination = *delivery_destination(&owner).as_bytes();
            node.answer_get(destination, get, clock(), report)
        }
    };
    let mode = session
        .respond_value_auto(request.request_id, &encode_value(&response)?)
        .await?;
    match carries {
        Some(true) => report.offer_mode = Some(mode),
        Some(false) => report.message_mode = Some(mode),
        None => {}
    }
    Ok(())
}

fn add(total: &mut StoreReceipt, more: &StoreReceipt) {
    total.inserted += more.inserted;
    total.duplicates += more.duplicates;
    total.rejected_too_large += more.rejected_too_large;
    total.evicted += more.evicted;
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
