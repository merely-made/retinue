//! The exclusive serial owner that applies controller transitions to the radio.

use core::time::Duration;

use tokio::time::{Instant, timeout};

use super::PersonalitySerialError;
use crate::PhyProfile;
use crate::direct_phy_serial::{DirectPhySerialLink, ReconfigureError};
use crate::link::Received;
use crate::lora::LoRaParams;
use crate::personality::{
    Acknowledgement, Controller, ControllerEvent, ControllerState, CoverageEvidence, Excursion,
    PauseOutcome, PersonalityId, StopCapability, Transition,
};

/// One installed controller personality and its caller-configured direct-PHY profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PersonalityProfile {
    pub personality: PersonalityId,
    pub profile: PhyProfile,
}

/// The receipt for one firmware-acknowledged profile transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransitionApplied {
    /// Frames discarded at profile boundaries instead of assigning them across adapters.
    pub discarded_rx: usize,
}

/// A persistent serial owner with a controller-clock epoch in milliseconds.
pub struct PersonalitySerialRuntime {
    controller: Controller,
    link: DirectPhySerialLink,
    profiles: Vec<PersonalityProfile>,
    epoch: Instant,
    epoch_ms: u64,
    recovery_latched: bool,
}

impl PersonalitySerialRuntime {
    /// Takes exclusive ownership of an already-open link.
    ///
    /// `epoch_ms` must equal the caller-monotonic value used to construct the
    /// controller immediately before this runtime is made. Every configured
    /// installed ID must have exactly one profile, so callers cannot relabel a
    /// transition target at application time.
    pub fn new(
        controller: Controller,
        link: DirectPhySerialLink,
        epoch_ms: u64,
        profiles: &[PersonalityProfile],
    ) -> Result<Self, PersonalitySerialError> {
        for profile in profiles {
            if !controller.config().installed.contains(profile.personality)
                || profiles
                    .iter()
                    .filter(|candidate| candidate.personality == profile.personality)
                    .count()
                    != 1
                || LoRaParams::try_from(profile.profile).is_err()
            {
                return Err(PersonalitySerialError::ProfileConfiguration);
            }
        }
        for raw in u8::MIN..=u8::MAX {
            let id = PersonalityId(raw);
            let count = profiles
                .iter()
                .filter(|profile| profile.personality == id)
                .count();
            if controller.config().installed.contains(id) && count != 1 {
                return Err(PersonalitySerialError::ProfileConfiguration);
            }
        }
        Ok(Self {
            controller,
            link,
            profiles: profiles.to_vec(),
            epoch: Instant::now(),
            epoch_ms,
            recovery_latched: false,
        })
    }

    /// Consume the controller at confirmed home while preserving its serial session.
    /// A caller may then construct a new immutable policy over the same radio.
    /// Uncertain or away state cannot release the link through this path.
    pub fn release_at_home(self) -> Result<DirectPhySerialLink, PersonalitySerialError> {
        if self.is_recovery_required() || self.controller.state() != ControllerState::Home {
            return Err(PersonalitySerialError::RecoveryRequired);
        }
        Ok(self.link)
    }

    pub fn controller(&self) -> &Controller {
        &self.controller
    }
    pub fn is_recovery_required(&self) -> bool {
        self.recovery_latched || self.controller.state() == ControllerState::RecoveryRequired
    }

    /// Starts an explicit controller excursion. Adapter readiness remains a
    /// caller-owned query; a returned transition must be given to
    /// [`Self::apply_transition`].
    pub fn request_excursion(
        &mut self,
        request: Excursion,
        coverage: CoverageEvidence,
        home_pause: PauseOutcome,
        target_stop: StopCapability,
    ) -> Result<Option<Transition>, PersonalitySerialError> {
        self.ensure_usable()?;
        let now = self.now()?;
        Ok(self
            .controller
            .request_excursion(now, request, coverage, home_pause, target_stop)?)
    }

    /// Continues a bounded controller deferral with fresh readiness evidence.
    pub fn continue_excursion(
        &mut self,
        coverage: CoverageEvidence,
        home_pause: PauseOutcome,
    ) -> Result<Option<Transition>, PersonalitySerialError> {
        self.ensure_usable()?;
        let now = self.now()?;
        Ok(self
            .controller
            .continue_excursion(now, coverage, home_pause)?)
    }

    pub fn finish(&mut self) -> Result<ControllerEvent, PersonalitySerialError> {
        self.ensure_usable()?;
        let now = self.now()?;
        Ok(self.controller.finish(now)?)
    }

