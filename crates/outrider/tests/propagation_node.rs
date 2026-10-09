//! A propagation node's link service against a raw client: the stock request grammar,
//! error answers and submission policy.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use outrider::{
    FETCH_PATH_HASH, LxmfPayload, NodePolicy, PROPAGATION_METADATA_NAME, PropagationAnnounce,
    PropagationBatch, PropagationCosts, PropagationEntry, PropagationError, PropagationNode,
    PropagationStamps, PropagationStore, PropagationStoreLimits, ServedFetch, delivery_destination,
    prepare_propagation_with, register_propagation, serve_fetch,
};
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceSession, SessionInbound};
use retinue::identity::PrivateIdentity;
use retinue::packet::{DestinationType, PacketType};
use retinue::token::encrypt_to_identity;
use rmpv::Value;

const NOW: f64 = 1_753_603_210.0;

struct Pair {
    node: Arc<Endpoint>,
    client: Arc<Endpoint>,
    client_identity: PrivateIdentity,
    announce: PeerAnnounce,
    /// Proofs of link data packets the node sent the client.
    packet_proofs: Arc<AtomicUsize>,
}

/// Wire `client` to `node`, counting the node's proofs of link data packets.
fn wire(client: &Endpoint, node: &Endpoint) -> Arc<AtomicUsize> {
    let (mut client_out, client_sink) = client.attach_interface().split();
    let (mut node_out, node_sink) = node.attach_interface().split();
    let proofs = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        while let Some(packet) = client_out.recv().await {
            if !node_sink.deliver(packet) {
                break;
            }
        }
    });
    let counted = Arc::clone(&proofs);
    tokio::spawn(async move {
        while let Some(packet) = node_out.recv().await {
            if packet.packet_type == PacketType::Proof
                && packet.destination_type == DestinationType::Link
                && packet.context == 0
            {
                counted.fetch_add(1, Ordering::AcqRel);
            }
            if !client_sink.deliver(packet) {
                break;
            }
        }
    });
    proofs
}

async fn pair(seed: u8) -> Pair {
    let client_identity = PrivateIdentity::from_secret_bytes(&[seed; 64]);
    let node = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x70; 64],
    )));
    let client = Arc::new(Endpoint::new(client_identity.clone()));
    let packet_proofs = wire(&client, &node);
    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: NOW as u64,
        active: true,
        transfer_limit_kb: 256.0,
        sync_limit_kb: 10_240.0,
        costs: PropagationCosts {
            propagation: 8,
            flexibility: 0,
            peering: 18,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"Node".to_vec()),
        )],
    };
    register_propagation(&node, &announce).unwrap();
    let announce = tokio::time::timeout(Duration::from_secs(2), client.next_announcement())
        .await
        .unwrap()
        .unwrap();
    Pair {
        node,
        client,
        client_identity,
        announce,
        packet_proofs,
    }
}

fn policy() -> NodePolicy {
    NodePolicy {
        costs: PropagationCosts {
            propagation: 8,
            flexibility: 0,
            peering: 18,
        },
        max_transfer_bytes: 2_000,
        allowed: None,
        link_idle: Duration::from_secs(10),
    }
}

fn state(policy: NodePolicy) -> Arc<PropagationNode> {
    Arc::new(PropagationNode::new(
        PropagationStore::new(PropagationStoreLimits {
            max_message_bytes: 4_096,
            ..PropagationStoreLimits::default()
        }),
        policy,
    ))
}

fn entry(recipient: &PrivateIdentity, index: u8, cost: u16) -> PropagationEntry {
    let stamps = PropagationStamps {
        delivery_cost: None,
        propagation_cost: cost,
        seed: [index; 32],
        max_attempts: 1 << 20,
    };
    prepare_propagation_with(
        &PrivateIdentity::from_secret_bytes(&[0x61; 64]),
        delivery_destination(recipient.public()),
        &LxmfPayload::text(NOW, [index], vec![index; usize::from(index) * 40]),
        &stamps,
        |plaintext| {
            let token =
                encrypt_to_identity(recipient.public(), &[index; 32], &[index; 16], plaintext);
            Ok((token, None))
        },
    )
    .unwrap()
    .entry
}

fn serve(
    pair: &Pair,
    node: &Arc<PropagationNode>,
) -> tokio::task::JoinHandle<Result<ServedFetch, PropagationError>> {
    let endpoint = Arc::clone(&pair.node);
    let node = Arc::clone(node);
    tokio::spawn(async move {
        let accepted = endpoint.accept_resource().await.unwrap();
        serve_fetch(&endpoint, accepted, &node, || NOW).await
    })
}

async fn get(session: &mut ResourceSession, data: Value) -> Value {
    let mut packed = Vec::new();
    rmpv::encode::write_value(
        &mut packed,
        &Value::Array(vec![
            Value::F64(NOW),
            Value::Binary(FETCH_PATH_HASH.to_vec()),
            data,
        ]),
    )
    .unwrap();
    let response = session.request_raw(&packed).await.unwrap();
    let Value::Array(mut envelope) =
        rmpv::decode::read_value(&mut response.packed.as_slice()).unwrap()
    else {
        panic!("response is not an envelope")
    };
    envelope.pop().unwrap()
}

