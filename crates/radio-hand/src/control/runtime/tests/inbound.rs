use super::fakes::*;
use crate::control::*;
use futures::executor::block_on;
use sha2::{Digest, Sha256};

#[test]
#[allow(unsafe_code)]
fn inbound_helpers_and_malformed_verified_command_counter_path() {
    let t = Trace::default();
    let operator = operator();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state_for_operator(&operator));
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    // SAFETY: this fixture starts at a fresh simulated board boot.
    let mut r = unsafe {
        ControlRuntime::new_after_hardware_reset(NodeId([0x10; 16]), key(), recovery_facts())
    };
    let (mut x, mut y, mut b, mut p) = super::buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();

    let mut verifier =
        retinue::command::Verifier::<1>::new(retinue::hash::AddressHash::from_bytes([0x10; 16]));
    verifier.authorize(*operator.public()).unwrap();
    let arm_payload = encoded_request(&apply_request(1));
    let arm_wire = signed_command(&operator, &arm_payload, 1);
    let arm_verified = verifier.verify(&arm_wire).unwrap();
    let inbound_arm = decode_verified_command(&arm_verified).unwrap();
    let armed = block_on(r.arm_inbound(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        &inbound_arm,
        1,
        prepared(1, 10),
    ))
    .unwrap();
    let generation = armed.value().response().effective_generation.unwrap();

    let commit_payload = encoded_request(&commit_request(2));
    let commit_wire = signed_command(&operator, &commit_payload, 2);
    let commit_verified = verifier.verify(&commit_wire).unwrap();
    let inbound_commit = decode_verified_command(&commit_verified).unwrap();
    assert!(matches!(
        block_on(r.commit_inbound(
        &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            &inbound_commit,
            2,
            PreparedCommit {
                change: ChangeId([1; 16]),
                candidate_generation: generation,
                commit_token: [0xA5; COMMIT_TOKEN_LEN],
            },
        )),
        Ok(outcome) if matches!(outcome.value(), Transition::Changed(_))
    ));

    let malformed_wire = signed_command(&operator, b"malformed inner payload", 3);
    let malformed_verified = verifier.verify(&malformed_wire).unwrap();
    assert!(matches!(
        decode_verified_command(&malformed_verified),
        Err(InboundControlError::InvalidRequest(_))
    ));
    t.clear();
    block_on(r.record_verified_command(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        &malformed_verified,
    ))
    .unwrap();
    assert!(
        t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Program(_)))
    );
    assert!(
        !t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Apply(_)))
    );

    let wrong_opcode_wire = retinue::command::Command {
        key_id: operator.hash(),
        class: retinue::command::TargetClass::Node,
        target: retinue::hash::AddressHash::from_bytes([0x10; 16]),
        counter: 4,
        opcode: COMMAND_OPCODE.wrapping_add(1),
        payload: b"other application",
    }
    .sign(&operator)
    .unwrap();
    let wrong_opcode = verifier.verify(&wrong_opcode_wire).unwrap();
    assert!(matches!(
        decode_verified_command(&wrong_opcode),
        Err(InboundControlError::WrongOpcode(_))
    ));
    t.clear();
    block_on(r.record_verified_command(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        &wrong_opcode,
    ))
    .unwrap();
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        4
    );
    assert!(
        t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Program(_)))
    );
    assert!(
        !t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Apply(_)))
    );

    let fleet = retinue::hash::AddressHash::from_bytes([0xf1; 16]);
    verifier.join_fleet(fleet);
    let fleet_wire = retinue::command::Command {
        key_id: operator.hash(),
        class: retinue::command::TargetClass::Fleet,
        target: fleet,
        counter: 5,
        opcode: COMMAND_OPCODE,
        payload: b"fleet authority is not node control",
    }
    .sign(&operator)
    .unwrap();
    let fleet_verified = verifier.verify(&fleet_wire).unwrap();
    assert!(matches!(
        decode_verified_command(&fleet_verified),
        Err(InboundControlError::NonNodeTarget)
    ));
    t.clear();
    block_on(r.record_verified_command(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        &fleet_verified,
    ))
    .unwrap();
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        5
    );
    assert!(
        !t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Apply(_)))
    );
}

#[test]
#[allow(unsafe_code)]
fn invalid_retinue_identity_journal_poisons_before_any_apply() {
    let t = Trace::default();
    let operator = operator();
    let mut s = FakeStore::blank(&t);
    let mut body = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state_for_operator(&operator), &mut body).unwrap();
    // First grant begins after RHD1's 30-byte fixed header; its public identity follows
    // the 16-byte controller id. Keep the hash consistent so this specifically exercises
    // Retinue public-key parsing rather than the radio-free hash binding check.
    let public = 46..110;
    let mut invalid_public = [0; RETINUE_PUBLIC_IDENTITY_LEN];
    invalid_public[32..].fill(2);
    body[public.clone()].copy_from_slice(&invalid_public);
    let digest = Sha256::digest(&body[public]);
    body[30..46].copy_from_slice(&digest[..16]);
    crate::store::encode(0, &body[..len], &mut s.a).unwrap();
    let mut a = FakeApplier::new(&t);
    // SAFETY: this fixture starts at a fresh simulated board boot.
    let mut r = unsafe {
        ControlRuntime::new_after_hardware_reset(NodeId([0x10; 16]), key(), recovery_facts())
    };
    let (mut x, mut y, mut b, mut p) = super::buffers();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Err(RuntimeError::Load(DurableLoadError::State(
            DurableError::Malformed
        )))
    ));
    assert!(r.is_poisoned());
    assert_eq!(a.calls, 0);
    assert!(
        !t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Apply(_)))
    );
}
