use super::super::*;
use super::fakes::*;
use crate::control::{NodeId, RuntimeError};
use core::{
    future::Future,
    task::{Context, Poll},
};
use futures::executor::block_on;

#[allow(unsafe_code)]
fn runtime() -> ControlRuntime {
    // SAFETY: each helper call represents a fresh simulated board boot.
    unsafe { ControlRuntime::new_after_hardware_reset(NodeId([0x10; 16]), key(), recovery_facts()) }
}

fn buffers() -> ([u8; PAGE], [u8; PAGE], [u8; MAX_DURABLE_BODY], [u8; PAGE]) {
    ([0; PAGE], [0; PAGE], [0; MAX_DURABLE_BODY], [0; PAGE])
}

fn boot_ready(
    r: &mut ControlRuntime,
    s: &mut FakeStore,
    a: &mut FakeApplier,
    x: &mut [u8; PAGE],
    y: &mut [u8; PAGE],
    b: &mut [u8; MAX_DURABLE_BODY],
    p: &mut [u8; PAGE],
) {
    block_on(r.boot_pre_radio(s, a, &mut scratch(x, y, b, p))).unwrap();
}

fn assert_quiet_bounds(calls: &[Call]) {
    let enter = calls
        .iter()
        .position(|c| matches!(c, Call::EnterQuiet))
        .unwrap();
    let finish = calls
        .iter()
        .rposition(|c| matches!(c, Call::FinishQuiet(_)))
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
fn live_storage_and_apply_are_inside_one_quiet_window() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    t.clear();
    let outcome = block_on(r.arm(
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
    assert_eq!(outcome.exit(), QuietExit::Resumed);
    let calls = t.snapshot();
    assert!(matches!(
        calls.as_slice(),
        [Call::EnterQuiet, .., Call::FinishQuiet(QuietExit::Resumed)]
    ));
    let enter = calls
        .iter()
        .position(|c| matches!(c, Call::EnterQuiet))
        .unwrap();
    let finish = calls
        .iter()
        .rposition(|c| matches!(c, Call::FinishQuiet(_)))
        .unwrap();
    assert!(
        calls[enter + 1..finish]
            .iter()
            .any(|c| matches!(c, Call::Program(_)))
    );
    assert!(
        calls[enter + 1..finish]
            .iter()
            .any(|c| matches!(c, Call::Apply(_)))
    );
    assert_quiet_bounds(&calls);
}

#[test]
fn apply_failure_recovery_reuses_the_entered_window() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
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
    assert_eq!(
        calls
            .iter()
            .filter(|c| matches!(c, Call::EnterQuiet))
            .count(),
        1
    );
    assert_eq!(
        calls
            .iter()
            .filter(|c| matches!(c, Call::FinishQuiet(_)))
            .count(),
        1
    );
    assert!(!r.is_poisoned());
    assert_quiet_bounds(&calls);
}

#[test]
fn quiet_entry_errors_are_retryable_but_finish_errors_poison() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    t.clear();
    window.fail_enter = true;
    assert!(matches!(
        block_on(r.record_verified_outer(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            controller(),
            1,
        )),
        Err(RuntimeError::Quiet(_))
    ));
    assert!(!r.is_poisoned());
    assert!(!r.quiet_in_progress());
    assert!(t.snapshot().is_empty());

    window.fail_enter = false;
    let outcome = block_on(r.record_verified_outer(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        controller(),
        1,
    ))
    .unwrap();
    assert_eq!(outcome.exit(), QuietExit::Resumed);
    assert_quiet_bounds(&t.snapshot());

    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    window.fail_finish = true;
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    t.clear();
    assert!(matches!(
        block_on(r.record_verified_outer(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            controller(),
            1,
        )),
        Err(RuntimeError::Quiet(_))
    ));
    assert!(r.is_poisoned());
    let calls = t.snapshot();
    assert!(matches!(calls.first(), Some(Call::EnterQuiet)));
    assert!(matches!(calls.last(), Some(Call::AbortQuiet)));
}

