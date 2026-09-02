mod authority;
mod basic;
mod fakes;
#[cfg(feature = "control-retinue")]
mod inbound;
mod quiet;
mod recovery;
use super::*;
use crate::control::{COMMIT_TOKEN_LEN, ChangeId, NodeId};
use fakes::*;
use futures::executor::block_on;
#[allow(unsafe_code)]
fn runtime() -> ControlRuntime {
    // SAFETY: each helper call represents a fresh simulated board boot.
    unsafe { ControlRuntime::new_after_hardware_reset(NodeId([0x10; 16]), key(), recovery_facts()) }
}

fn buffers() -> ([u8; PAGE], [u8; PAGE], [u8; MAX_DURABLE_BODY], [u8; PAGE]) {
    ([0; PAGE], [0; PAGE], [0; MAX_DURABLE_BODY], [0; PAGE])
}

fn assert_quiet_bounds(calls: &[Call]) {
    let enter = calls
        .iter()
        .position(|call| matches!(call, Call::EnterQuiet))
        .unwrap();
    let finish = calls
        .iter()
        .rposition(|call| matches!(call, Call::FinishQuiet(_)))
        .unwrap();
    assert!(enter < finish);
    for (index, call) in calls.iter().enumerate() {
        if matches!(
            call,
            Call::Read(_) | Call::Erase(_) | Call::Program(_) | Call::Apply(_)
        ) {
            assert!(enter < index && index < finish);
        }
    }
}
#[test]
fn arm_counter_persists_before_apply_and_replay() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    t.clear();
    let q = apply_request(1);
    let z = block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        1,
        &q,
        1,
        prepared(1, 10),
    ))
    .unwrap();
    assert!(matches!(z.value(), Transition::Changed(_)));
    let c = t.snapshot();
    assert!(
        c.iter()
            .position(|v| matches!(v, Call::Program(_)))
            .unwrap()
            < c.iter().position(|v| matches!(v, Call::Apply(_))).unwrap()
    );
    t.clear();
    assert!(matches!(
        block_on(r.arm(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            2,
            &q,
            2,
            prepared(1, 10)
        ))
        .unwrap()
        .into_value(),
        Transition::Replayed(_)
    ));
    assert!(
        t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Program(_)))
    );
}
#[test]
fn produced_flash_reboots_and_commit_failure_poisons() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    let q = apply_request(3);
    let armed = block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        3,
        &q,
        1,
        prepared(3, 10),
    ))
    .unwrap();
    let g = armed.value().response().effective_generation.unwrap();
    window.store.fail_program = true;
    assert!(matches!(
        block_on(r.commit(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            4,
            &commit_request(4),
            2,
            PreparedCommit {
                change: ChangeId([3; 16]),
                candidate_generation: g,
                commit_token: [0xA5; COMMIT_TOKEN_LEN]
            }
        )),
        Err(RuntimeError::Store(_))
    ));
    assert!(r.is_poisoned());
    window.store.fail_program = false;
    let mut reboot = runtime();
    assert!(matches!(
        block_on(reboot.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Ok(BootState::Ready)
    ));
}
#[test]
fn commit_success_persists_and_readback_selects_candidate() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    let armed = block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        1,
        &apply_request(1),
        1,
        prepared(1, 10),
    ))
    .unwrap();
    let generation = armed.value().response().effective_generation.unwrap();
    t.clear();
    assert!(matches!(
        block_on(r.commit(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            2,
            &commit_request(2),
            2,
            PreparedCommit {
                change: ChangeId([1; 16]),
                candidate_generation: generation,
                commit_token: [0xA5; COMMIT_TOKEN_LEN],
            },
        )),
        Ok(outcome) if matches!(outcome.value(), Transition::Changed(_))
    ));
    let calls = t.snapshot();
    assert!(calls.iter().any(|call| matches!(call, Call::Program(_))));
    assert!(
        calls
            .iter()
            .position(|call| matches!(call, Call::Program(_)))
            .unwrap()
            < calls
                .iter()
                .rposition(|call| matches!(call, Call::Read(_)))
                .unwrap()
    );
    assert_eq!(r.state().unwrap().known_good().generation, generation);
    let mut reboot = runtime();
    let mut reboot_applier = FakeApplier::new(&t);
    assert_eq!(
        block_on(reboot.boot_pre_radio(
            &mut window.store,
            &mut reboot_applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        ))
        .unwrap(),
        BootState::Ready
    );
    assert_eq!(
        reboot_applier.last_public,
        Some(configuration(b"candidate").public)
    );
}
#[test]
fn changed_refused_persists_counter_without_candidate_apply() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    let mut request = apply_request(1);
    request.expected_generation = ConfigGeneration(6);
    t.clear();
    assert!(matches!(
        block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            1,
            &request,
            1,
            prepared(1, 10),
        )),
        Ok(outcome)
            if matches!(
                outcome.value(),
                Transition::Changed(response)
                    if matches!(response.body, ResponseBody::Refused { reason: Refusal::StaleGeneration, .. })
            )
    ));
    assert!(!r.is_poisoned());
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
}
#[test]
fn nonmonotonic_counter_poisons_before_persist() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        1,
        &apply_request(1),
        1,
        prepared(1, 10),
    ))
    .unwrap();
    t.clear();
    assert!(matches!(
        block_on(r.arm(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            1,
            &apply_request(1),
            2,
            prepared(1, 10),
        )),
        Err(RuntimeError::VerifiedCounter(
            VerifiedCounterError::NotMonotonic
        ))
    ));
    assert!(r.is_poisoned());
    assert!(matches!(
        t.snapshot().as_slice(),
        [Call::EnterQuiet, Call::FinishQuiet(QuietExit::Resumed)]
    ));
}
#[test]
fn erase_program_and_readback_failures_poison_before_candidate_apply() {
    for failure in 0..3 {
        let t = Trace::default();
        let mut s = FakeStore::blank(&t);
        seed(&mut s, &state());
        let a = FakeApplier::new(&t);
        let mut window = FakeLiveOwner::new(&t, s, a);
        let mut r = runtime();
        let (mut x, mut y, mut b, mut p) = buffers();
        block_on(r.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
        ))
        .unwrap();
        match failure {
            0 => window.store.fail_erase = true,
            1 => window.store.fail_program = true,
            _ => window.store.corrupt_readback = true,
        }
        t.clear();
        let result = block_on(r.arm(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            1,
            &apply_request(1),
            1,
            prepared(1, 10),
        ));
        assert!(matches!(
            result,
            Err(RuntimeError::Store(_)) | Err(RuntimeError::ReadbackMismatch)
        ));
        assert!(r.is_poisoned());
        assert!(
            !t.snapshot()
                .iter()
                .any(|call| matches!(call, Call::Apply(_)))
        );
    }
}
#[test]
fn candidate_apply_failure_restores_and_persists_known_good() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    window.applier.fail_at = Some(2);
    assert_eq!(window.applier.fail_at, Some(2));
    t.clear();
    assert!(matches!(
        block_on(r.arm(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            1,
            &apply_request(1),
            1,
            prepared(1, 10),
        )),
        Err(RuntimeError::Apply(_))
    ));
    let calls = t.snapshot();
    let mut applies = calls.iter().filter_map(|call| match call {
        Call::Apply(public) => Some(*public),
        _ => None,
    });
    assert_eq!(applies.next(), Some(configuration(b"candidate").public));
    assert_eq!(applies.next(), Some(configuration(b"old").public));
    assert!(applies.next().is_none());
    assert!(
        calls
            .iter()
            .position(|call| matches!(call, Call::Apply(public) if *public == configuration(b"old").public))
            .unwrap()
            < calls
                .iter()
                .rposition(|call| matches!(call, Call::Program(_)))
                .unwrap()
    );
    assert_quiet_bounds(&calls);
    assert!(!r.is_poisoned());
}

