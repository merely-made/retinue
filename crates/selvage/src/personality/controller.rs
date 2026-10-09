//! The controller and its caller-facing operations.

use super::{
    Acknowledgement, ConfigError, ControllerConfig, ControllerError, ControllerEvent,
    ControllerState, CoverageEvidence, CoveragePolicy, Excursion, PauseOutcome, ReturnReason,
    StopCapability, Transition,
};

/// Dependency-free controller. It never receives packets and never touches radio hardware.
#[derive(Debug, Eq, PartialEq)]
pub struct Controller {
    pub(super) config: ControllerConfig,
    pub(super) state: ControllerState,
    pub(super) next_transition_id: u64,
    pub(super) last_now: u64,
}

impl Controller {
    pub fn new(config: ControllerConfig, now: u64) -> Result<Self, ConfigError> {
        if !config.installed.contains(config.home) {
            return Err(ConfigError::HomeNotInstalled);
        }
        if let Some(pin) = config.pin {
            if !config.installed.contains(pin) {
                return Err(ConfigError::PinNotInstalled(pin));
            }
            if pin != config.home {
                return Err(ConfigError::PinDiffersFromHome {
                    pin,
                    home: config.home,
                });
            }
        }
        if config.max_excursion_ms == 0
            || config.return_budget_ms == 0
            || config.max_defer_ms == 0
            || config.transition_timeout_ms == 0
        {
            return Err(ConfigError::ZeroDuration);
        }
        Ok(Self {
            config,
            state: ControllerState::Home,
            next_transition_id: 1,
            last_now: now,
        })
    }
    pub fn config(&self) -> &ControllerConfig {
        &self.config
    }
    pub fn state(&self) -> ControllerState {
        self.state
    }

    /// Admits an excursion after home pause, target return capability, coverage and policy checks.
    pub fn request_excursion(
        &mut self,
        now: u64,
        request: Excursion,
        coverage: CoverageEvidence,
        home_pause: PauseOutcome,
        target_stop: StopCapability,
    ) -> Result<Option<Transition>, ControllerError> {
        self.accept_time(now)?;
        if self.state != ControllerState::Home {
            return Err(ControllerError::NotHome);
        }
        if self.config.pin.is_some() {
            return Err(ControllerError::Pinned(self.config.home));
        }
        if request.target == self.config.home {
            return Err(ControllerError::HomeIsNotAnExcursion);
        }
        if !self.config.installed.contains(request.target) {
            return Err(ControllerError::Unsupported(request.target));
        }
        if request.duration_ms == 0 || request.duration_ms > self.config.max_excursion_ms {
            return Err(ControllerError::ExcursionTooLong);
        }
        self.validate_return_capability(request.interruption, target_stop)?;
        self.validate_coverage(now, request.duration_ms, coverage)?;
        self.pause_for_departure(now, request, coverage, home_pause, None)
    }

    /// Rechecks a finite deferral with a fresh caller-owned home adapter result.
    pub fn continue_excursion(
        &mut self,
        now: u64,
        coverage: CoverageEvidence,
        home_pause: PauseOutcome,
    ) -> Result<Option<Transition>, ControllerError> {
        self.accept_time(now)?;
        let (request, defer_deadline) = match self.state {
            ControllerState::DeferredExcursion {
                request,
                defer_deadline,
                ..
            } => (request, defer_deadline),
            _ => return Err(ControllerError::NotHome),
        };
        if now > defer_deadline {
            self.state = ControllerState::Home;
            return Err(ControllerError::DeferDeadlineExceeded);
        }
        self.validate_coverage(now, request.duration_ms, coverage)?;
        self.pause_for_departure(now, request, coverage, home_pause, Some(defer_deadline))
    }

    /// Marks an active excursion's requested work as complete and requests return home.
    pub fn finish(&mut self, now: u64) -> Result<ControllerEvent, ControllerError> {
        if !matches!(self.state, ControllerState::Away { .. }) {
            return Err(ControllerError::NotReturning);
        }
        self.request_return(now, ReturnReason::Completed)
    }

    /// Cancels a deferred request or makes an active excursion return-required.
    pub fn cancel(&mut self, now: u64) -> Result<ControllerEvent, ControllerError> {
        self.request_return(now, ReturnReason::Cancelled)
    }

    fn request_return(
        &mut self,
        now: u64,
        reason: ReturnReason,
    ) -> Result<ControllerEvent, ControllerError> {
        self.accept_time(now)?;
        match self.state {
            ControllerState::Away {
                target,
                return_by,
                interruption,
                ..
            } => {
                if now > return_by {
                    self.state = ControllerState::RecoveryRequired;
                    return Ok(ControllerEvent::RecoveryRequired);
                }
                let return_by = self.cap_return_by(now, return_by)?;
                self.state = ControllerState::ReturnRequired {
                    target,
                    reason,
                    return_by,
                    interruption,
                };
                Ok(ControllerEvent::ReturnRequired(reason))
            }
            ControllerState::Transitioning {
                request: Some(_), ..
            } => {
                if let ControllerState::Transitioning { pending_return, .. } = &mut self.state {
                    *pending_return = Some(reason);
                }
                Ok(ControllerEvent::ReturnRequired(reason))
            }
            ControllerState::DeferredExcursion { .. } => {
                self.state = ControllerState::Home;
                Ok(ControllerEvent::Idle)
            }
            _ => Err(ControllerError::NotReturning),
        }
    }

