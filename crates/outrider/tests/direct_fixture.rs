use outrider::{DeliveryAnnounce, LxmfPayload, decode, prepare};
use retinue::identity::{Identity, PrivateIdentity};
use serde_json::Value;
use sha2::{Digest, Sha256};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/lxmf_0_9_6_direct.json")).unwrap()
}

fn bytes(value: &Value, field: &str) -> Vec<u8> {
    hex::decode(value[field].as_str().unwrap()).unwrap()
}

fn array<const N: usize>(value: &Value, field: &str) -> [u8; N] {
    bytes(value, field).try_into().unwrap()
}

fn large_content() -> Vec<u8> {
    (0..128_u32)
        .flat_map(|value| Sha256::digest(value.to_be_bytes()).to_vec())
        .collect()
}

#[test]
fn direct_capture_replays_without_stock_python() {
    let fixture = fixture();
    let stock = &fixture["stock_to_outrider_small"];
    let stock_identity =
        Identity::from_public_bytes(&array(stock, "sender_public_identity")).unwrap();
    let stock_message = decode(&bytes(stock, "packed")).unwrap();
    assert_eq!(stock_message.message_id, array(stock, "message_id"));
    assert!(
        stock_message
            .verify_with(|message, signature| { stock_identity.verify(message, signature) })
    );
    assert_eq!(
        DeliveryAnnounce::decode(&bytes(stock, "sender_announce_app_data")).unwrap(),
        DeliveryAnnounce {
            display_name: Some(b"Stock Oracle".to_vec()),
            stamp_cost: Some(8),
        }
    );

    let receiver = &fixture["outrider_to_stock_small"];
    let sender = PrivateIdentity::from_secret_bytes(&[0x55; 64]);
    let receiver_message = decode(&bytes(receiver, "packed")).unwrap();
    assert_eq!(receiver_message.message_id, array(receiver, "message_id"));
    assert!(
        receiver_message
            .verify_with(|message, signature| { sender.public().verify(message, signature) })
    );

    let stock_large = &fixture["stock_to_outrider_large"];
    let prepared = prepare(
        array(stock, "destination"),
        array(stock, "source"),
        &LxmfPayload::text(1_753_603_201.5, b"STOCK LARGE TITLE", large_content()),
    )
    .unwrap();
    let packed = prepared
        .clone()
        .finish(PrivateIdentity::from_secret_bytes(&[0x77; 64]).sign(prepared.signing_bytes()));
    assert_eq!(prepared.message_id, array(stock_large, "message_id"));
    assert_eq!(
        packed.len(),
        stock_large["packed_len"].as_u64().unwrap() as usize
    );

    let outrider_large = &fixture["outrider_to_stock_large"];
    let prepared = prepare(
        array(receiver, "destination"),
        array(receiver, "source"),
        &LxmfPayload::text(1_753_603_202.5, b"OUTRIDER LARGE TITLE", large_content()),
    )
    .unwrap();
    let packed = prepared
        .clone()
        .finish(sender.sign(prepared.signing_bytes()));
    assert_eq!(prepared.message_id, array(outrider_large, "message_id"));
    assert_eq!(
        packed.len(),
        outrider_large["packed_len"].as_u64().unwrap() as usize
    );
}
