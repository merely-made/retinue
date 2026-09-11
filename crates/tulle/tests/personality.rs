//! Black-box acceptance for the radio-free personality controller.
//!
//! The fake adapters retain local state and expose pause outcomes. They do not
//! model a radio, a remote session, or protocol compatibility.

use tulle::personality::{
    Acknowledgement, Controller, ControllerConfig, ControllerError, ControllerEvent,
    ControllerState, CoverageEvidence, CoveragePolicy, Excursion, InstalledPersonalitySet,
    InterruptionPolicy, PauseOutcome, PersonalityId, ReturnReason, StopCapability,
};

const HOME: PersonalityId = PersonalityId(1);
const OTHER: PersonalityId = PersonalityId(2);

#[derive(Debug)]
struct FakeAdapter {
    id: PersonalityId,
    local_state: u32,
    pause: PauseOutcome,
    stop: StopCapability,
}

impl FakeAdapter {
    fn new(id: PersonalityId) -> Self {
        Self {
            id,
            local_state: 0,
            pause: PauseOutcome::Ready,
            stop: StopCapability::Resumable,
        }
    }
    fn pause(&self) -> PauseOutcome {
        self.pause
    }
    fn stop(&self) -> StopCapability {
        self.stop
    }
    fn visit(&mut self) {
        self.local_state += 1;
    }
}

fn controller(coverage: CoveragePolicy) -> Controller {
    let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
    Controller::new(
        ControllerConfig {
            home: HOME,
            pin: None,
            installed,
            coverage,
            max_excursion_ms: 100,
            return_budget_ms: 20,
            max_defer_ms: 10,
            transition_timeout_ms: 5,
        },
        0,
    )
    .unwrap()
}

fn request(policy: InterruptionPolicy) -> Excursion {
    Excursion {
        target: OTHER,
        duration_ms: 30,
        interruption: policy,
    }
}

#[test]
fn two_stateful_adapters_keep_local_state_across_a_normal_return() {
    let mut home = FakeAdapter::new(HOME);
    let mut other = FakeAdapter::new(OTHER);
    let mut c = controller(CoveragePolicy::AllowGap);
    let transition = c
        .request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            home.pause(),
            other.stop(),
        )
        .unwrap()
        .unwrap();
    c.acknowledge(1, transition.id, Acknowledgement::Completed)
        .unwrap();
    other.visit();
    assert_eq!(
        c.state(),
        ControllerState::Away {
            target: OTHER,
            excursion_deadline: 31,
            return_by: 51,
            interruption: InterruptionPolicy::ResumableOnly
        }
    );
    assert_eq!(other.local_state, 1);
    assert_eq!(
        c.tick(31, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::ReturnRequired(
            ReturnReason::ExcursionExpired
        ))
    );
    let back = c.begin_return(31, other.pause()).unwrap().unwrap();
    assert_eq!(back.from, OTHER);
    c.acknowledge(32, back.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(c.state(), ControllerState::Home);
    home.visit();
    let second = c
        .request_excursion(
            40,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            home.pause(),
            other.stop(),
        )
        .unwrap()
        .unwrap();
    c.acknowledge(41, second.id, Acknowledgement::Completed)
        .unwrap();
    other.visit();
    assert_eq!(other.local_state, 2);
    c.cancel(42).unwrap();
    let second_back = c.begin_return(42, other.pause()).unwrap().unwrap();
    c.acknowledge(43, second_back.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(home.local_state, 1);
    assert_eq!(other.local_state, 2);
    assert_eq!(home.id, HOME);
}

#[test]
fn pin_and_unsupported_targets_are_refused() {
    let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
    let pinned = Controller::new(
        ControllerConfig {
            home: HOME,
            pin: Some(HOME),
            installed,
            coverage: CoveragePolicy::AllowGap,
            max_excursion_ms: 10,
            return_budget_ms: 1,
            max_defer_ms: 1,
            transition_timeout_ms: 1,
        },
        0,
    )
    .unwrap();
    let mut c = pinned;
    assert_eq!(
        c.request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable
        ),
        Err(ControllerError::Pinned(HOME))
    );
    let mut c = controller(CoveragePolicy::AllowGap);
    assert_eq!(
        c.request_excursion(
            0,
            Excursion {
                target: PersonalityId(9),
                duration_ms: 1,
                interruption: InterruptionPolicy::ResumableOnly
            },
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready,
            StopCapability::Resumable
        ),
        Err(ControllerError::Unsupported(PersonalityId(9)))
    );
}

#[test]
fn busy_departure_is_deferred_only_within_bound_and_can_cancel() {
    let mut c = controller(CoveragePolicy::AllowGap);
    assert_eq!(
        c.request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::Busy { retry_at: 4 },
            StopCapability::Resumable
        ),
        Ok(None)
    );
    assert!(matches!(
        c.state(),
        ControllerState::DeferredExcursion { retry_at: 4, .. }
    ));
    assert_eq!(
        c.continue_excursion(
            4,
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready
        )
        .unwrap()
        .unwrap()
        .from,
        HOME
    );
    let mut c = controller(CoveragePolicy::AllowGap);
    c.request_excursion(
        0,
        request(InterruptionPolicy::ResumableOnly),
        CoverageEvidence { valid_until: None },
        PauseOutcome::Busy { retry_at: 4 },
        StopCapability::Resumable,
    )
    .unwrap();
    assert_eq!(c.cancel(1), Ok(ControllerEvent::Idle));
    assert_eq!(c.state(), ControllerState::Home);
}

