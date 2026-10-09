//! Store, snapshot and node-policy behaviour that needs no link.

use std::time::Duration;

use retinue::identity::PrivateIdentity;
use retinue::token::{IV_LEN, encrypt_to_identity};
use rmpv::Value;

use super::msgpack::{GetRequest, decode_get_request, decode_one, encode_value};
use super::policy::score_stamps;
use super::*;
use crate::announce::delivery_destination;
use crate::codec::LxmfPayload;
use crate::stamp::STAMP_LEN;

const DAY: f64 = 86_400.0;

fn entry(recipient: u8, index: u8, body_len: usize, cost: u16) -> PropagationEntry {
    let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient = PrivateIdentity::from_secret_bytes(&[recipient; 64]);
    let stamps = PropagationStamps {
        delivery_cost: None,
        propagation_cost: cost,
        seed: [0; STAMP_LEN],
        max_attempts: 1 << 20,
    };
    prepare_propagation_with(
        &sender,
        delivery_destination(recipient.public()),
        &LxmfPayload::text(f64::from(index), [index], vec![index; body_len]),
        &stamps,
        |plaintext| {
            let token = encrypt_to_identity(
                recipient.public(),
                &[index; 32],
                &[index; IV_LEN],
                plaintext,
            );
            Ok((token, None))
        },
    )
    .unwrap()
    .entry
}

fn owner(recipient: u8) -> [u8; 16] {
    *delivery_destination(PrivateIdentity::from_secret_bytes(&[recipient; 64]).public()).as_bytes()
}

fn limits(max_entries: usize) -> PropagationStoreLimits {
    PropagationStoreLimits {
        max_entries,
        max_bytes: 1 << 20,
        max_message_bytes: 4_096,
        max_age: Duration::from_secs(30 * 86_400),
        max_per_fetch: 16,
    }
}

fn batch(entries: &[&PropagationEntry]) -> PropagationBatch {
    PropagationBatch {
        transfer_time: 0.0,
        entries: entries.iter().map(|entry| (*entry).clone()).collect(),
    }
}

#[test]
fn offer_is_smallest_first_and_a_kb_budget_packs_as_stock() {
    let (large, small, medium) = (
        entry(0x62, 1, 600, 0),
        entry(0x62, 2, 10, 0),
        entry(0x62, 3, 300, 0),
    );
    let mut store = PropagationStore::new(limits(8));
    store.ingest(
        &batch(&[&large, &small, &medium, &entry(0x63, 4, 10, 0)]),
        1.0,
    );
    let ids = [
        large.transient_id(),
        small.transient_id(),
        medium.transient_id(),
    ];
    assert_eq!(store.offer(owner(0x62)), vec![ids[1], ids[2], ids[0]]);

    // 24 + (size + 16) per message: the large one overflows, the medium one after it fits.
    let size = |entry: &PropagationEntry| (entry.encode().len() + 16) as f64;
    let budget = 24.0 + size(&small) + size(&medium);
    let served: Vec<_> = store
        .select(owner(0x62), &[ids[1], ids[0], ids[2]], Some(budget))
        .iter()
        .map(PropagationMessage::transient_id)
        .collect();
    assert_eq!(served, vec![ids[1], ids[2]]);
    assert_eq!(store.select(owner(0x62), &ids, None).len(), 3);
    // Another owner's id is never served.
    assert!(store.select(owner(0x63), &ids, None).is_empty());
}

#[test]
fn a_get_request_takes_nil_ids_and_an_integer_or_float_limit() {
    let id = Value::Binary(vec![7; 32]);
    let decode = |parts: Vec<Value>| decode_get_request(&Value::Array(parts)).unwrap();
    assert_eq!(decode(vec![Value::Nil, Value::Nil]), GetRequest::Offer);
    assert_eq!(
        decode(vec![
            Value::Nil,
            Value::Array(vec![id.clone(), Value::from(3)])
        ]),
        GetRequest::Fetch {
            wanted: Vec::new(),
            handled: vec![[7; 32]],
            limit_kb: None,
        }
    );
    for limit in [Value::from(2), Value::F64(2.0), Value::F32(2.0)] {
        assert_eq!(
            decode(vec![Value::Array(vec![id.clone()]), Value::Nil, limit]),
            GetRequest::Fetch {
                wanted: vec![[7; 32]],
                handled: Vec::new(),
                limit_kb: Some(2.0),
            }
        );
    }
    assert!(decode_get_request(&Value::Array(vec![Value::Nil])).is_err());
}

