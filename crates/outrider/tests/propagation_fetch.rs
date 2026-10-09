use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use outrider::propagation::Verification;
use outrider::{
    Acknowledgement, DeliveryAnnounce, FetchPolicy, LxmfPayload, NodePolicy,
    PROPAGATION_METADATA_NAME, PropagationAnnounce, PropagationBatch, PropagationCosts,
    PropagationError, PropagationNode, PropagationStamps, PropagationStore, PropagationStoreLimits,
    delivery_name, fetch_propagation, prepare_propagation, prepare_propagation_with,
    register_delivery, register_opportunistic, register_propagation, serve_fetch,
};
use retinue::endpoint::{Endpoint, PeerAnnounce, ResourceTransferConfig};
use retinue::hash::{AddressHash, full_hash};
use retinue::identity::PrivateIdentity;
use retinue::lossy::{LossModel, connect};
use retinue::ratchet::RatchetStore;
use rmpv::Value;

const QUICK: ResourceTransferConfig = ResourceTransferConfig {
    timeout: Duration::from_secs(5),
    retry_interval: Duration::from_millis(50),
    request_window: 1,
};

fn node_announce() -> PropagationAnnounce {
    PropagationAnnounce {
        legacy: false,
        unix_time: 1_753_603_200,
        active: true,
        transfer_limit_kb: 256.0,
        sync_limit_kb: 10_240.0,
        costs: PropagationCosts {
            propagation: 0,
            flexibility: 0,
            peering: 0,
        },
        metadata: vec![(
            Value::from(PROPAGATION_METADATA_NAME),
            Value::Binary(b"Fetch Node".to_vec()),
        )],
    }
}

async fn heard(endpoint: &Endpoint, destination: AddressHash) -> PeerAnnounce {
    loop {
        let announce = tokio::time::timeout(Duration::from_secs(2), endpoint.next_announcement())
            .await
            .unwrap()
            .unwrap();
        if announce.destination == destination {
            return announce;
        }
    }
}

/// A node that is also a known sender, and a recipient registered with ratchets.
struct Pair {
    node: Arc<Endpoint>,
    node_identity: PrivateIdentity,
    recipient: Arc<Endpoint>,
    recipient_destination: AddressHash,
    node_seen: PeerAnnounce,
}

async fn ratcheted_pair() -> Pair {
    let node_identity = PrivateIdentity::from_secret_bytes(&[0x70; 64]);
    let recipient_identity = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let node = Arc::new(Endpoint::new(node_identity.clone()));
    let recipient = Arc::new(Endpoint::new(recipient_identity));
    connect(&recipient, &node, LossModel::new(62), LossModel::new(70));

    let propagation = register_propagation(&node, &node_announce()).unwrap();
    let node_seen = heard(&recipient, propagation).await;
    let source = register_delivery(&node, &DeliveryAnnounce::named(b"Fetch Sender")).unwrap();
    heard(&recipient, source).await;
    let recipient_destination = register_opportunistic(
        &recipient,
        &DeliveryAnnounce::named(b"Ratcheted Recipient"),
        RatchetStore::new(Default::default()).unwrap(),
    )
    .unwrap();
    heard(&node, recipient_destination).await;
    Pair {
        node,
        node_identity,
        recipient,
        recipient_destination,
        node_seen,
    }
}

fn stamps(delivery_cost: Option<u8>) -> PropagationStamps {
    PropagationStamps {
        delivery_cost,
        propagation_cost: 0,
        seed: [0; 32],
        max_attempts: 100_000,
    }
}

