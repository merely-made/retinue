use std::time::Duration;

use retinue::identity::{Identity, PrivateIdentity};
use retinue::token::{IV_LEN, encrypt_to_identity};
use rmpv::Value;

use super::msgpack::{
    decode_entry_response, decode_fetch_selection, decode_id_response, decode_offer_request,
    decode_one, encode_value,
};
use super::*;
use crate::announce::delivery_destination;
use crate::codec::LxmfPayload;
use crate::stamp::STAMP_LEN;

/// Prepare to the recipient's identity key with a fixed ephemeral key and IV.
fn sealed(
    sender: &PrivateIdentity,
    recipient: &Identity,
    payload: &LxmfPayload,
    key: u8,
    propagation_cost: u16,
    max_attempts: u64,
) -> Result<PreparedPropagation, PropagationError> {
    let stamps = PropagationStamps {
        delivery_cost: None,
        propagation_cost,
        seed: [0; STAMP_LEN],
        max_attempts,
    };
    prepare_propagation_with(
        sender,
        delivery_destination(recipient),
        payload,
        &stamps,
        |plaintext| {
            let iv = [key.wrapping_add(0x10); IV_LEN];
            Ok((
                encrypt_to_identity(recipient, &[key; 32], &iv, plaintext),
                None,
            ))
        },
    )
}

fn captured_announce() -> Vec<u8> {
    hex::decode(
        "97c2ce6a680a26c3cd0100cd2800930d03088101c41853746f636b2050726f7061676174696f6e204f7261636c65",
    )
    .unwrap()
}

#[test]
fn stock_announce_decodes_and_round_trips_exactly() {
    let bytes = captured_announce();
    let announce = PropagationAnnounce::decode(&bytes).unwrap();
    assert!(!announce.legacy);
    assert!(announce.active);
    assert_eq!(announce.transfer_limit_kib, 256);
    assert_eq!(announce.sync_limit_kib, 10_240);
    assert_eq!(
        announce.costs,
        PropagationCosts {
            propagation: 13,
            flexibility: 3,
            peering: 8
        }
    );
    assert_eq!(
        announce.name(),
        Some(b"Stock Propagation Oracle".as_slice())
    );
    assert_eq!(announce.encode().unwrap(), bytes);
}

#[test]
fn prepared_entry_decrypts_and_authenticates() {
    let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let payload = LxmfPayload::text(1_753_603_204.5, b"TITLE", b"BODY");
    let prepared = sealed(&sender, recipient.public(), &payload, 0x31, 8, 100_000).unwrap();
    assert_eq!(prepared.transient_id, prepared.entry.transient_id());
    assert!(prepared.entry.validate_stamp(8));
    let decoded = prepared
        .entry
        .decrypt_and_verify(&recipient, sender.public(), 4_096)
        .unwrap();
    assert_eq!(decoded.message_id, prepared.message_id);
    assert_eq!(decoded.payload.title, b"TITLE");
    assert_eq!(decoded.payload.content, b"BODY");
}

#[test]
fn delivery_cost_mints_a_stamp_on_the_message_id_inside_the_encryption() {
    let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let payload = LxmfPayload::text(1_753_603_204.5, b"TITLE", b"BODY");
    let unstamped = sealed(&sender, recipient.public(), &payload, 0x31, 0, 1).unwrap();
    let ratchet = retinue::hash::NameHash::of(&[0x77; 32]);
    let stamps = PropagationStamps {
        delivery_cost: Some(8),
        propagation_cost: 0,
        seed: [0; STAMP_LEN],
        max_attempts: 100_000,
    };
    let stamped = prepare_propagation_with(
        &sender,
        delivery_destination(recipient.public()),
        &payload,
        &stamps,
        |plaintext| {
            let token =
                encrypt_to_identity(recipient.public(), &[0x31; 32], &[0x41; 16], plaintext);
            Ok((token, Some(ratchet)))
        },
    )
    .unwrap();
    assert_eq!(stamped.ratchet_id, Some(ratchet));
    assert_eq!(stamped.message_id, unstamped.message_id);
    let decoded = stamped.entry.decrypt(&recipient, 4_096).unwrap();
    assert!(delivery_stamp_valid(&decoded, 8));
    let plain = unstamped.entry.decrypt(&recipient, 4_096).unwrap();
    assert!(plain.payload.stamp.is_none());
    assert!(!delivery_stamp_valid(&plain, 8));

    let exhausted = PropagationStamps {
        delivery_cost: Some(32),
        max_attempts: 1,
        ..stamps
    };
    assert!(matches!(
        prepare_propagation_with(
            &sender,
            delivery_destination(recipient.public()),
            &payload,
            &exhausted,
            |_| unreachable!("no stamp, no encryption"),
        ),
        Err(PropagationError::StampBudgetExhausted)
    ));
}

