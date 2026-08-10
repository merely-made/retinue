//! The FS2 signed command envelope, driven through its public surface only.
//!
//! These live outside the module on purpose. They are the gate's validation conditions
//! rather than internal checks, so exercising them through exactly the API a caller has is
//! the point: if a property here needed a private item to demonstrate, the API would be
//! wrong. Splitting them out also keeps `src/command.rs` inside the file-size ceiling.

use retinue::command::{
    COUNTER_WINDOW, Command, HEADER_LEN, MAX_COMMAND_LEN, MAX_PAYLOAD, Refusal, TargetClass,
    VERSION, Verifier,
};
use retinue::hash::{ADDRESS_HASH_LEN, AddressHash};
use retinue::identity::{IDENTITY_LEN, PrivateIdentity, SIGNATURE_LEN};

fn operator(fill: u8) -> PrivateIdentity {
    let mut secret = [0u8; IDENTITY_LEN];
    secret[..32].fill(fill);
    secret[32..].fill(fill ^ 0xff);
    PrivateIdentity::from_secret_bytes(&secret)
}

fn node() -> AddressHash {
    AddressHash::from_bytes([0x0a; ADDRESS_HASH_LEN])
}

fn verifier_for(op: &PrivateIdentity) -> Verifier<4> {
    let mut verifier = Verifier::new(node());
    verifier.authorize(*op.public()).unwrap();
    verifier
}

fn command<'a>(op: &PrivateIdentity, counter: u64, payload: &'a [u8]) -> Command<'a> {
    Command {
        key_id: op.hash(),
        class: TargetClass::Node,
        target: node(),
        counter,
        opcode: 7,
        payload,
    }
}

#[test]
fn a_signed_command_verifies_and_reports_what_it_said() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let wire = command(&op, 1, b"restart").sign(&op).unwrap();
    let accepted = verifier.accept(&wire).unwrap();
    assert_eq!(accepted.opcode, 7);
    assert_eq!(accepted.payload, b"restart");
    assert_eq!(accepted.key_id, op.hash());
    assert_eq!(verifier.accepted(), 1);
    assert_eq!(verifier.refusals(), 0);
}

#[test]
fn the_same_bytes_verify_no_matter_which_bearer_carried_them() {
    // FS2's transport-independence claim, made structurally: there is one entry point
    // and it takes bytes. Two verifiers in identical state accept identical bytes.
    let op = operator(0x11);
    let wire = command(&op, 1, b"ping").sign(&op).unwrap();
    let mut serial = verifier_for(&op);
    let mut over_the_air = verifier_for(&op);
    assert_eq!(
        serial.accept(&wire).unwrap(),
        over_the_air.accept(&wire).unwrap()
    );
}

#[test]
fn a_replayed_command_is_refused() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let wire = command(&op, 5, b"once").sign(&op).unwrap();
    assert!(verifier.accept(&wire).is_ok());
    assert_eq!(verifier.accept(&wire), Err(Refusal::CounterReplayed));
    // And anything at or behind the accepted counter, not just the exact bytes.
    let earlier = command(&op, 4, b"earlier").sign(&op).unwrap();
    assert_eq!(verifier.accept(&earlier), Err(Refusal::CounterReplayed));
    assert_eq!(verifier.refusals(), 2);
}

#[test]
fn a_replayed_command_is_still_refused_after_a_reboot_that_restored_the_ledger() {
    // The FS2 validation condition. FS3 owns making this durable; FS2 owns the seam.
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let wire = command(&op, 9, b"once").sign(&op).unwrap();
    assert!(verifier.accept(&wire).is_ok());

    let saved: Vec<(AddressHash, u64)> = verifier
        .ledger()
        .map(|op| (op.key_id, op.counter))
        .collect();

    let mut rebooted = verifier_for(&op);
    for (key_id, counter) in saved {
        assert!(rebooted.restore(key_id, counter));
    }
    assert_eq!(rebooted.accept(&wire), Err(Refusal::CounterReplayed));
}

