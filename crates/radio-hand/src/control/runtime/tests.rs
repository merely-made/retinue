mod authority;
mod basic;
mod commit;
mod fakes;
#[cfg(feature = "control-retinue")]
mod inbound;
#[cfg(feature = "control-retinue")]
mod lifecycle;
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