#[test]
fn node_error_codes_are_typed() {
    let response = |code: u64| {
        encode_value(&Value::Array(vec![
            Value::Binary(vec![0; 16]),
            Value::from(code),
        ]))
        .unwrap()
    };
    assert!(matches!(
        decode_id_response(&response(0xf0)),
        Err(PropagationError::NoIdentity)
    ));
    assert!(matches!(
        decode_entry_response(&response(0xf1)),
        Err(PropagationError::NoAccess)
    ));
    assert!(matches!(
        decode_id_response(&response(0xf2)),
        Err(PropagationError::InvalidFetchResponse)
    ));
}

#[test]
fn batch_is_one_timestamp_and_binary_entry_list() {
    let entry = PropagationEntry {
        message: PropagationMessage {
            destination: [1; 16],
            encrypted: vec![2; MIN_ENCRYPTED_MESSAGE_BYTES],
        },
        stamp: [3; STAMP_LEN],
    };
    let batch = PropagationBatch {
        transfer_time: 1_753_603_204.5,
        entries: vec![entry],
    };
    let encoded = batch.encode().unwrap();
    assert_eq!(
        PropagationBatch::decode(&encoded, encoded.len(), 1).unwrap(),
        batch
    );
}

#[test]
fn store_is_bounded_expires_and_acknowledges_by_owner() {
    let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let first = sealed(
        &sender,
        recipient.public(),
        &LxmfPayload::text(1.0, b"A", b"one"),
        0x31,
        0,
        1,
    )
    .unwrap();
    let second = sealed(
        &sender,
        recipient.public(),
        &LxmfPayload::text(2.0, b"B", b"two"),
        0x32,
        0,
        1,
    )
    .unwrap();
    let mut store = PropagationStore::new(PropagationStoreLimits {
        max_entries: 1,
        max_bytes: 1_024,
        max_message_bytes: 512,
        max_age: Duration::from_secs(10),
        max_per_fetch: 1,
    });
    let batch = PropagationBatch {
        transfer_time: 3.0,
        entries: vec![first.entry, second.entry],
    };
    let receipt = store.ingest(&batch, 3.0);
    assert_eq!(receipt.inserted, 2);
    assert_eq!(receipt.evicted, 1);
    assert_eq!(
        store.offer(
            *delivery_destination(recipient.public()).as_bytes(),
            usize::MAX
        ),
        vec![second.transient_id]
    );
    assert_eq!(
        store.acknowledge(
            *delivery_destination(recipient.public()).as_bytes(),
            &[second.transient_id]
        ),
        1
    );
    assert!(store.is_empty());

    let later = PropagationBatch {
        transfer_time: 4.0,
        entries: vec![
            sealed(
                &sender,
                recipient.public(),
                &LxmfPayload::text(4.0, b"C", b"three"),
                0x33,
                0,
                1,
            )
            .unwrap()
            .entry,
        ],
    };
    store.ingest(&later, 4.0);
    assert_eq!(store.prune(15.0), 1);
    assert!(store.is_empty());
}

#[test]
fn store_snapshot_round_trip_rederives_ids_bytes_and_owner_scope() {
    let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let first_recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let second_recipient = PrivateIdentity::from_secret_bytes(&[0x63; 64]);
    let first = sealed(
        &sender,
        first_recipient.public(),
        &LxmfPayload::text(10.0, b"A", b"first"),
        0x31,
        0,
        1,
    )
    .unwrap();
    let second = sealed(
        &sender,
        second_recipient.public(),
        &LxmfPayload::text(11.0, b"B", b"second"),
        0x32,
        0,
        1,
    )
    .unwrap();
    let limits = PropagationStoreLimits {
        max_entries: 4,
        max_bytes: 4_096,
        max_message_bytes: 1_024,
        max_age: Duration::from_secs(60),
        max_per_fetch: 4,
    };
    let mut store = PropagationStore::new(limits.clone());
    assert_eq!(
        store
            .ingest(
                &PropagationBatch {
                    transfer_time: 12.0,
                    entries: vec![first.entry.clone()],
                },
                12.0,
            )
            .inserted,
        1
    );
    assert_eq!(
        store
            .ingest(
                &PropagationBatch {
                    transfer_time: 13.0,
                    entries: vec![second.entry.clone()],
                },
                13.0,
            )
            .inserted,
        1
    );
    let original_bytes = store.bytes();
    let snapshot = store.encode_snapshot().unwrap();
    let (mut restored, receipt) = PropagationStore::restore(limits, &snapshot, 14.0).unwrap();

    assert_eq!(
        receipt,
        StoreRestoreReceipt {
            loaded: 2,
            ..StoreRestoreReceipt::default()
        }
    );
    assert_eq!(restored.bytes(), original_bytes);
    assert_eq!(restored.encode_snapshot().unwrap(), snapshot);
    assert_eq!(
        restored.offer(
            *delivery_destination(first_recipient.public()).as_bytes(),
            usize::MAX,
        ),
        vec![first.transient_id]
    );
    assert_eq!(
        restored.offer(
            *delivery_destination(second_recipient.public()).as_bytes(),
            usize::MAX,
        ),
        vec![second.transient_id]
    );
    assert_eq!(
        restored
            .ingest(
                &PropagationBatch {
                    transfer_time: 15.0,
                    entries: vec![first.entry],
                },
                15.0,
            )
            .duplicates,
        1
    );
}

