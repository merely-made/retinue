//! A propagation node's link service against a raw client: the stock request grammar,
//! error answers and submission policy.

use std::sync::Arc;
use std::time::Duration;

use outrider::{
    FETCH_PATH_HASH, LxmfPayload, NodePolicy, PROPAGATION_METADATA_NAME, PropagationAnnounce,
    PropagationBatch, PropagationCosts, PropagationEntry, PropagationError, PropagationNode,
    PropagationStore, PropagationStoreLimits, ServedFetch, prepare_propagation,
    register_propagation, serve_fetch,
};
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceSession, SessionInbound};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use rmpv::Value;

const NOW: f64 = 1_753_603_210.0;

struct Pair {
    node: Arc<Endpoint>,
    client: Arc<Endpoint>,
    client_identity: PrivateIdentity,
    announce: PeerAnnounce,
}

async fn pair(seed: u8) -> Pair {
    let client_identity = PrivateIdentity::from_secret_bytes(&[seed; 64]);
    let node = Arc::new(Endpoint::new(PrivateIdentity::from_secret_bytes(
        &[0x70; 64],
    )));
    let client = Arc::new(Endpoint::new(client_identity.clone()));
    connect(&client, &node, LossModel::new(1), LossModel::new(2));
    let announce = PropagationAnnounce {
        legacy: false,
        unix_time: NOW as u64,
        active: true,
        transfer_limit_kib: 256,
        sync_limit_kib: 10_240,
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
    prepare_propagation(
        &PrivateIdentity::from_secret_bytes(&[0x61; 64]),
        recipient.public(),
        &LxmfPayload::text(NOW, [index], vec![index; usize::from(index) * 40]),
        &[index; 32],
        &[index; 16],
        [index; 32],
        cost,
        1 << 20,
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

#[tokio::test]
async fn a_bad_stamp_is_answered_0xf5_and_its_identified_sender_throttled() {
    let pair = pair(0x63).await;
    let node = state(policy());
    let bad = entry(&pair.client_identity, 1, 0);
    assert!(bad.stamp_value() < 8, "pick another seed");
    let server = serve(&pair, &node);
    let mut session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    session.identify();
    let batch = PropagationBatch {
        transfer_time: NOW,
        entries: vec![bad],
    };
    session.send_data(&batch.encode().unwrap());
    let SessionInbound::Data(signal) = session.next_inbound(Duration::from_secs(5)).await.unwrap()
    else {
        panic!("expected the rejection packet")
    };
    assert_eq!(signal.data, [0x91, 0xcc, 0xf5]);
    let served = server.await.unwrap().unwrap();
    assert_eq!((served.rejected, served.stored.inserted), (1, 0));
    assert!(node.store().is_empty());
    let sender = *pair.client_identity.public().hash().as_bytes();
    assert!(node.is_throttled(&sender, NOW + 179.0));

    // A good stamp from the throttled sender is refused too.
    let server = serve(&pair, &node);
    let session = pair
        .client
        .open_resource(pair.announce.destination, pair.announce.identity)
        .await
        .unwrap();
    session.identify();
    let good = PropagationBatch {
        transfer_time: NOW,
        entries: vec![entry(&pair.client_identity, 2, 8)],
    };
    session.send_data(&good.encode().unwrap());
    assert!(matches!(
        server.await.unwrap(),
        Err(PropagationError::Throttled)
    ));
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