#[test]
fn an_acknowledgement_deletes_and_answers_empty_and_the_id_is_remembered() {
    let first = entry(0x62, 1, 10, 0);
    let node = PropagationNode::new(PropagationStore::new(limits(8)), node_policy(0, 0));
    node.store().ingest(&batch(&[&first]), 10.0);
    let mut report = ServedFetch::default();
    let ack = GetRequest::Fetch {
        wanted: Vec::new(),
        handled: vec![first.transient_id()],
        limit_kb: None,
    };
    assert_eq!(
        node.answer_get(owner(0x62), ack, 11.0, &mut report),
        Value::Array(Vec::new())
    );
    assert_eq!(report.acknowledged, 1);
    assert!(node.store().is_empty());
    let offer = node.answer_get(owner(0x62), GetRequest::Offer, 12.0, &mut report);
    assert_eq!(offer, Value::Array(Vec::new()));

    // A stock resend of the same entry is not stored again for 180 days.
    let mut store = node.into_store();
    assert_eq!(store.ingest(&batch(&[&first]), 13.0).duplicates, 1);
    store.prune(13.0 + 181.0 * DAY);
    assert!(!store.has_processed(&first.transient_id()));
    assert_eq!(
        store.ingest(&batch(&[&first]), 13.0 + 181.0 * DAY).inserted,
        1
    );
}

#[test]
fn eviction_drops_the_heaviest_and_spares_prioritised_destinations() {
    let old_small = entry(0x62, 1, 10, 0);
    let new_large = entry(0x62, 2, 400, 0);
    let prioritised_large = entry(0x63, 3, 400, 0);
    let mut store = PropagationStore::new(limits(2));
    store.prioritise(owner(0x63));
    store.ingest(&batch(&[&old_small]), 0.0);
    store.ingest(&batch(&[&prioritised_large]), 20.0 * DAY);
    // Five four-day units of age make the small entry outweigh a fresh large one.
    let receipt = store.ingest(&batch(&[&new_large]), 20.0 * DAY);
    assert_eq!((receipt.inserted, receipt.evicted), (1, 1));
    assert!(store.offer(owner(0x62)) == vec![new_large.transient_id()]);
    assert_eq!(
        store.offer(owner(0x63)),
        vec![prioritised_large.transient_id()]
    );
}

#[test]
fn snapshot_v2_keeps_stamps_and_processed_ids_and_v1_still_restores() {
    let stamped = entry(0x62, 1, 10, 4);
    let value = stamped.stamp_value();
    let gone = entry(0x62, 2, 10, 0);
    let mut store = PropagationStore::new(limits(8));
    store.ingest(&batch(&[&stamped, &gone]), 5.0);
    store.acknowledge(owner(0x62), &[gone.transient_id()]);

    let snapshot = store.encode_snapshot().unwrap();
    let (restored, receipt) = PropagationStore::restore(limits(8), &snapshot, 6.0).unwrap();
    assert_eq!(receipt.loaded, 1);
    assert_eq!(restored.stamp_value(&stamped.transient_id()), Some(value));
    assert_eq!(
        restored.stamped_entry(&stamped.transient_id()),
        Some(stamped.clone())
    );
    assert!(restored.has_processed(&gone.transient_id()));
    assert_eq!(restored.encode_snapshot().unwrap(), snapshot);

    let v1 = encode_value(&Value::Array(vec![
        Value::Binary(b"outrider-propagation-store".to_vec()),
        Value::from(1),
        Value::Array(vec![Value::Array(vec![
            Value::F64(5.0),
            Value::Binary(stamped.message().encode()),
        ])]),
    ]))
    .unwrap();
    let (restored, receipt) = PropagationStore::restore(limits(8), &v1, 6.0).unwrap();
    assert_eq!(receipt.loaded, 1);
    assert_eq!(restored.stamp_value(&stamped.transient_id()), Some(0));
    assert_eq!(restored.stamped_entry(&stamped.transient_id()), None);
    assert!(restored.has_processed(&stamped.transient_id()));
    let Value::Array(parts) = decode_one(&restored.encode_snapshot().unwrap()).unwrap() else {
        unreachable!()
    };
    assert_eq!(parts[1], Value::from(2));
}

#[test]
fn restore_is_bounded_by_the_limits_not_a_fixed_count() {
    let entries: Vec<_> = (0..6).map(|index| entry(0x62, index, 4, 0)).collect();
    let mut store = PropagationStore::new(limits(6));
    store.ingest(&batch(&entries.iter().collect::<Vec<_>>()), 1.0);
    let snapshot = store.encode_snapshot().unwrap();
    let (restored, receipt) = PropagationStore::restore(limits(4), &snapshot, 2.0).unwrap();
    assert_eq!((restored.len(), receipt.loaded, receipt.evicted), (4, 6, 2));
}

fn node_policy(propagation: u8, flexibility: u8) -> NodePolicy {
    NodePolicy {
        costs: PropagationCosts {
            propagation,
            flexibility,
            peering: 18,
        },
        max_transfer_bytes: 1 << 16,
        allowed: None,
        link_idle: LINK_MAX_INACTIVITY,
    }
}