fn ids(ids: &[[u8; 32]]) -> Value {
    Value::Array(ids.iter().map(|id| Value::Binary(id.to_vec())).collect())
}

#[tokio::test]
async fn one_link_offers_fetches_acknowledges_then_offers_nothing() {
    let pair = pair(0x62).await;
    let node = state(policy());
    let entries: Vec<_> = [3, 1, 2]
        .into_iter()
        .map(|index| entry(&pair.client_identity, index, 0))
        .collect();
    node.store().ingest(
        &PropagationBatch {
            transfer_time: NOW,
            entries: entries.clone(),
        },
        NOW,
    );
    let server = serve(&pair, &node);

    let mut session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    session.identify();
    let smallest_first: Vec<[u8; 32]> = [1, 2, 0].map(|at| entries[at].transient_id()).into();
    let offer = get(&mut session, Value::Array(vec![Value::Nil, Value::Nil])).await;
    assert_eq!(offer, ids(&smallest_first));
    let fetched = get(
        &mut session,
        Value::Array(vec![
            ids(&smallest_first),
            Value::Array(Vec::new()),
            Value::F64(1000.0),
        ]),
    )
    .await;
    assert!(matches!(&fetched, Value::Array(messages) if messages.len() == 3));
    let ack = get(
        &mut session,
        Value::Array(vec![Value::Nil, ids(&smallest_first)]),
    )
    .await;
    assert_eq!(ack, Value::Array(Vec::new()));
    let again = get(&mut session, Value::Array(vec![Value::Nil, Value::Nil])).await;
    assert_eq!(again, Value::Array(Vec::new()));
    drop(session);

    let served = server.await.unwrap().unwrap();
    assert_eq!((served.served.len(), served.acknowledged), (3, 3));
    assert!(node.store().is_empty());
}

#[tokio::test]
async fn an_unidentified_or_disallowed_fetch_gets_the_stock_error() {
    let pair = pair(0x62).await;
    let mut policy = policy();
    policy.allowed = Some([[9; 16]].into());
    let node = state(policy);
    let server = serve(&pair, &node);
    let mut session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    let offer = Value::Array(vec![Value::Nil, Value::Nil]);
    assert_eq!(get(&mut session, offer.clone()).await, Value::from(0xf0));
    session.identify();
    assert_eq!(get(&mut session, offer).await, Value::from(0xf1));
    drop(session);
    assert_eq!(server.await.unwrap().unwrap().owner, None);
}

async fn open(pair: &Pair) -> ResourceSession {
    let session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    session.identify();
    session
}

async fn rejection(session: &mut ResourceSession) -> Vec<u8> {
    match session.next_inbound(Duration::from_secs(5)).await.unwrap() {
        SessionInbound::Data(signal) => signal.data,
        other => panic!("expected the rejection packet, got {other:?}"),
    }
}

#[tokio::test]
async fn a_bad_stamp_packet_is_answered_0xf5_and_its_sender_not_throttled() {
    let pair = pair(0x63).await;
    let node = state(policy());
    let bad = entry(&pair.client_identity, 1, 0);
    assert!(bad.stamp_value() < 8, "pick another seed");
    let server = serve(&pair, &node);
    let mut session = open(&pair).await;
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![bad],
    };
    session.send_data(&batch.encode().unwrap());
    assert_eq!(rejection(&mut session).await, [0x91, 0xcc, 0xf5]);
    let served = server.await.unwrap().unwrap();
    assert_eq!((served.rejected, served.stored.inserted), (1, 0));
    assert!(node.store().is_empty());
    // Stock's packet path throttles no one (`LXMRouter.py` 2303-2329).
    let sender = *pair.client_identity.public().hash().as_bytes();
    assert!(!node.is_throttled(&sender, NOW));
}

#[tokio::test]
async fn a_bad_stamp_resource_throttles_its_identified_sender() {
    let pair = pair(0x63).await;
    let node = state(policy());
    let server = serve(&pair, &node);
    let mut session = open(&pair).await;
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![entry(&pair.client_identity, 1, 0)],
    };
    session.publish(&batch.encode().unwrap()).await.unwrap();
    let served = server.await.unwrap().unwrap();
    assert_eq!((served.rejected, served.stored.inserted), (1, 0));
    let sender = *pair.client_identity.public().hash().as_bytes();
    assert!(node.is_throttled(&sender, NOW + 179.0));
    assert!(node.store().is_empty());
}