#[test]
fn reset_required_blocks_live_work_until_new_runtime() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    window.exit = QuietExit::ResetRequired;
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    let out = block_on(r.record_verified_outer(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        controller(),
        1,
    ))
    .unwrap();
    assert_eq!(out.exit(), QuietExit::ResetRequired);
    assert!(r.reset_pending());
    t.clear();
    assert!(matches!(
        block_on(r.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Err(RuntimeError::ResetPending)
    ));
    assert!(t.snapshot().is_empty());
    let calls = t.snapshot();
    assert!(matches!(
        block_on(r.record_verified_outer(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            controller(),
            2,
        )),
        Err(RuntimeError::ResetPending)
    ));
    assert_eq!(t.snapshot(), calls);

    let mut after_reset = runtime();
    assert!(matches!(
        block_on(after_reset.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Ok(BootState::Ready)
    ));
}

#[test]
fn operation_error_wins_finish_error_but_still_poisons() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    window.fail_finish = true;
    window.store.fail_program = true;
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    assert!(matches!(
        block_on(r.record_verified_outer(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            controller(),
            1,
        )),
        Err(RuntimeError::Store(_))
    ));
    assert!(r.is_poisoned());
}

#[test]
fn dropped_live_apply_aborts_quiet_and_blocks_the_runtime() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    window.applier.pending = true;
    t.clear();

    {
        let mut durable_scratch = scratch(&mut x, &mut y, &mut b, &mut p);
        let request = apply_request(1);
        let mut live = core::pin::pin!(r.arm(
            &mut window,
            &mut durable_scratch,
            NodeId([0x10; 16]),
            controller(),
            1,
            &request,
            1,
            prepared(1, 10),
        ));
        let waker = futures::task::noop_waker_ref();
        let mut context = Context::from_waker(waker);
        assert!(matches!(live.as_mut().poll(&mut context), Poll::Pending));
    }

    let calls = t.snapshot();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, Call::AbortQuiet))
            .count(),
        1
    );
    assert!(r.quiet_in_progress());
    assert!(matches!(
        block_on(r.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Err(RuntimeError::QuietInProgress)
    ));
    assert_eq!(t.snapshot(), calls);
}

#[test]
fn dropped_quiet_entry_latches_reset_before_any_durable_or_apply_work() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let mut a = FakeApplier::new(&t);
    let mut window = PendingEntryQuiet::new(&t);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(&mut r, &mut s, &mut a, &mut x, &mut y, &mut b, &mut p);
    t.clear();

    {
        let mut durable_scratch = scratch(&mut x, &mut y, &mut b, &mut p);
        let request = apply_request(1);
        let mut live = core::pin::pin!(r.arm(
            &mut window,
            &mut durable_scratch,
            NodeId([0x10; 16]),
            controller(),
            1,
            &request,
            1,
            prepared(1, 10),
        ));
        let waker = futures::task::noop_waker_ref();
        let mut context = Context::from_waker(waker);
        assert!(matches!(live.as_mut().poll(&mut context), Poll::Pending));
    }

    assert!(window.radio_stopped());
    assert!(window.reset_latched());
    assert!(matches!(
        t.snapshot().as_slice(),
        [Call::EnterQuiet, Call::AbortEnteringQuiet]
    ));
    assert!(r.quiet_in_progress());
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Err(RuntimeError::QuietInProgress)
    ));
    assert!(matches!(
        t.snapshot().as_slice(),
        [Call::EnterQuiet, Call::AbortEnteringQuiet]
    ));
}

#[test]
fn dropped_finish_aborts_quiet_and_leaves_the_runtime_blocked() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    window.pending_finish = true;
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
    t.clear();

    {
        let mut durable_scratch = scratch(&mut x, &mut y, &mut b, &mut p);
        let mut live = core::pin::pin!(r.record_verified_outer(
            &mut window,
            &mut durable_scratch,
            controller(),
            1,
        ));
        let waker = futures::task::noop_waker_ref();
        let mut context = Context::from_waker(waker);
        assert!(matches!(live.as_mut().poll(&mut context), Poll::Pending));
    }

    assert!(r.quiet_in_progress());
    assert_eq!(
        t.snapshot()
            .iter()
            .filter(|call| matches!(call, Call::AbortQuiet))
            .count(),
        1
    );
}

#[test]
fn expire_and_revert_keep_every_operation_inside_quiet_bounds() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let a = FakeApplier::new(&t);
    let mut window = FakeLiveOwner::new(&t, s, a);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    boot_ready(
        &mut r,
        &mut window.store,
        &mut window.applier,
        &mut x,
        &mut y,
        &mut b,
        &mut p,
    );
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
    block_on(r.expire(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        10,
    ))
    .unwrap();
    assert_quiet_bounds(&t.snapshot());

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
    block_on(r.revert(&mut window, &mut scratch(&mut x, &mut y, &mut b, &mut p))).unwrap();
    assert_quiet_bounds(&t.snapshot());
}
