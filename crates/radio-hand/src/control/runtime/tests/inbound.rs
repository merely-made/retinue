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

#[test]
#[allow(unsafe_code)]
fn verified_status_is_journaled_before_it_is_answered_and_replay_is_refused() {
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
    let first_write = FirstWriteStatus {
        control: PairEvidence::Valid,
        pending: PairEvidence::Blank,
    };

    // The board's verifier comes from its durable grants, never from the carrier.
    let mut verifier = restore_verifier::<MAX_OWNER_GRANTS>(r.state().unwrap()).unwrap();
    let request = Request {
        transaction: TransactionId([0x51; 16]),
        transaction_sequence: 0,
        expected_generation: ConfigGeneration(0),
        operation: Operation::Status,
        arguments: heapless::Vec::new(),
    };
    let wire = signed_command(&operator, &encoded_request(&request), 1);
    let mut frame = [0_u8; MAX_CONTROL_COMMAND_FRAME_LEN];
    let frame_len = encode_command_frame(&wire, &mut frame).unwrap();
    let command = decode_command_frame(&frame[..frame_len]).unwrap();
    assert_eq!(command, &wire[..]);
    let verified = verifier.verify(command).unwrap();
    let inbound = decode_verified_command(&verified).unwrap();

    t.clear();
    let response = block_on(r.observe_status_inbound(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        &inbound,
        first_write,
    ))
    .unwrap()
    .into_value();
    let calls = t.snapshot();
    super::assert_quiet_bounds(&calls);
    assert!(calls.iter().any(|call| matches!(call, Call::Program(_))));
    assert!(!calls.iter().any(|call| matches!(call, Call::Apply(_))));
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        1
    );

    assert_eq!(response.node, NodeId([0x10; 16]));
    assert_eq!(response.transaction, request.transaction);
    assert_eq!(response.known_good_generation, ConfigGeneration(7));
    assert_eq!(response.effective_generation, None);
    let ResponseBody::Observed(body) = &response.body else {
        panic!("a verified Status is answered with an Observed body");
    };
    let status = ControlStatusV1::decode(body).unwrap();
    assert_eq!(
        status.authority(),
        ControlStatusAuthority::VerifiedController
    );
    assert_eq!(status.query_nonce(), [0x51; CONTROL_STATUS_NONCE_LEN]);
    assert_eq!(status.node(), NodeId([0x10; 16]));
    assert_eq!(status.control(), ControlStatusEvidence::Valid);
    assert_eq!(status.pending(), ControlStatusEvidence::Blank);
    assert_eq!(status.boot(), ControlStatusBootFact::KnownGoodApplied);
    assert_eq!(status.known_good_generation(), ConfigGeneration(7));
    assert_eq!(status.generation_watermark(), ConfigGeneration(7));

    let mut response_frame = [0_u8; MAX_CONTROL_RESPONSE_FRAME_LEN];
    let response_len = encode_response_frame(&response, &mut response_frame).unwrap();
    assert_eq!(
        decode_response_frame(&response_frame[..response_len]).unwrap(),
        response
    );

    // Exact replay is refused by the live verifier and by one rebuilt from the journal.
    assert_eq!(
        verifier.verify(&wire).err(),
        Some(retinue::command::Refusal::CounterReplayed)
    );
    let mut rebuilt = restore_verifier::<MAX_OWNER_GRANTS>(r.state().unwrap()).unwrap();
    assert_eq!(
        rebuilt.verify(&wire).err(),
        Some(retinue::command::Refusal::CounterReplayed)
    );

    // A verified mutation reaching the Status-only observer is refused as unsupported, but
    // its outer counter is still journaled before the refusal leaves the board.
    let mutation = apply_request(2);
    let mutation_wire = signed_command(&operator, &encoded_request(&mutation), 2);
    let mutation_verified = rebuilt.verify(&mutation_wire).unwrap();
    let mutation_inbound = decode_verified_command(&mutation_verified).unwrap();
    t.clear();
    let refused = block_on(r.observe_status_inbound(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        &mutation_inbound,
        first_write,
    ))
    .unwrap()
    .into_value();
    assert!(matches!(
        refused.body,
        ResponseBody::Refused {
            reason: Refusal::UnsupportedOperation,
            ..
        }
    ));
    assert_eq!(refused.transaction, mutation.transaction);
    let calls = t.snapshot();
    assert!(calls.iter().any(|call| matches!(call, Call::Program(_))));
    assert!(!calls.iter().any(|call| matches!(call, Call::Apply(_))));
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        2
    );
    assert!(!r.is_poisoned());
}