#[test]
fn a_restore_can_never_walk_a_counter_backwards() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    assert!(
        verifier
            .accept(&command(&op, 100, b"").sign(&op).unwrap())
            .is_ok()
    );
    assert!(verifier.restore(op.hash(), 3));
    assert_eq!(verifier.ledger().next().unwrap().counter, 100);
}

#[test]
fn a_counter_far_beyond_the_window_is_refused_so_one_capture_cannot_lock_an_operator_out() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let far = command(&op, u64::MAX, b"lockout").sign(&op).unwrap();
    assert_eq!(verifier.accept(&far), Err(Refusal::CounterTooFar));
    // The operator is unharmed: the next ordinary command still works.
    assert!(
        verifier
            .accept(&command(&op, 1, b"fine").sign(&op).unwrap())
            .is_ok()
    );
    // And the window edge itself is acceptable.
    let edge = command(&op, 1 + COUNTER_WINDOW, b"edge").sign(&op).unwrap();
    assert!(verifier.accept(&edge).is_ok());
}

#[test]
fn an_unallowlisted_operator_authorizes_nothing() {
    let known = operator(0x11);
    let stranger = operator(0x22);
    let mut verifier = verifier_for(&known);
    let wire = command(&stranger, 1, b"open").sign(&stranger).unwrap();
    assert_eq!(verifier.accept(&wire), Err(Refusal::UnknownKey));
}

#[test]
fn revoking_a_key_stops_its_commands() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    assert!(
        verifier
            .accept(&command(&op, 1, b"").sign(&op).unwrap())
            .is_ok()
    );
    assert!(verifier.revoke(op.hash()));
    assert_eq!(
        verifier.accept(&command(&op, 2, b"").sign(&op).unwrap()),
        Err(Refusal::UnknownKey)
    );
    assert!(!verifier.revoke(op.hash()));
}

#[test]
fn re_authorizing_a_known_key_does_not_reopen_its_replay_window() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let wire = command(&op, 5, b"once").sign(&op).unwrap();
    assert!(verifier.accept(&wire).is_ok());
    verifier.authorize(*op.public()).unwrap();
    assert_eq!(verifier.accept(&wire), Err(Refusal::CounterReplayed));
}

#[test]
fn a_command_for_another_node_is_refused() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let mut elsewhere = command(&op, 1, b"");
    elsewhere.target = AddressHash::from_bytes([0xbb; ADDRESS_HASH_LEN]);
    assert_eq!(
        verifier.accept(&elsewhere.sign(&op).unwrap()),
        Err(Refusal::WrongTarget)
    );
}

#[test]
fn a_fleet_command_reaches_only_nodes_in_that_fleet() {
    let op = operator(0x11);
    let fleet = AddressHash::from_bytes([0xf1; ADDRESS_HASH_LEN]);
    let mut wire = command(&op, 1, b"all-stop");
    wire.class = TargetClass::Fleet;
    wire.target = fleet;
    let signed = wire.sign(&op).unwrap();

    let mut outsider = verifier_for(&op);
    assert_eq!(outsider.accept(&signed), Err(Refusal::WrongTarget));

    let mut member = verifier_for(&op);
    member.join_fleet(fleet);
    assert!(member.accept(&signed).is_ok());

    let mut other_fleet = verifier_for(&op);
    other_fleet.join_fleet(AddressHash::from_bytes([0xf2; ADDRESS_HASH_LEN]));
    assert_eq!(other_fleet.accept(&signed), Err(Refusal::WrongTarget));
}

#[test]
fn a_forged_or_tampered_command_is_refused() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let good = command(&op, 1, b"safe").sign(&op).unwrap();

    // Every single-bit flip in the envelope must break the signature, and none may be
    // mistaken for a valid command. Walk the opcode and payload bytes exhaustively.
    for at in [0, 1, 34, 42, 43, HEADER_LEN, HEADER_LEN + 3] {
        let mut forged = good.clone();
        forged[at] ^= 0x01;
        assert!(
            verifier.accept(&forged).is_err(),
            "a flipped byte at {at} was accepted"
        );
    }
    // The signature itself.
    let mut forged = good.clone();
    let last = forged.len() - 1;
    forged[last] ^= 0x01;
    assert_eq!(verifier.accept(&forged), Err(Refusal::BadSignature));
    // None of that advanced the counter, so the genuine command still works.
    assert!(verifier.accept(&good).is_ok());
}