#[test]
fn stamps_are_admitted_from_cost_minus_flexibility_and_each_entry_stands_alone() {
    let policy = node_policy(8, 3);
    assert_eq!(policy.stamp_floor(), 5);
    assert_eq!(node_policy(2, 3).stamp_floor(), 0);
    let good = entry(0x62, 1, 10, 5);
    let mut bad = entry(0x62, 2, 10, 0);
    while bad.stamp_value() >= 5 {
        bad.stamp[0] = bad.stamp[0].wrapping_add(1);
    }
    let (valid, invalid) = score_stamps(vec![bad, good.clone()], policy.stamp_floor());
    assert_eq!(invalid, 1);
    assert_eq!(valid, vec![(good.clone(), good.stamp_value())]);
}

#[test]
fn the_allow_list_and_the_throttle_window() {
    let mut policy = node_policy(0, 0);
    assert!(policy.allows(&[1; 16]));
    policy.allowed = Some([[1; 16]].into());
    assert!(policy.allows(&[1; 16]) && !policy.allows(&[2; 16]));

    let node = PropagationNode::new(PropagationStore::new(limits(1)), policy);
    node.throttle([2; 16], 100.0);
    assert!(node.is_throttled(&[2; 16], 279.0));
    assert!(!node.is_throttled(&[2; 16], 280.0));
}

#[test]
fn the_submission_limit_is_exactly_what_the_store_admits() {
    let fits = entry(0x62, 1, 300, 0);
    let message_bytes = fits.message().encode().len();
    let packed = |entry: &PropagationEntry| batch(&[entry]).encode().unwrap().len();
    for max_message_bytes in [message_bytes, message_bytes - 1] {
        let limits = PropagationStoreLimits {
            max_message_bytes,
            ..limits(8)
        };
        let admitted = PropagationStore::new(limits.clone())
            .ingest(&batch(&[&fits]), 1.0)
            .inserted
            == 1;
        assert_eq!(packed(&fits) <= limits.max_submission_bytes(), admitted);
        assert_eq!(
            limits.announced_limit_kb(),
            limits.max_submission_bytes().div_ceil(1_000) as f64
        );
    }
    // Total capacity bounds it too.
    let tight = PropagationStoreLimits {
        max_bytes: message_bytes + STAMP_LEN - 1,
        ..limits(8)
    };
    assert!(packed(&fits) > tight.max_submission_bytes());
}

#[test]
fn a_message_refused_as_too_large_is_not_remembered_as_processed() {
    let large = entry(0x62, 1, 300, 0);
    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_message_bytes: 100,
        ..limits(8)
    });
    assert_eq!(store.ingest(&batch(&[&large]), 1.0).rejected_too_large, 1);
    assert!(!store.has_processed(&large.transient_id()));
}

#[test]
fn prune_drops_only_expired_entries_and_ids_oldest_first() {
    let entries: Vec<_> = (0..3).map(|index| entry(0x62, index, 4, 0)).collect();
    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_age: Duration::from_secs(25),
        ..limits(8)
    });
    for (at, entry) in [0.0, 10.0, 20.0].into_iter().zip(&entries) {
        store.ingest(&batch(&[entry]), at);
    }
    assert_eq!(store.prune(36.0), 2);
    assert_eq!(store.offer(owner(0x62)), vec![entries[2].transient_id()]);
    // Processed ids outlive the entries, then go oldest first.
    let day = |days: f64| days * DAY;
    store.prune(day(180.0) + 5.0);
    assert!(!store.has_processed(&entries[0].transient_id()));
    assert!(store.has_processed(&entries[1].transient_id()));
}

#[test]
fn served_keeps_the_last_message_response_and_counts_them_all() {
    let (first, second) = (entry(0x62, 1, 10, 0), entry(0x62, 2, 10, 0));
    let node = PropagationNode::new(PropagationStore::new(limits(8)), node_policy(0, 0));
    node.store().ingest(&batch(&[&first, &second]), 1.0);
    let mut report = ServedFetch::default();
    let fetch = |wanted: Vec<[u8; 32]>| GetRequest::Fetch {
        wanted,
        handled: Vec::new(),
        limit_kb: None,
    };
    for _ in 0..3 {
        node.answer_get(
            owner(0x62),
            fetch(vec![first.transient_id()]),
            2.0,
            &mut report,
        );
    }
    node.answer_get(
        owner(0x62),
        fetch(vec![second.transient_id()]),
        2.0,
        &mut report,
    );
    // An acknowledgement-only request leaves the record alone.
    node.answer_get(owner(0x62), fetch(Vec::new()), 2.0, &mut report);
    assert_eq!(report.served, vec![second.transient_id()]);
    assert_eq!(report.served_total, 4);
}