#[test]
fn restore_reapplies_expiry_and_current_capacity_limits() {
    let sender = PrivateIdentity::from_secret_bytes(&[0x61; 64]);
    let recipient = PrivateIdentity::from_secret_bytes(&[0x62; 64]);
    let prepared: Vec<_> = (0..3_u8)
        .map(|index| {
            sealed(
                &sender,
                recipient.public(),
                &LxmfPayload::text(f64::from(index), [index], [index; 8]),
                0x31 + index,
                0,
                1,
            )
            .unwrap()
        })
        .collect();
    let original_limits = PropagationStoreLimits {
        max_entries: 3,
        max_bytes: 4_096,
        max_message_bytes: 1_024,
        max_age: Duration::from_secs(60),
        max_per_fetch: 3,
    };
    let mut original = PropagationStore::new(original_limits);
    for (index, entry) in prepared.iter().enumerate() {
        original.ingest(
            &PropagationBatch {
                transfer_time: index as f64,
                entries: vec![entry.entry.clone()],
            },
            [1.0, 5.0, 9.0][index],
        );
    }

    let limits = PropagationStoreLimits {
        max_entries: 1,
        max_bytes: 1_024,
        max_message_bytes: 512,
        max_age: Duration::from_secs(5),
        max_per_fetch: 1,
    };
    let snapshot = original.encode_snapshot().unwrap();
    let (restored, receipt) = PropagationStore::restore(limits, &snapshot, 10.0).unwrap();
    assert_eq!(
        receipt,
        StoreRestoreReceipt {
            loaded: 2,
            expired: 1,
            evicted: 1,
            ..StoreRestoreReceipt::default()
        }
    );
    assert_eq!(restored.len(), 1);
    assert_eq!(
        restored.offer(
            *delivery_destination(recipient.public()).as_bytes(),
            usize::MAX,
        ),
        vec![prepared[2].transient_id]
    );

    let small_message_limit = PropagationStoreLimits {
        max_entries: 3,
        max_bytes: 4_096,
        max_message_bytes: 64,
        max_age: Duration::from_secs(60),
        max_per_fetch: 3,
    };
    let (restored, receipt) =
        PropagationStore::restore(small_message_limit, &snapshot, 10.0).unwrap();
    assert_eq!(receipt.rejected_too_large, 3);
    assert!(restored.is_empty());
}

#[test]
fn corrupt_or_unknown_store_snapshots_are_rejected_atomically() {
    let limits = PropagationStoreLimits::default();
    let empty = PropagationStore::new(limits.clone())
        .encode_snapshot()
        .unwrap();
    let mut wrong_magic = decode_one(&empty).unwrap();
    let Value::Array(parts) = &mut wrong_magic else {
        unreachable!()
    };
    parts[0] = Value::Binary(b"not-outrider".to_vec());
    assert!(matches!(
        PropagationStore::restore(limits.clone(), &encode_value(&wrong_magic).unwrap(), 1.0),
        Err(PropagationError::InvalidStoreSnapshot)
    ));

    let mut wrong_version = decode_one(&empty).unwrap();
    let Value::Array(parts) = &mut wrong_version else {
        unreachable!()
    };
    parts[1] = Value::from(2);
    assert!(matches!(
        PropagationStore::restore(limits.clone(), &encode_value(&wrong_version).unwrap(), 1.0),
        Err(PropagationError::UnsupportedStoreSnapshotVersion(2))
    ));

    let mut trailing = empty;
    trailing.push(0);
    assert!(matches!(
        PropagationStore::restore(limits, &trailing, 1.0),
        Err(PropagationError::MalformedMessagePack)
    ));

    assert!(matches!(
        PropagationStore::restore_bounded(
            PropagationStoreLimits::default(),
            &trailing,
            trailing.len() - 1,
            1.0,
        ),
        Err(PropagationError::StoreSnapshotTooLarge)
    ));
}

#[test]
fn captured_fetch_requests_decode_to_offer_and_selection() {
    let offer =
        hex::decode("93cb41da9a05b2533e92c4109dc1a72883468f57fed571e796e9ce9892c0c0").unwrap();
    decode_offer_request(&offer).unwrap();

    let followup = hex::decode(
        "93cb41da9a060120c7b3c4109dc1a72883468f57fed571e796e9ce989391c420444444444444444444444444444444444444444444444444444444444444444490cd03e8",
    )
    .unwrap();
    let (wanted, handled, limit) = decode_fetch_selection(&followup).unwrap();
    assert_eq!(wanted, vec![[0x44; 32]]);
    assert!(handled.is_empty());
    assert_eq!(limit, FETCH_LIMIT);
}
