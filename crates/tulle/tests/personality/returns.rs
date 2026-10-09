//! Normal, finished, and cancelled returns home.

use tulle::personality::{
    Acknowledgement, ControllerError, ControllerEvent, ControllerState, CoverageEvidence,
    CoveragePolicy, InterruptionPolicy, PauseOutcome, ReturnReason, StopCapability,
};

use crate::{FakeAdapter, HOME, OTHER, controller, request};

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
