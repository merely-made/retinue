//! Stage, resume, and abandon under injected storage faults.

use seneschal::control::{
    AbandonOutcome, FirstWriteActions, FirstWriteScratch, FirstWriteStorageError, PairEvidence,
    ResumeOutcome, abandon_first_write, first_write_status, resume_first_write, stage_first_write,
};

use super::store::{Fault, Store, run_with, staged_store};
use super::{NODE, PAGE, facts, state};

#[test]
fn stage_failures_never_touch_control_and_leave_a_safe_retry_state() {
    for fault in [
        Fault::ErasePending,
        Fault::ProgramPending,
        Fault::VerifyPending,
        Fault::TornProgramPending,
    ] {
        let mut store = Store::blank();
        store.inject(fault);
        assert!(
            run_with(&mut store, |store, scratch| stage_first_write(
                store,
                scratch,
                &state(),
                NODE,
                &facts()
            ))
            .is_err()
        );
        let status = first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts(),
        );
        assert_eq!(status.control, PairEvidence::Blank);
        assert!(status.claim_eligible() || status.resume_eligible() || status.abandon_eligible());
    }
}

#[test]
fn inspection_read_failures_are_typed_and_scratch_refuses_mismatched_slots() {
    for fault in [Fault::ReadControl, Fault::ReadPending] {
        let mut store = Store::blank();
        store.inject(fault);
        assert!(matches!(
            run_with(&mut store, |store, scratch| stage_first_write(
                store,
                scratch,
                &state(),
                NODE,
                &facts()
            )),
            Err(FirstWriteStorageError::Store { .. })
        ));
    }
    let mut a = [0; 8];
    let mut b = [0; 8];
    let mut p_a = [0; 8];
    let mut p_b = [0; 8];
    let mut body = [0; seneschal::control::MAX_DURABLE_BODY];
    let mut page = [0; 7];
    let mut readback = [0; 8];
    assert!(
        FirstWriteScratch::new(
            &mut a,
            &mut b,
            &mut p_a,
            &mut p_b,
            &mut body,
            &mut page,
            &mut readback
        )
        .is_err()
    );
}

#[test]
fn corrupt_control_can_be_repaired_from_valid_pending_but_torn_control_never_hides_pending() {
    let mut store = staged_store();
    store.control[0][0] = 0;
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .control,
        PairEvidence::Valid
    );

    let mut store = staged_store();
    store.inject(Fault::TornProgramControl);
    assert!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Corrupt);
    assert_eq!(status.pending, PairEvidence::Valid);
}

#[test]
fn corrupt_repair_advances_outer_sequence_and_handles_max_without_losing_pending() {
    let mut store = staged_store();
    seneschal::store::encode(41, b"malformed-durable-body", &mut store.control[1]).unwrap();
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Corrupt);
    assert_eq!(status.pending, PairEvidence::Valid);
    store.inject(Fault::ProgramControl);
    assert!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Corrupt);
    assert_eq!(status.pending, PairEvidence::Valid);
    store.inject(Fault::None);
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    assert_eq!(
        seneschal::store::decode(&store.control[0])
            .unwrap()
            .sequence,
        42
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .pending,
        PairEvidence::Blank
    );

    let mut store = staged_store();
    seneschal::store::encode(u32::MAX, b"malformed-durable-a", &mut store.control[0]).unwrap();
    seneschal::store::encode(u32::MAX, b"malformed-durable-b", &mut store.control[1]).unwrap();
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    assert_eq!(
        seneschal::store::decode(&store.control[0])
            .unwrap()
            .sequence,
        u32::MAX
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .control,
        PairEvidence::Valid
    );
}

#[test]
fn resume_failures_preserve_pending_until_control_is_durable_then_report_cleanup() {
    for fault in [Fault::EraseControl, Fault::ProgramControl] {
        let mut store = staged_store();
        store.inject(fault);
        assert!(
            run_with(&mut store, |store, scratch| resume_first_write(
                store,
                scratch,
                NODE,
                &facts()
            ))
            .is_err()
        );
        let status = first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts(),
        );
        assert_eq!(status.control, PairEvidence::Blank);
        assert_eq!(status.pending, PairEvidence::Valid);
    }
    let mut store = staged_store();
    store.inject(Fault::VerifyControl);
    assert!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Valid);
    assert_eq!(status.pending, PairEvidence::Valid);

    for fault in [
        Fault::ErasePending,
        Fault::VerifyPending,
        Fault::ErasePendingB,
        Fault::VerifyPendingB,
    ] {
        let mut store = staged_store();
        store.inject(fault);
        let outcome = run_with(&mut store, |store, scratch| {
            resume_first_write(store, scratch, NODE, &facts())
        })
        .unwrap();
        assert!(matches!(
            outcome,
            ResumeOutcome::CommittedWithCleanupFailure(FirstWriteStorageError::Store { .. })
        ));
        let status = first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts(),
        );
        assert_eq!(status.control, PairEvidence::Valid);
    }
}

#[test]
fn abandon_requires_blank_control_and_its_partial_cleanup_is_safe_to_retry() {
    let mut store = staged_store();
    store.inject(Fault::ErasePending);
    assert!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .pending,
        PairEvidence::Valid
    );
    store.inject(Fault::None);
    assert_eq!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(AbandonOutcome::Abandoned)
    );
    let blank = [0xff; PAGE];
    assert!(
        first_write_status(
            &blank,
            &blank,
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .ordinary_service_eligible()
    );
}

#[test]
fn abandon_erases_corrupt_pending_only_with_blank_control_and_can_retry_after_slot_b_failure() {
    let mut store = staged_store();
    store.pending[0][0] = 0;
    assert_eq!(
        first_write_status(
            &store.control[0],
            &store.control[1],
            &store.pending[0],
            &store.pending[1],
            NODE,
            &facts()
        )
        .actions()
        .bits(),
        FirstWriteActions::ABANDON
    );
    assert_eq!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(AbandonOutcome::Abandoned)
    );

    let mut store = staged_store();
    store.inject(Fault::ErasePendingB);
    assert!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
    let status = first_write_status(
        &store.control[0],
        &store.control[1],
        &store.pending[0],
        &store.pending[1],
        NODE,
        &facts(),
    );
    assert_eq!(status.control, PairEvidence::Blank);
    assert_eq!(status.pending, PairEvidence::Blank);
    store.inject(Fault::None);
    assert_eq!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(AbandonOutcome::NothingStaged)
    );
}

#[test]
fn valid_control_wins_stale_pending_and_abandon_will_not_erase_it() {
    let mut store = staged_store();
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::Committed)
    );
    // Restage a valid pending record synthetically to prove it cannot supersede control.
    let stale = staged_store();
    store.pending = stale.pending;
    assert_eq!(
        run_with(&mut store, |store, scratch| resume_first_write(
            store,
            scratch,
            NODE,
            &facts()
        )),
        Ok(ResumeOutcome::AlreadyControlPresent)
    );
    assert!(
        run_with(&mut store, |store, scratch| abandon_first_write(
            store,
            scratch,
            NODE,
            &facts()
        ))
        .is_err()
    );
}