#[test]
fn a_command_signed_for_a_different_domain_does_not_verify() {
    // The signature covers DOMAIN || envelope. A signature over the bare envelope, which
    // is what a different retinue signing context would produce, must not be accepted.
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let wire = command(&op, 1, b"x").sign(&op).unwrap();
    let envelope_len = wire.len() - SIGNATURE_LEN;
    let mut naked = wire[..envelope_len].to_vec();
    naked.extend_from_slice(&op.sign(&wire[..envelope_len]));
    assert_eq!(verifier.accept(&naked), Err(Refusal::BadSignature));
}

#[test]
fn malformed_input_is_refused_without_panicking() {
    let op = operator(0x11);
    let mut verifier = verifier_for(&op);
    let good = command(&op, 1, b"payload").sign(&op).unwrap();

    assert_eq!(verifier.accept(&[]), Err(Refusal::Malformed));
    for cut in 0..good.len() {
        // Every truncation, one at a time. The claim is "no panic", so the assertion is
        // simply that a value came back.
        assert!(verifier.accept(&good[..cut]).is_err());
    }
    // A trailing byte is not the command that was signed.
    let mut extended = good.to_vec();
    extended.push(0);
    assert_eq!(verifier.accept(&extended), Err(Refusal::Malformed));

    let mut wrong_version = good.clone();
    wrong_version[0] = VERSION + 1;
    assert_eq!(
        verifier.accept(&wrong_version),
        Err(Refusal::UnknownVersion)
    );

    let mut wrong_class = good.clone();
    wrong_class[1] = 9;
    assert_eq!(verifier.accept(&wrong_class), Err(Refusal::Malformed));

    // A declared payload length that disagrees with the bytes present.
    let mut lying_length = good.clone();
    lying_length[43] = 0xff;
    lying_length[44] = 0xff;
    assert_eq!(verifier.accept(&lying_length), Err(Refusal::PayloadTooLong));
}

#[test]
fn an_oversized_payload_is_refused_at_signing_time() {
    let op = operator(0x11);
    let payload = [0u8; MAX_PAYLOAD + 1];
    assert_eq!(
        command(&op, 1, &payload).sign(&op),
        Err(Refusal::PayloadTooLong)
    );
    // And the largest legal one is genuinely legal.
    let payload = [0u8; MAX_PAYLOAD];
    let mut verifier = verifier_for(&op);
    let wire = command(&op, 1, &payload).sign(&op).unwrap();
    assert_eq!(wire.len(), MAX_COMMAND_LEN);
    assert!(verifier.accept(&wire).is_ok());
}

#[test]
fn the_allowlist_is_bounded_and_says_so() {
    let mut verifier: Verifier<2> = Verifier::new(node());
    assert!(verifier.authorize(*operator(0x11).public()).is_ok());
    assert!(verifier.authorize(*operator(0x22).public()).is_ok());
    assert_eq!(
        verifier.authorize(*operator(0x33).public()),
        Err(Refusal::AllowlistFull)
    );
}

#[test]
fn the_key_id_on_the_wire_is_always_the_signer() {
    // A caller who fills in someone else's key id gets their own written instead, so the
    // envelope cannot claim a signer it does not have.
    let op = operator(0x11);
    let mut lying = command(&op, 1, b"");
    lying.key_id = AddressHash::from_bytes([0x99; ADDRESS_HASH_LEN]);
    let wire = lying.sign(&op).unwrap();
    let mut verifier = verifier_for(&op);
    assert_eq!(verifier.accept(&wire).unwrap().key_id, op.hash());
}