#[test]
fn provisional_reboot_applies_known_good_before_rollback_persist() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let (mut x, mut y, mut b, mut p) = buffers();
    let mut r = runtime();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        1,
        &apply_request(1),
        1,
        prepared(1, 10),
    ))
    .unwrap();
    t.clear();
    let mut reboot = runtime();
    let mut reboot_applier = FakeApplier::new(&t);
    assert_eq!(
        block_on(reboot.boot_pre_radio(
            &mut window.store,
            &mut reboot_applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        ))
        .unwrap(),
        BootState::Ready
    );
    let calls = t.snapshot();
    assert!(
        calls
            .iter()
            .all(|call| !matches!(call, Call::EnterQuiet | Call::FinishQuiet(_)))
    );
    assert!(
        calls
            .iter()
            .position(|call| matches!(call, Call::Apply(public) if *public == configuration(b"old").public))
            .unwrap()
            < calls
                .iter()
                .position(|call| matches!(call, Call::Program(_)))
                .unwrap()
    );
    assert!(reboot.state().unwrap().provisional().is_none());
}

#[test]
fn recovery_apply_and_recovery_persistence_failures_poison() {
    for persistence_failure in [false, true] {
        let t = Trace::default();
        let mut s = FakeStore::blank(&t);
        seed(&mut s, &state());
        let a = FakeApplier::new(&t);
        let mut window = FakeLiveOwner::new(&t, s, a);
        let (mut x, mut y, mut b, mut p) = buffers();
        let mut first = runtime();
        block_on(first.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
        ))
        .unwrap();
        block_on(first.arm(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            NodeId([0x10; 16]),
            controller(),
            1,
            &apply_request(1),
            1,
            prepared(1, 10),
        ))
        .unwrap();
        t.clear();
        let mut reboot = runtime();
        let mut recovery_applier = FakeApplier::new(&t);
        if persistence_failure {
            window.store.fail_program = true;
        } else {
            recovery_applier.fail_at = Some(1);
        }
        assert!(matches!(
            block_on(reboot.boot_pre_radio(
                &mut window.store,
                &mut recovery_applier,
                &mut scratch(&mut x, &mut y, &mut b, &mut p)
            )),
            Err(RuntimeError::Apply(_)) | Err(RuntimeError::Store(_))
        ));
        assert!(reboot.is_poisoned());
        if persistence_failure {
            assert!(
                t.snapshot()
                    .iter()
                    .position(|call| matches!(call, Call::Apply(_)))
                    .unwrap()
                    < t.snapshot()
                        .iter()
                        .position(|call| matches!(call, Call::Program(_)))
                        .unwrap()
            );
        }
    }
}