#[tokio::test]
async fn a_packet_entry_too_short_for_a_stamp_is_rejected_and_the_rest_kept() {
    let pair = pair(0x62).await;
    let mut policy = policy();
    policy.costs.propagation = 0;
    let node = state(policy);
    let good = entry(&pair.client_identity, 1, 0);
    let server = serve(&pair, &node);
    let mut session = open(&pair).await;
    let mut packed = Vec::new();
    rmpv::encode::write_value(
        &mut packed,
        &Value::Array(vec![
            Value::F64(NOW),
            Value::Array(vec![
                Value::Binary(good.encode()),
                Value::Binary(vec![0; 40]),
            ]),
        ]),
    )
    .unwrap();
    session.send_data(&packed);
    assert_eq!(rejection(&mut session).await, [0x91, 0xcc, 0xf5]);
    let served = server.await.unwrap().unwrap();
    assert_eq!((served.rejected, served.stored.inserted), (1, 1));
    assert!(node.store().has_processed(&good.transient_id()));
}

#[tokio::test]
async fn a_packet_the_store_refuses_as_too_large_goes_unproven() {
    let pair = pair(0x62).await;
    let mut policy = policy();
    policy.costs.propagation = 0;
    let node = Arc::new(PropagationNode::new(
        PropagationStore::new(PropagationStoreLimits {
            max_message_bytes: 100,
            ..PropagationStoreLimits::default()
        }),
        policy,
    ));
    let server = serve(&pair, &node);
    let mut session = open(&pair).await;
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![entry(&pair.client_identity, 1, 0)],
    };
    session.send_data(&batch.encode().unwrap());
    // The link goes on: the next request is answered.
    let offer = get(&mut session, Value::Array(vec![Value::Nil, Value::Nil])).await;
    assert_eq!(offer, Value::Array(Vec::new()));
    drop(session);
    let served = server.await.unwrap().unwrap();
    assert_eq!(
        (served.stored.rejected_too_large, served.stored.inserted),
        (1, 0)
    );
    assert_eq!(pair.packet_proofs.load(Ordering::Acquire), 0);
    assert!(!node.store().has_processed(&batch.entries[0].transient_id()));
}

#[tokio::test]
async fn a_stored_packet_is_proven() {
    let pair = pair(0x62).await;
    let mut policy = policy();
    policy.costs.propagation = 0;
    let node = state(policy);
    let server = serve(&pair, &node);
    let mut session = open(&pair).await;
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![entry(&pair.client_identity, 1, 0)],
    };
    session.send_data(&batch.encode().unwrap());
    let offer = get(&mut session, Value::Array(vec![Value::Nil, Value::Nil])).await;
    assert_eq!(offer, ids(&[batch.entries[0].transient_id()]));
    drop(session);
    assert_eq!(server.await.unwrap().unwrap().stored.inserted, 1);
    assert_eq!(pair.packet_proofs.load(Ordering::Acquire), 1);
}

#[tokio::test]
async fn a_resource_past_what_the_store_admits_is_refused_at_advertisement() {
    let pair = pair(0x62).await;
    let mut policy = policy();
    policy.max_transfer_bytes = 1 << 20;
    let limits = PropagationStoreLimits {
        max_message_bytes: 300,
        ..PropagationStoreLimits::default()
    };
    let node = Arc::new(PropagationNode::new(
        PropagationStore::new(limits.clone()),
        policy,
    ));
    assert_eq!(
        node.policy().max_transfer_bytes,
        limits.max_submission_bytes()
    );
    let server = serve(&pair, &node);
    let mut session = open(&pair).await;
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![entry(&pair.client_identity, 10, 8)],
    };
    assert!(batch.entries[0].message().encode().len() > 300);
    assert!(session.publish(&batch.encode().unwrap()).await.is_err());
    drop(session);
    let served = server.await.unwrap().unwrap();
    assert_eq!(served.stored, Default::default());
    assert!(node.store().is_empty());
}

#[tokio::test]
async fn a_client_transfer_of_two_entries_is_ignored() {
    let pair = pair(0x62).await;
    let mut policy = policy();
    policy.costs.propagation = 0;
    let node = state(policy);
    let server = serve(&pair, &node);
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![
            entry(&pair.client_identity, 1, 0),
            entry(&pair.client_identity, 2, 0),
        ],
    };
    let mut session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    session.publish(&batch.encode().unwrap()).await.unwrap();
    assert!(matches!(
        server.await.unwrap(),
        Err(PropagationError::UnpeeredBatch)
    ));
    assert!(node.store().is_empty());
}

#[tokio::test]
async fn a_submission_past_the_ceiling_is_refused_before_transfer() {
    let pair = pair(0x62).await;
    let node = state(policy());
    let server = serve(&pair, &node);
    let mut session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    let oversized = PropagationBatch {
        transfer_time: NOW,
        entries: vec![entry(&pair.client_identity, 255, 8)],
    };
    let packed = oversized.encode().unwrap();
    assert!(packed.len() > policy().max_transfer_bytes);
    assert!(session.publish(&packed).await.is_err());
    drop(session);
    let served = server.await.unwrap().unwrap();
    assert_eq!(served.stored.inserted, 0);
    assert!(node.store().is_empty());
}