    pub fn cancel(&mut self) -> Result<ControllerEvent, PersonalitySerialError> {
        self.ensure_usable()?;
        let now = self.now()?;
        Ok(self.controller.cancel(now)?)
    }

    /// Advances controller deadlines using this runtime's monotonic epoch.
    pub fn tick(
        &mut self,
        coverage: CoverageEvidence,
    ) -> Result<ControllerEvent, PersonalitySerialError> {
        self.ensure_usable()?;
        let now = self.now()?;
        Ok(self.controller.tick(now, coverage)?)
    }

    /// Queries the active target adapter externally, then supplies that result
    /// to begin the return transition.
    pub fn begin_return(
        &mut self,
        target_pause: PauseOutcome,
    ) -> Result<Option<Transition>, PersonalitySerialError> {
        self.ensure_usable()?;
        let now = self.now()?;
        Ok(self.controller.begin_return(now, target_pause)?)
    }

    /// Sends only while the supplied personality is the controller's active one.
    /// An away send is bounded by its excursion deadline, reserving return time.
    pub async fn send(
        &mut self,
        personality: PersonalityId,
        frame: impl Into<Vec<u8>>,
    ) -> Result<Duration, PersonalitySerialError> {
        let frame = frame.into();
        let deadline = self.active_deadline(personality)?;
        if let Some(deadline) = deadline {
            let remaining = self.remaining(deadline)?;
            let profile = self.profile_for(personality)?;
            let params = LoRaParams::try_from(profile)
                .map_err(|_| PersonalitySerialError::ProfileConfiguration)?;
            if params.time_on_air(frame.len()) > Duration::from_millis(remaining) {
                return Err(PersonalitySerialError::ExcursionWindowTooShort);
            }
        }
        self.recovery_latched = true;
        let result = match deadline {
            Some(deadline) => match timeout(
                Duration::from_millis(self.remaining(deadline)?),
                self.link.send(frame),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => {
                    self.recovery_latched = true;
                    return Err(PersonalitySerialError::DeadlineExpired);
                }
            },
            None => self.link.send(frame).await,
        };
        if let Some(deadline) = deadline
            && self.now()? > deadline
        {
            return Err(PersonalitySerialError::DeadlineExpired);
        }
        match result {
            Ok(duration) => {
                self.recovery_latched = false;
                Ok(duration)
            }
            Err(error) => Err(PersonalitySerialError::Transmit(error)),
        }
    }

    /// Receives without exposing the serial link. Away receives return at the
    /// excursion deadline so the host can reserve the whole return budget.
    pub async fn recv(&mut self) -> Result<Option<Received>, PersonalitySerialError> {
        let deadline = self.active_deadline_for_receive()?;
        match deadline {
            Some(deadline) => timeout(
                Duration::from_millis(self.remaining(deadline)?),
                self.link.recv(),
            )
            .await
            .map_err(|_| PersonalitySerialError::ReceiveDeadline),
            None => Ok(self.link.recv().await),
        }
    }

    /// Reconfigures the persistent link for the exact current controller action.
    /// It samples this runtime's epoch after the await before acknowledging.
    pub async fn apply_transition(
        &mut self,
        transition: Transition,
    ) -> Result<TransitionApplied, PersonalitySerialError> {
        if self.is_recovery_required() {
            return Err(PersonalitySerialError::RecoveryRequired);
        }
        self.require_current_transition(transition)?;
        let profile = self
            .profiles
            .iter()
            .find(|profile| profile.personality == transition.to)
            .copied()
            .ok_or(PersonalitySerialError::ProfileConfiguration)?;
        // If this future is externally cancelled after it begins polling, the
        // profile request may still be in the pump. Keep recovery latched until
        // a successful post-acknowledgement path explicitly clears it.
        self.recovery_latched = true;
        let mut discarded_rx = self.link.discard_buffered_rx();
        let result = timeout(
            Duration::from_millis(self.remaining(transition.deadline)?),
            self.link.reconfigure(profile.profile),
        )
        .await;
        match result {
            Ok(Ok(())) => {
                let after = self.now()?;
                if after > transition.deadline {
                    self.fail_transition(
                        after,
                        transition.id,
                        Acknowledgement::Unknown,
                        PersonalitySerialError::DeadlineExpired,
                    )
                } else {
                    self.controller.acknowledge(
                        after,
                        transition.id,
                        Acknowledgement::Completed,
                    )?;
                    if self.controller.state() == ControllerState::RecoveryRequired {
                        Err(PersonalitySerialError::RecoveryRequired)
                    } else {
                        discarded_rx += self.link.discard_buffered_rx();
                        self.recovery_latched = false;
                        Ok(TransitionApplied { discarded_rx })
                    }
                }
            }
            Ok(Err(error)) => {
                let acknowledgement = match &error {
                    ReconfigureError::Rejected { .. } | ReconfigureError::InvalidProfile(_) => {
                        Acknowledgement::Failed
                    }
                    ReconfigureError::TimedOut | ReconfigureError::Stopped => {
                        Acknowledgement::Unknown
                    }
                };
                let now = self.now_or_latch();
                self.fail_transition(
                    now,
                    transition.id,
                    acknowledgement,
                    PersonalitySerialError::Reconfigure(error),
                )
            }
            Err(_) => {
                let now = self.now_or_latch();
                self.fail_transition(
                    now,
                    transition.id,
                    Acknowledgement::Unknown,
                    PersonalitySerialError::DeadlineExpired,
                )
            }
        }
    }

