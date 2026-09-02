use super::super::*;
use super::fakes::*;
use crate::control::{
    BoardRecoveryFacts, DurableError, ManagementCarrier, NodeId, RecoveryPathFacts, Refusal,
    ResponseBody,
};
use futures::executor::block_on;
use heapless::Vec;

#[allow(unsafe_code)]
fn runtime() -> ControlRuntime {
    // SAFETY: each helper call represents a fresh simulated board boot.
    unsafe { ControlRuntime::new_after_hardware_reset(NodeId([0x10; 16]), key(), recovery_facts()) }
}

fn buffers() -> ([u8; PAGE], [u8; PAGE], [u8; MAX_DURABLE_BODY], [u8; PAGE]) {
    ([0; PAGE], [0; PAGE], [0; MAX_DURABLE_BODY], [0; PAGE])
}

#[test]
fn unsafe_recovery_arm_journals_counter_and_refusal_without_apply() {
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
    let result = block_on(r.arm(
        &mut window,
        &mut scratch(&mut x, &mut y, &mut b, &mut p),
        NodeId([0x10; 16]),
        controller(),
        1,
        &apply_request(1),
        1,
        unsafe_prepared(1, 10),
    ));
    assert!(matches!(
        result,
        Ok(ref transition) if matches!(
            transition.value().response().body,
            ResponseBody::Refused { reason: Refusal::UnsafeRecoveryPath, .. }
        )
    ));
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
    assert_eq!(
        r.state().unwrap().owner_grants()[0].accepted_outer_counter(),
        1
    );
}

#[test]
#[allow(unsafe_code)]
fn incompatible_board_facts_poison_boot_before_apply() {
    let t = Trace::default();
    let mut s = FakeStore::blank(&t);
    seed(&mut s, &state());
    let facts = BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Reticulum, false, true, true).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    // SAFETY: this test begins a fresh simulated board boot with incompatible board facts.
    let mut r =
        unsafe { ControlRuntime::new_after_hardware_reset(NodeId([0x10; 16]), key(), facts) };
    let mut a = FakeApplier::new(&t);
    let (mut x, mut y, mut b, mut p) = buffers();
    assert!(matches!(
        block_on(r.boot_pre_radio(&mut s, &mut a, &mut scratch(&mut x, &mut y, &mut b, &mut p))),
        Err(RuntimeError::Durable(DurableError::Malformed))
    ));
    assert!(r.is_poisoned());
    assert!(
        !t.snapshot()
            .iter()
            .any(|call| matches!(call, Call::Apply(_)))
    );
}
