use outrider::{PropagationAnnounce, PropagationBatch};
use retinue::identity::PrivateIdentity;
use serde::Deserialize;

#[derive(Deserialize)]
struct Fixture {
    sender_seed_hex: String,
    recipient_seed_hex: String,
    message_id_hex: String,
    transient_id_hex: String,
    stamp_hex: String,
    stamp_value: u16,
    submission_hex: String,
    announce_hex: String,
    offer_request_hex: String,
    selection_request_hex: String,
}

fn bytes<const N: usize>(hex_value: &str) -> [u8; N] {
    hex::decode(hex_value).unwrap().try_into().unwrap()
}

#[test]
fn stock_propagation_fixture_replays_without_python() {
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/lxmf_0_9_6_propagation.json")).unwrap();
    PropagationAnnounce::decode(&hex::decode(&fixture.announce_hex).unwrap()).unwrap();
    let submission = hex::decode(&fixture.submission_hex).unwrap();
    let batch = PropagationBatch::decode(&submission, submission.len(), 1).unwrap();
    assert_eq!(batch.entries.len(), 1);
    let entry = &batch.entries[0];
    assert_eq!(entry.transient_id(), bytes::<32>(&fixture.transient_id_hex));
    assert_eq!(entry.stamp(), &bytes::<32>(&fixture.stamp_hex));
    assert_eq!(entry.stamp_value(), fixture.stamp_value);

    let recipient = PrivateIdentity::from_secret_bytes(&bytes::<64>(&fixture.recipient_seed_hex));
    let sender = PrivateIdentity::from_secret_bytes(&bytes::<64>(&fixture.sender_seed_hex));
    let message = entry
        .decrypt_and_verify(&recipient, sender.public(), 4_096)
        .unwrap();
    assert_eq!(message.message_id, bytes::<32>(&fixture.message_id_hex));
    assert_eq!(message.payload.title, b"PROPAGATION TITLE");
    assert_eq!(message.payload.content, b"PROPAGATION BODY");

    // These captures are also replayed by module tests that pin their exact
    // offer and selection grammar.
    assert!(!fixture.offer_request_hex.is_empty());
    assert!(!fixture.selection_request_hex.is_empty());
}
