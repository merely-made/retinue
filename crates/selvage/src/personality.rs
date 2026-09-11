//! Radio-free coordination for deliberate personality excursions.
//!
//! This is a decision model, not a radio owner or protocol adapter. Callers own
//! adapter state, query adapters for readiness without changing adapter state, and report explicit hardware completion.
//! `radio-hand` remains the owner of hardware, keeper scheduling, and recovery.
//! Caller-supplied coverage proves neither RF reception nor authentication.
//!
//! Configuration is immutable in this first slice. A pin must equal home: it
//! means a dedicated selected home, rather than silently restoring a different
//! configured personality. All timestamps are caller-monotonic milliseconds;
//! construction assumes the caller's hardware owner has already confirmed home.

pub const MAX_INSTALLED_PERSONALITIES: usize = 8;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PersonalityId(pub u8);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InstalledPersonalitySet {
    entries: [Option<PersonalityId>; MAX_INSTALLED_PERSONALITIES],
    len: usize,
}

impl InstalledPersonalitySet {
    pub fn new(entries: &[PersonalityId]) -> Result<Self, ConfigError> {
        if entries.len() > MAX_INSTALLED_PERSONALITIES {
            return Err(ConfigError::TooManyInstalled);
        }
        let mut result = Self {
            entries: [None; MAX_INSTALLED_PERSONALITIES],
            len: 0,
        };
        for &entry in entries {
            if result.contains(entry) {
                return Err(ConfigError::DuplicateInstalled(entry));
            }
            result.entries[result.len] = Some(entry);
            result.len += 1;
        }
        Ok(result)
    }
    pub fn contains(&self, personality: PersonalityId) -> bool {
        self.entries[..self.len].contains(&Some(personality))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterruptionPolicy {
    ResumableOnly,
    AllowSessionLoss,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopCapability {
    Resumable,
    RequiresSessionLoss,
    Unsupported,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseOutcome {
    Ready,
    Busy { retry_at: u64 },
    RequiresSessionLoss { affected_sessions: u16 },
    Unsupported,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CoveragePolicy {
    AllowGap,
    RequireCoverage,
}

/// Finite caller-supplied evidence. It is neither a radio receipt nor proof of peer identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoverageEvidence {
    pub valid_until: Option<u64>,
}
impl CoverageEvidence {
    pub fn is_valid_at(self, now: u64) -> bool {
        self.valid_until.is_some_and(|until| until >= now)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerConfig {
    pub home: PersonalityId,
    pub pin: Option<PersonalityId>,
    pub installed: InstalledPersonalitySet,
    pub coverage: CoveragePolicy,
    pub max_excursion_ms: u64,
    pub return_budget_ms: u64,
    pub max_defer_ms: u64,
    pub transition_timeout_ms: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Excursion {
    pub target: PersonalityId,
    pub duration_ms: u64,
    pub interruption: InterruptionPolicy,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Transition {
    pub id: u64,
    pub from: PersonalityId,
    pub to: PersonalityId,
    pub deadline: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acknowledgement {
    Completed,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnReason {
    /// The excursion's requested work completed successfully.
    Completed,
    Cancelled,
    ExcursionExpired,
    CoverageLost,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerState {
    Home,
    DeferredExcursion {
        request: Excursion,
        coverage: CoverageEvidence,
        retry_at: u64,
        defer_deadline: u64,
    },
    Transitioning {
        transition: Transition,
        request: Option<Excursion>,
        pending_return: Option<ReturnReason>,
    },
    Away {
        target: PersonalityId,
        excursion_deadline: u64,
        return_by: u64,
        interruption: InterruptionPolicy,
    },
    ReturnRequired {
        target: PersonalityId,
        reason: ReturnReason,
        return_by: u64,
        interruption: InterruptionPolicy,
    },
    DeferredReturn {
        target: PersonalityId,
        reason: ReturnReason,
        retry_at: u64,
        return_by: u64,
        interruption: InterruptionPolicy,
    },
    RecoveryRequired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerEvent {
    Idle,
    ReturnRequired(ReturnReason),
    RecoveryRequired,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfigError {
    TooManyInstalled,
    DuplicateInstalled(PersonalityId),
    HomeNotInstalled,
    PinNotInstalled(PersonalityId),
    PinDiffersFromHome {
        pin: PersonalityId,
        home: PersonalityId,
    },
    ZeroDuration,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControllerError {
    ClockRegression { previous: u64, received: u64 },
    TimeOverflow,
    NotHome,
    NotReturning,
    RecoveryRequired,
    Unsupported(PersonalityId),
    Pinned(PersonalityId),
    HomeIsNotAnExcursion,
    ExcursionTooLong,
    CoverageUnavailable,
    TargetCannotReturn,
    SessionLossNotAllowed { affected_sessions: u16 },
    PauseUnsupported,
    RetryBeforeNow { retry_at: u64, now: u64 },
    DeferDeadlineExceeded,
    WrongAcknowledgement { expected: u64, received: u64 },
}

/// Dependency-free controller. It never receives packets and never touches radio hardware.
#[derive(Debug, Eq, PartialEq)]
pub struct Controller {
    config: ControllerConfig,
    state: ControllerState,
    next_transition_id: u64,
    last_now: u64,
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

    fn pause_for_departure(
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

    fn pause_for_return(
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
    fn validate_return_capability(
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
    fn validate_coverage(
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
    fn activation_coverage_by(
        &self,
        transition: Transition,
        request: Excursion,
    ) -> Result<u64, ControllerError> {
        self.add(
            self.add(transition.deadline, request.duration_ms)?,
            self.config.return_budget_ms,
        )
    }
    fn cap_return_by(&self, now: u64, original: u64) -> Result<u64, ControllerError> {
        Ok(match now.checked_add(self.config.return_budget_ms) {
            Some(candidate) => original.min(candidate),
            None => original,
        })
    }
    fn start_transition(
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
    fn accept_time(&mut self, now: u64) -> Result<(), ControllerError> {
        if now < self.last_now {
            return Err(ControllerError::ClockRegression {
                previous: self.last_now,
                received: now,
            });
        }
        self.last_now = now;
        Ok(())
    }
    fn add(&self, left: u64, right: u64) -> Result<u64, ControllerError> {
        left.checked_add(right).ok_or(ControllerError::TimeOverflow)
    }
}