#[test]
fn repeated_busy_reports_cannot_extend_the_original_deferral_bound() {
    let mut c = controller(CoveragePolicy::AllowGap);
    c.request_excursion(
        0,
        request(InterruptionPolicy::ResumableOnly),
        CoverageEvidence { valid_until: None },
        PauseOutcome::Busy { retry_at: 4 },
        StopCapability::Resumable,
    )
    .unwrap();
    assert_eq!(
        c.continue_excursion(
            4,
            CoverageEvidence { valid_until: None },
            PauseOutcome::Busy { retry_at: 9 }
        ),
        Ok(None)
    );
    assert_eq!(
        c.continue_excursion(
            11,
            CoverageEvidence { valid_until: None },
            PauseOutcome::Ready
        ),
        Err(ControllerError::DeferDeadlineExceeded)
    );
    assert_eq!(c.state(), ControllerState::Home);
}

#[test]
fn session_loss_requires_explicit_permission_on_departure_and_return() {
    let mut c = controller(CoveragePolicy::AllowGap);
    assert_eq!(
        c.request_excursion(
            0,
            request(InterruptionPolicy::ResumableOnly),
            CoverageEvidence { valid_until: None },
            PauseOutcome::RequiresSessionLoss {
                affected_sessions: 2
            },
            StopCapability::Resumable
        ),
        Err(ControllerError::SessionLossNotAllowed {
            affected_sessions: 2
        })
    );
    let mut c = controller(CoveragePolicy::AllowGap);
    let t = c
        .request_excursion(
            0,
            request(InterruptionPolicy::AllowSessionLoss),
            CoverageEvidence { valid_until: None },
            PauseOutcome::RequiresSessionLoss {
                affected_sessions: 2,
            },
            StopCapability::RequiresSessionLoss,
        )
        .unwrap()
        .unwrap();
    c.acknowledge(1, t.id, Acknowledgement::Completed).unwrap();
    c.cancel(2).unwrap();
    assert!(
        c.begin_return(
            2,
            PauseOutcome::RequiresSessionLoss {
                affected_sessions: 2
            }
        )
        .unwrap()
        .is_some()
    );
}

#[test]
fn cancellation_after_activation_requests_return_and_ack_is_required() {
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
    assert_eq!(
        c.cancel(2),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Cancelled))
    );
    let back = c.begin_return(2, PauseOutcome::Ready).unwrap().unwrap();
    assert!(matches!(c.state(), ControllerState::Transitioning { .. }));
    assert_eq!(
        c.acknowledge(3, back.id + 99, Acknowledgement::Completed),
        Err(ControllerError::WrongAcknowledgement {
            expected: back.id,
            received: back.id + 99
        })
    );
    assert!(matches!(c.state(), ControllerState::Transitioning { .. }));
    c.acknowledge(3, back.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(c.state(), ControllerState::Home);
}

#[test]
fn explicit_finish_uses_completed_return_reason() {
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
    assert_eq!(
        c.finish(2),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Completed))
    );
    let back = c.begin_return(2, PauseOutcome::Ready).unwrap().unwrap();
    c.acknowledge(3, back.id, Acknowledgement::Completed)
        .unwrap();
    assert_eq!(c.state(), ControllerState::Home);
}

#[test]
fn cancellation_during_departure_acknowledges_only_into_a_return_obligation() {
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
        c.cancel(1),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Cancelled))
    );
    c.acknowledge(2, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert!(matches!(
        c.state(),
        ControllerState::ReturnRequired {
            reason: ReturnReason::Cancelled,
            ..
        }
    ));
}

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
fn cancellation_during_activation_is_visible_before_any_home_report() {
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
    c.cancel(1).unwrap();
    assert_eq!(
        c.tick(2, CoverageEvidence { valid_until: None }),
        Ok(ControllerEvent::ReturnRequired(ReturnReason::Cancelled))
    );
    assert!(!matches!(c.state(), ControllerState::Home));
    c.acknowledge(3, departure.id, Acknowledgement::Completed)
        .unwrap();
    assert!(!matches!(c.state(), ControllerState::Home));
}

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
fn constructor_rejects_uninstalled_home_and_pin() {
    let installed = InstalledPersonalitySet::new(&[OTHER]).unwrap();
    let config = ControllerConfig {
        home: HOME,
        pin: None,
        installed,
        coverage: CoveragePolicy::AllowGap,
        max_excursion_ms: 1,
        return_budget_ms: 1,
        max_defer_ms: 1,
        transition_timeout_ms: 1,
    };
    assert!(matches!(
        Controller::new(config, 0),
        Err(tulle::personality::ConfigError::HomeNotInstalled)
    ));
    let installed = InstalledPersonalitySet::new(&[HOME]).unwrap();
    let config = ControllerConfig {
        home: HOME,
        pin: Some(OTHER),
        installed,
        coverage: CoveragePolicy::AllowGap,
        max_excursion_ms: 1,
        return_budget_ms: 1,
        max_defer_ms: 1,
        transition_timeout_ms: 1,
    };
    assert!(matches!(
        Controller::new(config, 0),
        Err(tulle::personality::ConfigError::PinNotInstalled(OTHER))
    ));
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