    fn active_deadline(
        &mut self,
        personality: PersonalityId,
    ) -> Result<Option<u64>, PersonalitySerialError> {
        if self.is_recovery_required() {
            return Err(PersonalitySerialError::RecoveryRequired);
        }
        match self.controller.state() {
            ControllerState::Home if personality == self.controller.config().home => Ok(None),
            ControllerState::Away {
                target,
                excursion_deadline,
                ..
            } if personality == target => Ok(Some(excursion_deadline)),
            _ => Err(PersonalitySerialError::RecoveryRequired),
        }
    }
    fn ensure_usable(&self) -> Result<(), PersonalitySerialError> {
        if self.is_recovery_required() {
            Err(PersonalitySerialError::RecoveryRequired)
        } else {
            Ok(())
        }
    }
    fn active_deadline_for_receive(&mut self) -> Result<Option<u64>, PersonalitySerialError> {
        if self.is_recovery_required() {
            return Err(PersonalitySerialError::RecoveryRequired);
        }
        match self.controller.state() {
            ControllerState::Home => Ok(None),
            ControllerState::Away {
                excursion_deadline, ..
            } => Ok(Some(excursion_deadline)),
            _ => Err(PersonalitySerialError::RecoveryRequired),
        }
    }
    fn require_current_transition(
        &self,
        received: Transition,
    ) -> Result<(), PersonalitySerialError> {
        match self.controller.state() {
            ControllerState::Transitioning { transition, .. } if transition == received => Ok(()),
            ControllerState::Transitioning { transition, .. } => {
                Err(PersonalitySerialError::WrongTransition {
                    expected: Some(transition.id),
                    received: received.id,
                })
            }
            _ => Err(PersonalitySerialError::WrongTransition {
                expected: None,
                received: received.id,
            }),
        }
    }
    fn profile_for(
        &self,
        personality: PersonalityId,
    ) -> Result<PhyProfile, PersonalitySerialError> {
        self.profiles
            .iter()
            .find(|profile| profile.personality == personality)
            .map(|profile| profile.profile)
            .ok_or(PersonalitySerialError::ProfileConfiguration)
    }
    fn now(&mut self) -> Result<u64, PersonalitySerialError> {
        let elapsed = self.epoch.elapsed().as_millis();
        let elapsed =
            u64::try_from(elapsed).map_err(|_| PersonalitySerialError::DeadlineExpired)?;
        self.epoch_ms
            .checked_add(elapsed)
            .ok_or(PersonalitySerialError::DeadlineExpired)
    }
    fn now_or_latch(&mut self) -> u64 {
        self.now().unwrap_or_else(|_| {
            self.recovery_latched = true;
            u64::MAX
        })
    }
    fn remaining(&mut self, deadline: u64) -> Result<u64, PersonalitySerialError> {
        deadline
            .checked_sub(self.now()?)
            .ok_or(PersonalitySerialError::DeadlineExpired)
    }
    fn fail_transition<T>(
        &mut self,
        now: u64,
        id: u64,
        acknowledgement: Acknowledgement,
        error: PersonalitySerialError,
    ) -> Result<T, PersonalitySerialError> {
        self.recovery_latched = true;
        let _ = self.controller.acknowledge(now, id, acknowledgement);
        Err(error)
    }
}
