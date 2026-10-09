//! The controller's internal steps: pauses, transitions, deadlines and checks.

use super::{
    Controller, ControllerError, ControllerState, CoverageEvidence, CoveragePolicy, Excursion,
    InterruptionPolicy, PauseOutcome, PersonalityId, ReturnReason, StopCapability, Transition,
};

impl Controller {
    pub(super) fn pause_for_departure(
        &mut self,
        now: u64,
        request: Excursion,
        coverage: CoverageEvidence,
        outcome: PauseOutcome,
        defer_deadline: Option<u64>,
    ) -> Result<Option<Transition>, ControllerError> {
        match outcome {
            PauseOutcome::Ready => self
                .start_transition(now, self.config.home, request.target, Some(request), None)
                .map(Some),
            PauseOutcome::Busy { retry_at } => {
                if retry_at < now {
                    return Err(ControllerError::RetryBeforeNow { retry_at, now });
                }
                let defer_deadline = match defer_deadline {
                    Some(deadline) => deadline,
                    None => self.add(now, self.config.max_defer_ms)?,
                };
                if retry_at > defer_deadline {
                    return Err(ControllerError::DeferDeadlineExceeded);
                }
                self.state = ControllerState::DeferredExcursion {
                    request,
                    coverage,
                    retry_at,
                    defer_deadline,
                };
                Ok(None)
            }
            PauseOutcome::RequiresSessionLoss { .. }
                if request.interruption == InterruptionPolicy::AllowSessionLoss =>
            {
                self.start_transition(now, self.config.home, request.target, Some(request), None)
                    .map(Some)
            }
            PauseOutcome::RequiresSessionLoss { affected_sessions } => {
                Err(ControllerError::SessionLossNotAllowed { affected_sessions })
            }
            PauseOutcome::Unsupported => Err(ControllerError::PauseUnsupported),
        }
    }

    pub(super) fn pause_for_return(
        &mut self,
        now: u64,
        target: PersonalityId,
        reason: ReturnReason,
        return_by: u64,
        interruption: InterruptionPolicy,
        outcome: PauseOutcome,
    ) -> Result<Option<Transition>, ControllerError> {
        match outcome {
            PauseOutcome::Ready => self
                .start_transition(now, target, self.config.home, None, Some(return_by))
                .map(Some),
            PauseOutcome::Busy { retry_at } => {
                if retry_at < now {
                    return Err(ControllerError::RetryBeforeNow { retry_at, now });
                }
                if retry_at > return_by {
                    self.state = ControllerState::RecoveryRequired;
                    return Err(ControllerError::DeferDeadlineExceeded);
                }
                self.state = ControllerState::DeferredReturn {
                    target,
                    reason,
                    retry_at,
                    return_by,
                    interruption,
                };
                Ok(None)
            }
            PauseOutcome::RequiresSessionLoss { .. }
                if interruption == InterruptionPolicy::AllowSessionLoss =>
            {
                self.start_transition(now, target, self.config.home, None, Some(return_by))
                    .map(Some)
            }
            PauseOutcome::RequiresSessionLoss { affected_sessions } => {
                Err(ControllerError::SessionLossNotAllowed { affected_sessions })
            }
            PauseOutcome::Unsupported => Err(ControllerError::PauseUnsupported),
        }
    }
    pub(super) fn validate_return_capability(
        &self,
        policy: InterruptionPolicy,
        capability: StopCapability,
    ) -> Result<(), ControllerError> {
        match capability {
            StopCapability::Resumable => Ok(()),
            StopCapability::RequiresSessionLoss
                if policy == InterruptionPolicy::AllowSessionLoss =>
            {
                Ok(())
            }
            StopCapability::RequiresSessionLoss => Err(ControllerError::SessionLossNotAllowed {
                affected_sessions: 0,
            }),
            StopCapability::Unsupported => Err(ControllerError::TargetCannotReturn),
        }
    }
    pub(super) fn validate_coverage(
        &self,
        now: u64,
        duration_ms: u64,
        coverage: CoverageEvidence,
    ) -> Result<(), ControllerError> {
        let coverage_by = self.add(
            self.add(
                self.add(now, self.config.transition_timeout_ms)?,
                duration_ms,
            )?,
            self.config.return_budget_ms,
        )?;
        if self.config.coverage == CoveragePolicy::RequireCoverage
            && !coverage.is_valid_at(coverage_by)
        {
            return Err(ControllerError::CoverageUnavailable);
        }
        Ok(())
    }
    pub(super) fn activation_coverage_by(
        &self,
        transition: Transition,
        request: Excursion,
    ) -> Result<u64, ControllerError> {
        self.add(
            self.add(transition.deadline, request.duration_ms)?,
            self.config.return_budget_ms,
        )
    }
    pub(super) fn cap_return_by(&self, now: u64, original: u64) -> Result<u64, ControllerError> {
        Ok(match now.checked_add(self.config.return_budget_ms) {
            Some(candidate) => original.min(candidate),
            None => original,
        })
    }
    pub(super) fn start_transition(
        &mut self,
        now: u64,
        from: PersonalityId,
        to: PersonalityId,
        request: Option<Excursion>,
        cap_deadline: Option<u64>,
    ) -> Result<Transition, ControllerError> {
        let id = self.next_transition_id;
        self.next_transition_id = self
            .next_transition_id
            .checked_add(1)
            .ok_or(ControllerError::TimeOverflow)?;
        let deadline = self.add(now, self.config.transition_timeout_ms)?;
        let transition = Transition {
            id,
            from,
            to,
            deadline: cap_deadline.map_or(deadline, |cap| deadline.min(cap)),
        };
        self.state = ControllerState::Transitioning {
            transition,
            request,
            pending_return: None,
        };
        Ok(transition)
    }
    pub(super) fn accept_time(&mut self, now: u64) -> Result<(), ControllerError> {
        if now < self.last_now {
            return Err(ControllerError::ClockRegression {
                previous: self.last_now,
                received: now,
            });
        }
        self.last_now = now;
        Ok(())
    }
    pub(super) fn add(&self, left: u64, right: u64) -> Result<u64, ControllerError> {
        left.checked_add(right).ok_or(ControllerError::TimeOverflow)
    }
}
