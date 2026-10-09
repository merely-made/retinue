//! Coverage policy and latched coverage-loss returns.

use tulle::personality::{
    Acknowledgement, ControllerError, ControllerEvent, ControllerState, CoverageEvidence,
    CoveragePolicy, InterruptionPolicy, PauseOutcome, ReturnReason, StopCapability,
};

use crate::{controller, request};

#[test]
fn coverage_loss_during_activation_is_latched_until_return() {
    let mut c = controller(CoveragePolicy::RequireCoverage);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence {
                valid_until: Some(55),
            },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        c.tick(
            2,
            CoverageEvidence {
                valid_until: Some(1)
            }
        ),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::CoverageLost))
    );
    c.acknowledge(3, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert!(matches!(
        c.state(),
        ControllerState::ReturnRequired {
            reason: ReturnReason::CoverageLost,
            ..
        }
    ));
}

#[test]
fn current_but_short_coverage_during_activation_still_latches_return() {
    let mut c = controller(CoveragePolicy::RequireCoverage);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence {
                valid_until: Some(55),
            },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        c.tick(
            2,
            CoverageEvidence {
                valid_until: Some(2)
            }
        ),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::CoverageLost))
    );
    c.acknowledge(3, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert!(matches!(
        c.state(),
        ControllerState::ReturnRequired {
            reason: ReturnReason::CoverageLost,
            ..
        }
    ));
}

#[test]
fn recovered_coverage_cannot_clear_a_latched_activation_return() {
    let mut c = controller(CoveragePolicy::RequireCoverage);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence {
                valid_until: Some(55),
            },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        c.tick(
            2,
            CoverageEvidence {
                valid_until: Some(1)
            }
        ),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::CoverageLost))
    );
    c.acknowledge(3, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(
        c.tick(
            4,
            CoverageEvidence {
                valid_until: Some(55)
            }
        ),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::CoverageLost))
    );
}

#[test]
fn recovered_coverage_cannot_clear_a_latched_activation_cancellation() {
    let mut c = controller(CoveragePolicy::RequireCoverage);
    let departure = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence {
                valid_until: Some(55),
            },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        c.cancel(1),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Cancelled))
    );
    c.acknowledge(2, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(
        c.tick(
            3,
            CoverageEvidence {
                valid_until: Some(55)
            }
        ),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Cancelled))
    );
}

#[test]
fn required_coverage_expires_but_optional_gap_is_allowed() {
    let mut required = controller(CoveragePolicy::RequireCoverage);
    assert_eq!(
        required.request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence {
                valid_until: Some(54)
            },
            PauseOutcome::Ready,
            StopCapability::Resumable
        ),
        Err(ControllerError::CoverageUnavailable)
    );
    let t = required
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence {
                valid_until: Some(55),
            },
            PauseOutcome::Ready,
            StopCapability::Resumable,
        )
        .unwrap()
        .unwrap();
    required
        .acknowledge(1, t.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(
        required.tick(
            2,
            CoverageEvidence {
                valid_until: Some(20)
            }
        ),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::CoverageLost))
    );
    let mut optional = controller(CoveragePolicy::AllowGap);
    assert!(
        optional
            .request_excursion(
                0,
                request(InterruptionPolicy::ResumableOnly),
                CoverageEvidence { valid_until: None },
                PauseOutcome::Ready,
                StopCapability::Resumable
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn deferred_required_coverage_is_revalidated_with_fresh_evidence() {
    let mut c = controller(CoveragePolicy::RequireCoverage);
    c.request_excursion(
        0,
        request(InterruptionPolicy::ResumableOnly),
        CoverageEvidence {
            valid_until: Some(55),
        },
        PauseOutcome::Busy { retry_at: 4 },
        StopCapability::Resumable,
    )
    .unwrap();
    assert_eq!(
        c.continue_excursion(
            4,
            CoverageEvidence {
                valid_until: Some(4)
            },
            PauseOutcome::Ready
        ),
        Err(ControllerError::CoverageUnavailable)
    );
    assert!(matches!(
        c.state(),
        ControllerState::DeferredExcursion { .. }
    ));
}