/// One signed lifecycle request from the operator, verified by the board's verifier.
#[cfg(feature = "control-retinue")]
fn lifecycle_request(
    operation: Operation,
    transaction: u8,
    sequence: u64,
    expected: ConfigGeneration,
    arguments: &[u8],
) -> Request {
    Request {
        transaction: TransactionId([transaction; 16]),
        transaction_sequence: sequence,
        expected_generation: expected,
        operation,
        arguments: heapless::Vec::try_from(arguments).unwrap(),
    }
}

#[test]
#[allow(unsafe_code)]
fn lifecycle_over_verified_commands_applies_commits_reverts_and_expires() {
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
    let first_write = FirstWriteStatus {
        control: PairEvidence::Valid,
        pending: PairEvidence::Blank,
    };
    let mut verifier = restore_verifier::<MAX_OWNER_GRANTS>(r.state().unwrap()).unwrap();
    let mut counter = 0_u64;
    let mut serve = |r: &mut ControlRuntime,
                     window: &mut FakeLiveOwner<'_>,
                     request: &Request,
                     now: u64,
                     token: u8| {
        counter += 1;
        let wire = signed_command(&operator, &encoded_request(request), counter);
        let verified = verifier.verify(&wire).unwrap();
        let inbound = decode_verified_command(&verified).unwrap();
        block_on(r.serve_inbound(
            window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            &inbound,
            now,
            first_write,
            [token; COMMIT_TOKEN_LEN],
        ))
        .map(|outcome| outcome.into_value())
    };
    let candidate = configuration(b"candidate").public;
    let change = ChangeId([0x77; 16]);

    // Out-of-range lifetime: refused, but the counter is journaled.
    let too_long = ProvisionalApplyArguments {
        change,
        public: candidate,
        lifetime_ms: MAX_PROVISIONAL_LIFETIME_MS + 1,
    };
    t.clear();
    let refused = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::ProvisionalApply,
            1,
            1,
            ConfigGeneration(7),
            &too_long.encode(),
        ),
        1_000,
        0xA1,
    )
    .unwrap();
    assert!(matches!(
        refused.body,
        ResponseBody::Refused {
            reason: Refusal::InvalidArguments,
            ..
        }
    ));
    assert!(t.snapshot().iter().any(|c| matches!(c, Call::Program(_))));
    assert!(!t.snapshot().iter().any(|c| matches!(c, Call::Apply(_))));
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        1
    );
    assert!(r.provisional_deadline_ms().is_none());

    // Apply: journaled before the hardware sees the candidate, answered with the token.
    let apply = ProvisionalApplyArguments {
        change,
        public: candidate,
        lifetime_ms: 60_000,
    };
    t.clear();
    let provisional = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::ProvisionalApply,
            2,
            2,
            ConfigGeneration(7),
            &apply.encode(),
        ),
        1_000,
        0xA2,
    )
    .unwrap();
    let ResponseBody::Provisional {
        deadline_ms,
        commit_token,
        ..
    } = provisional.body
    else {
        panic!("apply answers provisionally");
    };
    assert_eq!(deadline_ms, 61_000);
    assert_eq!(commit_token, [0xA2; COMMIT_TOKEN_LEN]);
    let candidate_generation = provisional.effective_generation.unwrap();
    assert_eq!(candidate_generation, ConfigGeneration(8));
    assert_eq!(r.provisional_deadline_ms(), Some(61_000));
    let calls = t.snapshot();
    let program = calls
        .iter()
        .position(|c| matches!(c, Call::Program(_)))
        .unwrap();
    let apply_at = calls
        .iter()
        .position(|c| matches!(c, Call::Apply(_)))
        .unwrap();
    assert!(
        program < apply_at,
        "the rollback record is durable before the radio changes"
    );
    assert_eq!(window.applier.last_public, Some(candidate));

    // Commit with the wrong change id names nothing: refused, candidate stays armed.
    let wrong = CommitArguments {
        change: ChangeId([0x78; 16]),
        candidate_generation,
        commit_token,
    };
    let refused = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::Commit,
            3,
            3,
            ConfigGeneration(7),
            &wrong.encode(),
        ),
        2_000,
        0,
    )
    .unwrap();
    assert!(matches!(
        refused.body,
        ResponseBody::Refused {
            reason: Refusal::InvalidCommit,
            ..
        }
    ));
    assert_eq!(r.provisional_deadline_ms(), Some(61_000));

    // Revert by the right change id restores known-good on the hardware and in the journal.
    t.clear();
    let reverted = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::Revert,
            4,
            4,
            ConfigGeneration(7),
            &RevertArguments { change }.encode(),
        ),
        3_000,
        0,
    )
    .unwrap();
    assert!(matches!(reverted.body, ResponseBody::Applied(_)));
    assert_eq!(reverted.known_good_generation, ConfigGeneration(7));
    assert!(r.provisional_deadline_ms().is_none());
    assert_eq!(
        window.applier.last_public,
        Some(configuration(b"old").public)
    );
    assert!(t.snapshot().iter().any(|c| matches!(c, Call::Apply(_))));

    // A second apply then a matching commit: known-good moves to the candidate generation,
    // which is fresh because the reverted generation is never reused.
    let apply = ProvisionalApplyArguments {
        change: ChangeId([0x79; 16]),
        public: candidate,
        lifetime_ms: 60_000,
    };
    let provisional = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::ProvisionalApply,
            5,
            5,
            ConfigGeneration(7),
            &apply.encode(),
        ),
        4_000,
        0xA5,
    )
    .unwrap();
    let candidate_generation = provisional.effective_generation.unwrap();
    assert_eq!(candidate_generation, ConfigGeneration(9));
    let commit = CommitArguments {
        change: ChangeId([0x79; 16]),
        candidate_generation,
        commit_token: [0xA5; COMMIT_TOKEN_LEN],
    };
    let committed = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::Commit,
            6,
            6,
            ConfigGeneration(7),
            &commit.encode(),
        ),
        5_000,
        0,
    )
    .unwrap();
    assert!(matches!(committed.body, ResponseBody::Applied(_)));
    assert_eq!(committed.known_good_generation, ConfigGeneration(9));
    assert!(r.provisional_deadline_ms().is_none());
    assert_eq!(
        r.state().unwrap().known_good().generation,
        ConfigGeneration(9)
    );

    // Expiry: an unconfirmed third candidate rolls back when the board clock passes its
    // deadline, and the hardware is restored to the new known-good.
    let apply = ProvisionalApplyArguments {
        change: ChangeId([0x7A; 16]),
        public: configuration(b"third").public,
        lifetime_ms: 1_000,
    };
    let provisional = serve(
        &mut r,
        &mut window,
        &lifecycle_request(
            Operation::ProvisionalApply,
            7,
            7,
            ConfigGeneration(9),
            &apply.encode(),
        ),
        6_000,
        0xA7,
    )
    .unwrap();
    assert!(matches!(provisional.body, ResponseBody::Provisional { .. }));
    assert_eq!(r.provisional_deadline_ms(), Some(7_000));
    let not_yet = block_on(r.expire(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        6_999,
    ))
    .unwrap();
    assert!(!not_yet.into_value());
    assert_eq!(r.provisional_deadline_ms(), Some(7_000));
    t.clear();
    let rolled_back = block_on(r.expire(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        7_000,
    ))
    .unwrap();
    assert!(rolled_back.into_value());
    assert!(r.provisional_deadline_ms().is_none());
    assert_eq!(window.applier.last_public, Some(candidate));
    assert_eq!(
        r.state().unwrap().known_good().generation,
        ConfigGeneration(9)
    );
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        7
    );
    assert!(!r.is_poisoned());
}
