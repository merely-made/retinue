//! A full verified-command lifecycle: arm, commit, revert, and expire.

use super::fakes::*;
use crate::control::*;
use futures::executor::block_on;

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
