//! Excursion admission: pins, deferral bounds, session loss, and configuration.

use tulle::personality::{
    Acknowledgement, Controller, ControllerConfig, ControllerError, ControllerEvent,
    ControllerState, CoverageEvidence, CoveragePolicy, Excursion, InstalledPersonalitySet,
    InterruptionPolicy, PauseOutcome, PersonalityId, StopCapability,
};

use crate::{HOME, OTHER, controller, request};

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
