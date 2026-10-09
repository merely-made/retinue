//! Profile switching: excursion assessment, pause and suspend, and acknowledged transitions.

use super::*;

impl Runtime {
    fn assessment(
        &self,
        now: u64,
        id: PersonalityId,
        return_by: u64,
        allow: bool,
    ) -> Result<PauseOutcome, Error> {
        if self.work.inflight().is_some() {
            return Ok(PauseOutcome::Busy {
                retry_at: now
                    .checked_add(self.config.tx_budget_ms)
                    .ok_or(Error::TimeOverflow)?,
            });
        }
        let outcome = match id {
            RETINUE => match self.retinue.assess_pause(now, return_by) {
                Ok(_) => PauseOutcome::Ready,
                Err(retinue::instance::InstanceError::PauseBlocked(reason)) => match reason {
                    PauseBlocked::ClockBeforeLinkActivity { .. }
                    | PauseBlocked::LinkExpiryOverflow
                    | PauseBlocked::ReturnBoundBeforeNow { .. } => {
                        return Err(Error::Retinue(
                            retinue::instance::InstanceError::PauseBlocked(reason),
                        ));
                    }
                    _ => PauseOutcome::RequiresSessionLoss {
                        affected_sessions: 1,
                    },
                },
                Err(e) => return Err(e.into()),
            },
            SENNET => match self.sennet.assess_pause(now, return_by)? {
                sennet::instance::PauseOutcome::Ready => PauseOutcome::Ready,
                sennet::instance::PauseOutcome::Busy { retry_at } => {
                    PauseOutcome::Busy { retry_at }
                }
                sennet::instance::PauseOutcome::RequiresLoss { pending } => {
                    PauseOutcome::RequiresSessionLoss {
                        affected_sessions: pending as u16,
                    }
                }
            },
            TUCKET => match self.tucket.assess_pause(now, return_by)? {
                tucket::instance::PauseAssessment::Ready => PauseOutcome::Ready,
                tucket::instance::PauseAssessment::Busy { retry_at } => PauseOutcome::Busy {
                    retry_at: retry_at.max(now),
                },
                tucket::instance::PauseAssessment::RequiresLoss { pending } => {
                    PauseOutcome::RequiresSessionLoss {
                        affected_sessions: pending.len() as u16,
                    }
                }
            },
            _ => return Err(Error::WrongInstance),
        };
        // Validate the protocol's clock and lifecycle before queue loss can
        // authorize departure. Queued work cannot hide a protocol refusal.
        if !self.work.is_empty() {
            return Ok(PauseOutcome::RequiresSessionLoss {
                affected_sessions: self.work.len() as u16,
            });
        }
        Ok(if allow && matches!(outcome, PauseOutcome::Busy { .. }) {
            PauseOutcome::RequiresSessionLoss {
                affected_sessions: 1,
            }
        } else {
            outcome
        })
    }
    pub(super) fn pause(
        &mut self,
        now: u64,
        id: PersonalityId,
        return_by: u64,
        allow: bool,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<(), Error> {
        match id {
            RETINUE => {
                let permission = if allow {
                    InterruptionPermission::AllowSessionLoss
                } else {
                    InterruptionPermission::PreserveSessions
                };
                if let Some(report) = self.retinue.pause(now, return_by, permission, iv)? {
                    self.event(Event::RetinueInterrupted(report));
                }
            }
            SENNET => {
                if allow
                    && self.sennet.pending_identity().is_some()
                    && let Some(e) = self
                        .sennet
                        .discard_pending(now, sennet::instance::LossPermission::operator())?
                {
                    self.event(Event::Sennet(e));
                }
                self.sennet.pause(now, return_by)?;
            }
            TUCKET => {
                if allow
                    && self.tucket.assess_pause(now, return_by)?
                        != tucket::instance::PauseAssessment::Ready
                {
                    let r = self
                        .tucket
                        .interrupt(tucket::instance::LossPermission::Allow)?;
                    self.tucket_lost(now, r)?;
                }
                self.tucket.pause(now, return_by)?;
            }
            _ => return Err(Error::WrongInstance),
        }
        Ok(())
    }
    fn suspend(
        &mut self,
        now: u64,
        t: Transition,
        return_by: u64,
        allow: bool,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<(), Error> {
        for id in self.work.deactivate(now, allow)? {
            self.settle_loss(now, id)?;
        }
        self.pause(now, t.from, return_by, allow, iv)
    }
    fn return_bound(&self, now: u64, request: Excursion) -> Result<u64, Error> {
        now.checked_add(self.config.controller.transition_timeout_ms)
            .and_then(|n| n.checked_add(request.duration_ms))
            .and_then(|n| n.checked_add(self.config.controller.return_budget_ms))
            .ok_or(Error::TimeOverflow)
    }
    pub fn request(
        &mut self,
        now: u64,
        request: Excursion,
        iv: impl FnMut() -> [u8; 16],
    ) -> Result<Step, Error> {
        self.time(now)?;
        if self.state() != ControllerState::Home {
            return Err(ControllerError::NotHome.into());
        }
        self.expiry(now)?;
        let bound = self.return_bound(now, request)?;
        let allow = request.interruption == InterruptionPolicy::AllowSessionLoss;
        let pause = self.assessment(now, self.config.controller.home, bound, allow)?;
        // Inbound Retinue sessions can arrive unsolicited while away.
        let stop = if request.target == RETINUE {
            StopCapability::RequiresSessionLoss
        } else {
            StopCapability::Resumable
        };
        let transition = self.controller.request_excursion(
            now,
            request,
            CoverageEvidence { valid_until: None },
            pause,
            stop,
        )?;
        if let Some(t) = transition {
            self.suspend(now, t, bound, allow, iv)?;
        }
        Ok(Step {
            transition,
            report: self.take_report(),
        })
    }
    pub fn tick(&mut self, now: u64, iv: impl FnMut() -> [u8; 16]) -> Result<Step, Error> {
        self.time(now)?;
        self.expiry(now)?;
        if self
            .controller
            .tick(now, CoverageEvidence { valid_until: None })?
            == ControllerEvent::RecoveryRequired
        {
            return Err(Error::RecoveryRequired);
        }
        let mut transition = None;
        match self.state() {
            ControllerState::DeferredExcursion {
                request, retry_at, ..
            } if now >= retry_at => {
                let bound = self.return_bound(now, request)?;
                let allow = request.interruption == InterruptionPolicy::AllowSessionLoss;
                let pause = self.assessment(now, self.config.controller.home, bound, allow)?;
                transition = self.controller.continue_excursion(
                    now,
                    CoverageEvidence { valid_until: None },
                    pause,
                )?;
                if let Some(t) = transition {
                    self.suspend(now, t, bound, allow, iv)?;
                }
            }
            ControllerState::ReturnRequired {
                target,
                interruption,
                ..
            }
            | ControllerState::DeferredReturn {
                target,
                interruption,
                ..
            } => {
                let allow = interruption == InterruptionPolicy::AllowSessionLoss;
                // Alternate state has no promised next visit. Account for all
                // obligations that cannot survive indefinite local absence.
                let pause = self.assessment(now, target, u64::MAX, allow)?;
                transition = self.controller.begin_return(now, pause)?;
                if let Some(t) = transition {
                    self.suspend(now, t, u64::MAX, allow, iv)?;
                }
            }
            _ => {}
        }
        Ok(Step {
            transition,
            report: self.take_report(),
        })
    }
    pub fn cancel(&mut self, now: u64, iv: impl FnMut() -> [u8; 16]) -> Result<Step, Error> {
        self.time(now)?;
        self.controller.cancel(now)?;
        self.tick(now, iv)
    }
    pub fn acknowledge(
        &mut self,
        now: u64,
        id: u64,
        ack: Acknowledgement,
    ) -> Result<Report, Error> {
        self.time(now)?;
        let to = match self.state() {
            ControllerState::Transitioning { transition, .. } if transition.id == id => {
                transition.to
            }
            _ => {
                return Err(Error::Controller(ControllerError::WrongAcknowledgement {
                    expected: match self.state() {
                        ControllerState::Transitioning { transition, .. } => transition.id,
                        _ => 0,
                    },
                    received: id,
                }));
            }
        };
        self.controller.acknowledge(now, id, ack)?;
        if self.state() == ControllerState::RecoveryRequired {
            return Err(Error::RecoveryRequired);
        }
        match to {
            RETINUE => {
                let r = self.retinue.resume(now)?;
                self.retinue_expired(r);
            }
            SENNET => {
                if let Some(e) = self.sennet.resume(now)? {
                    self.event(Event::Sennet(e));
                }
            }
            TUCKET => {
                let r = self.tucket.resume(now)?;
                self.tucket_lost(now, r)?;
            }
            _ => return Err(Error::WrongInstance),
        }
        self.work.activate(now, to)?;
        Ok(self.take_report())
    }
}
