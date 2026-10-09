//! Stalls, late or wrong acknowledgements, overdue returns, and clock faults.

use tulle::personality::{
    Acknowledgement, Controller, ControllerConfig, ControllerError, ControllerEvent,
    ControllerState, CoverageEvidence, CoveragePolicy, InstalledPersonalitySet, InterruptionPolicy,
    PauseOutcome, ReturnReason, StopCapability,
};

use crate::{HOME, OTHER, controller, request};

#[test]
fn stalled_activation_and_failed_restore_require_recovery() {
    let mut c = controller(CoveragePolicy::AllowGap);
    let t = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        c.tick(6, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::RecoveryRequired)
    );
    assert_eq!(
        c.acknowledge(6, t.id, Acknowledgement::Completed),
        Err(ControllerError::WrongAcknowledgement {
            expected: 0,
            received: t.id
        })
    );
    let mut c = controller(CoveragePolicy::AllowGap);
    let t = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    c.acknowledge(1, t.id, Acknowledgement::Completed).unwrap();
    c.cancel(2).unwrap();
    let back = c.begin_return(2, PauseOutcome::Ready).unwrap().unwrap();
    c.acknowledge(3, back.id, Acknowledgement::Unknown).unwrap();
    assert_eq!(c.state(), ControllerState::RecoveryRequired);
}

#[test]
fn return_transition_deadline_is_capped_and_late_ack_recovers() {
    let mut c = controller(CoveragePolicy::AllowGap);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    c.acknowledge(1, departure.id, Acknowledgement::Completed)
        .unwrap();
    c.finish(30).unwrap();
    let back = c.begin_return(50, PauseOutcome::Ready).unwrap().unwrap();
    assert_eq!(back.deadline, 50);
    c.acknowledge(51, back.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(c.state(), ControllerState::RecoveryRequired);
}

#[test]
fn wrong_departure_ack_preserves_transition_for_the_matching_ack() {
    let mut c = controller(CoveragePolicy::AllowGap);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        c.acknowledge(1, departure.id + 1, Acknowledgement::Completed),
        Err(ControllerError::WrongAcknowledgement {
            expected: departure.id,
            received: departure.id + 1
        })
    );
    assert!(matches!(c.state(), ControllerState::Transitioning { .. }));
    c.acknowledge(2, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert!(matches!(c.state(), ControllerState::Away { .. }));
}

#[test]
fn pending_return_remains_visible_and_overdue_return_requires_recovery() {
    let mut c = controller(CoveragePolicy::AllowGap);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    c.acknowledge(1, departure.id, Acknowledgement::Completed)
        .unwrap();
    c.cancel(2).unwrap();
    let back = c
        .begin_return(2, PauseOutcome::Busy { retry_at: 3 })
        .unwrap();
    assert_eq!(back, None);
    assert_eq!(
        c.tick(3, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Cancelled))
    );
    assert_eq!(
        c.tick(52, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::RecoveryRequired)
    );
    assert_eq!(c.state(), ControllerState::RecoveryRequired);
}

#[test]
fn clock_regression_and_overflow_are_visible() {
    let mut c = controller(CoveragePolicy::AllowGap);
    assert_eq!(
        c.tick(0, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::Idle)
    );
    assert_eq!(
        c.tick(u64::MAX - 1, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::Idle)
    );
    assert_eq!(
        c.tick(u64::MAX - 2, CoverageEvidence { valid_until: None }),
        Err(ControllerError::ClockRegression {
            previous: u64::MAX - 1,
            received: u64::MAX - 2
        })
    );
    let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
    let mut c = Controller::new(
        ControllerConfig {
            home: HOME,
            pin: None,
            installed,
            coverage: CoveragePolicy::AllowGap,
            max_excursion_ms: u64::MAX,
            return_budget_ms: 1,
            max_defer_ms: 1,
            transition_timeout_ms: 1,
        },
        u64::MAX,
    )
    .unwrap();
    assert_eq!(
        c.request_excursion(
            u64::MAX,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable
        ),
        Err(ControllerError::TimeOverflow)
    );
}
