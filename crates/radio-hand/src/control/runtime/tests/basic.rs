use super::super::*;
use super::fakes::*;
use crate::control::NodeId;
use crate::store::Slot;
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

#[test]
#[allow(unsafe_code)]
fn clean_boot_foreign_and_apply_failure() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let mut a = FakeApplier::new(&t);
    a.fail_at = Some(1);
    let (mut x, mut y, mut b, mut p) = buffers();
    let mut r = runtime();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Err(RuntimeError::Apply(_))
    ));
    assert!(r.is_poisoned());
    // SAFETY: this independent test fixture represents a fresh simulated board boot.
    let mut other = unsafe {
        ControlRuntime::new_after_hardware_reset(NodeId([9; 16]), key(), recovery_facts())
    };
    let mut ap = FakeApplier::new(&t);
    assert!(matches!(
        block_on(other.boot_pre_radio(
            &mut s,
            &mut ap,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Err(RuntimeError::ForeignNode { .. })
    ));
}

fn buffers() -> ([u8; PAGE], [u8; PAGE], [u8; MAX_DURABLE_BODY], [u8; PAGE]) {
    ([0; PAGE], [0; PAGE], [0; MAX_DURABLE_BODY], [0; PAGE])
}

#[test]
fn blank_and_short_scratch_fail_closed() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    let mut a = FakeApplier::new(&t);
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Ok(BootState::Blank)
    ));
    let mut short = [0; 8];
    let mut body = [0; MAX_DURABLE_BODY];
    assert!(matches!(
        DurableScratch::new(&mut short, &mut [0; 8], &mut body, &mut [0; 8]),
        Err(DurableScratchError::SlotTooSmall { .. })
    ));
}

#[test]
fn boot_is_one_shot_and_new_runtime_acknowledges_simulated_reset() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let mut a = FakeApplier::new(&t);
    let (mut x, mut y, mut b, mut p) = buffers();
    let mut r = runtime();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Ok(BootState::Ready)
    ));
    assert!(matches!(
        t.snapshot().as_slice(),
        [Call::Read(Slot::A), Call::Read(Slot::B), Call::Apply(public)]
            if *public == configuration(b"old").public
    ));
    s.a.fill(0xFF);
    s.b.fill(0xFF);
    t.clear();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Err(RuntimeError::BootAlreadyAttempted)
    ));
    assert!(t.snapshot().is_empty());

    let mut after_reset = runtime();
    assert!(matches!(
        block_on(after_reset.boot_pre_radio(
            &mut s,
            &mut a,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Ok(BootState::Blank)
    ));
    assert!(after_reset.state().is_none());
}

#[test]
fn dropped_pre_radio_boot_cannot_admit_live_work() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let mut a = FakeApplier::new(&t);
    a.pending = true;
    let mut r = runtime();
    let (mut x, mut y, mut b, mut p) = buffers();
    {
        let mut durable_scratch = scratch(&mut x, &mut y, &mut b, &mut p);
        let mut boot = core::pin::pin!(r.boot_pre_radio(&mut s, &mut a, &mut durable_scratch));
        let waker = futures::task::noop_waker_ref();
        let mut context = Context::from_waker(waker);
        assert!(matches!(boot.as_mut().poll(&mut context), Poll::Pending));
    }
    let calls = t.snapshot();
    let mut window = FakeLiveOwner::new(&t, s, a);
    assert!(matches!(
        block_on(r.record_verified_outer(
            &mut window,
            &mut scratch(&mut x, &mut y, &mut b, &mut p),
            controller(),
            1,
        )),
        Err(RuntimeError::BootIncomplete)
    ));
    assert_eq!(t.snapshot(), calls);
    assert!(matches!(
        block_on(r.boot_pre_radio(
            &mut window.store,
            &mut window.applier,
            &mut scratch(&mut x, &mut y, &mut b, &mut p)
        )),
        Err(RuntimeError::BootAlreadyAttempted)
    ));
    assert_eq!(t.snapshot(), calls);
}