#[test]
fn expire_and_revert_apply_before_persist() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    block_on(r.boot_pre_radio(
        &mut window.store,
        &mut window.applier,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
    ))
    .unwrap();
    block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        1,
        &apply_request(1),
        1,
        prepared(1, 10),
    ))
    .unwrap();
    t.clear();
    assert!(
        block_on(r.expire(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            10,
        ))
        .unwrap()
        .value()
    );
    let calls = t.snapshot();
    assert_quiet_bounds(&calls);
    assert!(
        calls
            .iter()
            .position(|call| matches!(call, Call::Apply(_)))
            .unwrap()
            < calls
                .iter()
                .position(|call| matches!(call, Call::Program(_)))
                .unwrap()
    );

    block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        2,
        &apply_request(2),
        2,
        prepared(2, 20),
    ))
    .unwrap();
    t.clear();
    assert!(
        block_on(r.revert(&mut window, &mut scratch(&mut x, &mut y, &mut b, &mut p),))
            .unwrap()
            .value()
    );
    let calls = t.snapshot();
    assert_quiet_bounds(&calls);
    assert!(
        calls
            .iter()
            .position(|call| matches!(call, Call::Apply(_)))
            .unwrap()
            < calls
                .iter()
                .position(|call| matches!(call, Call::Program(_)))
                .unwrap()
    );
}