    /// Starts the return only after the target adapter reports its actual pause result.
    pub fn begin_return(
        &mut self,
        now: u64,
        target_pause: PauseOutcome,
    ) -> Result<Option<Transition>, ControllerError> {
        self.accept_time(now)?;
        let (target, reason, return_by, interruption) = match self.state {
            ControllerState::ReturnRequired {
                target,
                reason,
                return_by,
                interruption,
                ..
            }
            | ControllerState::DeferredReturn {
                target,
                reason,
                return_by,
                interruption,
                ..
            } => (target, reason, return_by, interruption),
            _ => return Err(ControllerError::NotReturning),
        };
        if now > return_by {
            self.state = ControllerState::RecoveryRequired;
            return Ok(None);
        }
        self.pause_for_return(now, target, reason, return_by, interruption, target_pause)
    }

    /// Advances deadlines. `ReturnRequired` must be followed by `begin_return` with real adapter state.
    pub fn tick(
        &mut self,
        now: u64,
        coverage: CoverageEvidence,
    ) -> Result<ControllerEvent, ControllerError> {
        self.accept_time(now)?;
        match self.state {
            ControllerState::Transitioning { transition, .. } if now > transition.deadline => {
                self.state = ControllerState::RecoveryRequired;
                Ok(ControllerEvent::RecoveryRequired)
            }
            ControllerState::Transitioning {
                transition,
                request: Some(request),
                ..
            } if self.config.coverage == CoveragePolicy::RequireCoverage => {
                let coverage_by = match self.activation_coverage_by(transition, request) {
                    Ok(coverage_by) => coverage_by,
                    Err(error) => {
                        self.state = ControllerState::RecoveryRequired;
                        return Err(error);
                    }
                };
                if !coverage.is_valid_at(coverage_by) {
                    if let ControllerState::Transitioning { pending_return, .. } = &mut self.state {
                        *pending_return = Some(ReturnReason::CoverageLost);
                    }
                    Ok(ControllerEvent::ReturnRequired(ReturnReason::CoverageLost))
                } else {
                    match self.state {
                        ControllerState::Transitioning {
                            pending_return: Some(reason),
                            ..
                        } => Ok(ControllerEvent::ReturnRequired(reason)),
                        _ => Ok(ControllerEvent::Idle),
                    }
                }
            }
            ControllerState::Transitioning {
                pending_return: Some(reason),
                ..
            } => Ok(ControllerEvent::ReturnRequired(reason)),
            ControllerState::DeferredExcursion { defer_deadline, .. } if now > defer_deadline => {
                self.state = ControllerState::Home;
                Ok(ControllerEvent::Idle)
            }
            ControllerState::DeferredReturn { return_by, .. } if now > return_by => {
                self.state = ControllerState::RecoveryRequired;
                Ok(ControllerEvent::RecoveryRequired)
            }
            ControllerState::ReturnRequired { return_by, .. } if now > return_by => {
                self.state = ControllerState::RecoveryRequired;
                Ok(ControllerEvent::RecoveryRequired)
            }
            ControllerState::Away { return_by, .. } if now > return_by => {
                self.state = ControllerState::RecoveryRequired;
                Ok(ControllerEvent::RecoveryRequired)
            }
            ControllerState::Away {
                target,
                excursion_deadline,
                return_by,
                interruption,
            } => {
                let reason = if now >= excursion_deadline {
                    Some(ReturnReason::ExcursionExpired)
                } else if self.config.coverage == CoveragePolicy::RequireCoverage
                    && !coverage.is_valid_at(return_by)
                {
                    Some(ReturnReason::CoverageLost)
                } else {
                    None
                };
                if let Some(reason) = reason {
                    let return_by = self.cap_return_by(now, return_by)?;
                    self.state = ControllerState::ReturnRequired {
                        target,
                        reason,
                        return_by,
                        interruption,
                    };
                    Ok(ControllerEvent::ReturnRequired(reason))
                } else {
                    Ok(ControllerEvent::Idle)
                }
            }
            ControllerState::ReturnRequired { reason, .. } => {
                Ok(ControllerEvent::ReturnRequired(reason))
            }
            ControllerState::DeferredReturn { reason, .. } => {
                Ok(ControllerEvent::ReturnRequired(reason))
            }
            ControllerState::RecoveryRequired => Ok(ControllerEvent::RecoveryRequired),
            _ => Ok(ControllerEvent::Idle),
        }
    }

    /// Completes only a matching, current transition. The radio owner must
    /// correlate this ID with the exact commanded target and observed result;
    /// this model does not authenticate endpoints. A stale acknowledgement
    /// changes nothing.
    pub fn acknowledge(
        &mut self,
        now: u64,
        id: u64,
        acknowledgement: Acknowledgement,
    ) -> Result<(), ControllerError> {
        self.accept_time(now)?;
        let (transition, request, pending_return) = match self.state {
            ControllerState::Transitioning {
                transition,
                request,
                pending_return,
            } => (transition, request, pending_return),
            _ => {
                return Err(ControllerError::WrongAcknowledgement {
                    expected: 0,
                    received: id,
                });
            }
        };
        if id != transition.id {
            return Err(ControllerError::WrongAcknowledgement {
                expected: transition.id,
                received: id,
            });
        }
        if now > transition.deadline || acknowledgement != Acknowledgement::Completed {
            self.state = ControllerState::RecoveryRequired;
            return Ok(());
        }
        if transition.to == self.config.home {
            self.state = ControllerState::Home;
        } else {
            let request = request.expect("outbound transitions retain their explicit request");
            let excursion_deadline = self.add(now, request.duration_ms)?;
            let return_by = self.add(excursion_deadline, self.config.return_budget_ms)?;
            self.state = if let Some(reason) = pending_return {
                ControllerState::ReturnRequired {
                    target: transition.to,
                    reason,
                    return_by: self.cap_return_by(now, return_by)?,
                    interruption: request.interruption,
                }
            } else {
                ControllerState::Away {
                    target: transition.to,
                    excursion_deadline,
                    return_by,
                    interruption: request.interruption,
                }
            };
        }
        Ok(())
    }
}