#[tokio::test]
async fn large_ratcheted_fetch_response_uses_a_resource_and_authenticates() {
    let pair = ratcheted_pair().await;
    let content: Vec<u8> = (0..4_096_u32)
        .map(|value| value.wrapping_mul(73).wrapping_add(19) as u8)
        .collect();
    let prepared = prepare_propagation(
        &pair.node,
        &pair.node_identity,
        pair.recipient_destination,
        &LxmfPayload::text(1_753_603_204.5, b"PROPAGATION TITLE", content.clone()),
        &stamps(None),
    )
    .unwrap();
    let current = pair.recipient.current_ratchet_id(&delivery_name());
    assert!(current.is_some());
    assert_eq!(prepared.ratchet_id, current);

    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_entries: 4,
        max_bytes: 64 * 1024,
        max_message_bytes: 16 * 1024,
        max_age: Duration::from_secs(60),
        max_per_fetch: 1,
    });
    let batch = PropagationBatch {
        transfer_time: 1_753_603_205.0,
        entries: vec![prepared.entry],
    };
    assert_eq!(store.ingest(&batch, 1_753_603_205.0).inserted, 1);

    let store = PropagationNode::new(store, NodePolicy::from_announce(&node_announce()));
    let server = tokio::spawn({
        let node = Arc::clone(&pair.node);
        async move {
            let mut accepted = node.accept_resource().await.unwrap();
            accepted.session.set_config(QUICK);
            serve_fetch(&node, accepted, &store, || 1_753_603_206.0)
                .await
                .unwrap()
        }
    });
    let policy = FetchPolicy {
        max_messages: 1,
        retain_on_node: true,
        max_entry_bytes: 16 * 1024,
        max_message_bytes: 16 * 1024,
        resource: QUICK,
        ..FetchPolicy::default()
    };
    let receipt = tokio::time::timeout(
        Duration::from_secs(15),
        fetch_propagation(
            &pair.recipient,
            &pair.node_seen,
            1_753_603_206.0,
            |_| false,
            &policy,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    let served = tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(receipt.offered.len(), 1);
    assert!(receipt.rejected.is_empty());
    assert!(matches!(receipt.acknowledgement, Acknowledgement::NotSent));
    let fetched = &receipt.messages[0];
    assert_eq!(fetched.message.payload.content, content);
    assert_eq!(fetched.ratchet_id, current);
    assert_eq!(
        fetched.verification,
        Verification::Verified(*pair.node_identity.public())
    );
    assert_eq!(served.served, receipt.offered);
}

fn request_data(packed: &[u8]) -> Vec<Value> {
    let Value::Array(mut request) = rmpv::decode::read_value(&mut Cursor::new(packed)).unwrap()
    else {
        panic!("request is an array")
    };
    let Value::Array(data) = request.pop().unwrap() else {
        panic!("request data is an array")
    };
    data
}

fn ids(ids: &[[u8; 32]]) -> Value {
    Value::Array(ids.iter().map(|id| Value::Binary(id.to_vec())).collect())
}

fn packed(value: Value) -> Vec<u8> {
    let mut bytes = Vec::new();
    rmpv::encode::write_value(&mut bytes, &value).unwrap();
    bytes
}

#[tokio::test]
async fn fetch_splits_haves_opens_each_entry_and_acknowledges_everything_received() {
    let pair = ratcheted_pair().await;
    let payload = |title: &[u8]| LxmfPayload::text(1_753_603_204.5, title.to_vec(), b"body");
    let prepare = |sender: &PrivateIdentity, title: &[u8], cost| {
        let node = Arc::clone(&pair.node);
        let destination = pair.recipient_destination;
        prepare_propagation_with(
            sender,
            destination,
            &payload(title),
            &stamps(cost),
            |plain| {
                node.encrypt_for(destination, plain)
                    .map_err(PropagationError::Decrypt)
            },
        )
        .unwrap()
        .entry
        .message()
        .encode()
    };
    let stranger = PrivateIdentity::from_secret_bytes(&[0x71; 64]);
    let good = prepare(&pair.node_identity, b"good", Some(4));
    let unknown = prepare(&stranger, b"unknown", Some(4));
    let unstamped = prepare(&pair.node_identity, b"unstamped", None);
    let mut corrupt = pair.recipient_destination.as_slice().to_vec();
    corrupt.extend_from_slice(&[0x5a; 128]);
    let unexpected = prepare(&pair.node_identity, b"unexpected", Some(4));
    let held = [0x48; 32];
    let id = |bytes: &[u8]| full_hash(bytes);
    let offered = [held, id(&good), id(&unknown), id(&unstamped), id(&corrupt)];
    let served = [&good, &unknown, &unstamped, &corrupt, &unexpected];
    let received: Vec<[u8; 32]> = served.iter().map(|bytes| id(bytes)).collect();

    let server = tokio::spawn({
        let node = Arc::clone(&pair.node);
        let (served, received) = (
            served.map(|bytes| Value::Binary(bytes.clone())).to_vec(),
            received.clone(),
        );
        async move {
            let mut accepted = node.accept_resource().await.unwrap();
            let session = &mut accepted.session;
            let list = session.receive_raw_request().await.unwrap();
            assert_eq!(request_data(&list.packed), vec![Value::Nil, Value::Nil]);
            session
                .respond_value_auto(list.request_id, &packed(ids(&offered)))
                .await
                .unwrap();
            let get = session.receive_raw_request().await.unwrap();
            assert_eq!(
                request_data(&get.packed),
                vec![ids(&offered[1..]), ids(&[held]), Value::from(7)]
            );
            session
                .respond_value_auto(get.request_id, &packed(Value::Array(served)))
                .await
                .unwrap();
            let ack = session.receive_raw_request().await.unwrap();
            assert_eq!(request_data(&ack.packed), vec![Value::Nil, ids(&received)]);
            session
                .respond_value_auto(ack.request_id, &packed(Value::Array(Vec::new())))
                .await
                .unwrap();

            // The next session is refused outright.
            let mut refused = node.accept_resource().await.unwrap();
            let list = refused.session.receive_raw_request().await.unwrap();
            refused
                .session
                .respond_value_auto(list.request_id, &packed(Value::from(0xf1)))
                .await
                .unwrap();
        }
    });
    let policy = FetchPolicy {
        max_messages: 4,
        transfer_limit_kb: 7,
        stamp_cost: Some(4),
        resource: QUICK,
        ..FetchPolicy::default()
    };
    let fetch = || {
        fetch_propagation(
            &pair.recipient,
            &pair.node_seen,
            1_753_603_206.0,
            |id| *id == held,
            &policy,
        )
    };
    let receipt = tokio::time::timeout(Duration::from_secs(10), fetch())
        .await
        .unwrap()
        .unwrap();

    assert_eq!(receipt.haves, vec![held]);
    assert_eq!(receipt.wants, offered[1..].to_vec());
    assert!(matches!(
        receipt.acknowledgement,
        Acknowledgement::Confirmed
    ));
    let titles: Vec<_> = receipt
        .messages
        .iter()
        .map(|fetched| {
            (
                fetched.message.payload.title.clone(),
                fetched.verification.clone(),
            )
        })
        .collect();
    assert_eq!(
        titles,
        vec![
            (
                b"good".to_vec(),
                Verification::Verified(*pair.node_identity.public())
            ),
            (b"unknown".to_vec(), Verification::SourceUnknown),
        ]
    );
    let rejected: Vec<_> = receipt
        .rejected
        .iter()
        .map(|rejected| (rejected.transient_id, format!("{:?}", rejected.error)))
        .collect();
    assert_eq!(rejected.len(), 3);
    assert_eq!(rejected[0], (id(&unstamped), "InvalidDeliveryStamp".into()));
    assert_eq!(rejected[1].0, id(&corrupt));
    assert!(rejected[1].1.starts_with("Decrypt"));
    assert_eq!(
        rejected[2],
        (id(&unexpected), "UnexpectedTransientId".into())
    );

    let refused = tokio::time::timeout(Duration::from_secs(10), fetch())
        .await
        .unwrap();
    assert!(matches!(refused, Err(PropagationError::NoAccess)));
    server.await.unwrap();
}
