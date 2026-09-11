//! Board-owned, one-shot direct-PHY excursions.
//!
//! This scheduler has no protocol adapter state.  It changes only the V4 PHY at a
//! completed command/event boundary and always restores the captured home profile.

use selvage::PhyProfile;
use selvage::personality::{
    Acknowledgement, Controller, ControllerConfig, ControllerError, ControllerEvent,
    ControllerState, CoverageEvidence, CoveragePolicy, Excursion, InstalledPersonalitySet,
    InterruptionPolicy, PauseOutcome, PersonalityId, StopCapability, Transition,
};

use crate::radio_owner::{V4ExcursionError, V4RadioOwner};
use lora_phy::{DelayNs, mod_traits::RadioKind};

const HOME: PersonalityId = PersonalityId(0);
const AWAY: PersonalityId = PersonalityId(1);
pub const MAX_EXCURSION_MS: u64 = 60_000;
const RETURN_BUDGET_MS: u64 = 2_000;
const TRANSITION_TIMEOUT_MS: u64 = 1_500;

pub(crate) struct BoardExcursion {
    controller: Controller,
    home: PhyProfile,
    away: PhyProfile,
    last_deadline: u64,
}

impl BoardExcursion {
    pub(crate) fn new(home: PhyProfile, now: u64) -> Self {
        let installed = InstalledPersonalitySet::new(&[HOME, AWAY]).expect("fixed installed set");
        let controller = Controller::new(
            ControllerConfig {
                home: HOME,
                pin: None,
                installed,
                coverage: CoveragePolicy::AllowGap,
                max_excursion_ms: MAX_EXCURSION_MS,
                return_budget_ms: RETURN_BUDGET_MS,
                max_defer_ms: 1,
                transition_timeout_ms: TRANSITION_TIMEOUT_MS,
            },
            now,
        )
        .expect("fixed controller config");
        Self {
            controller,
            home,
            away: home,
            last_deadline: 0,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        !matches!(self.controller.state(), ControllerState::Home)
    }

    pub(crate) fn last_deadline(&self) -> u64 {
        self.last_deadline
    }

    pub(crate) fn next_deadline(&self) -> Option<u64> {
        match self.controller.state() {
            ControllerState::Transitioning { transition, .. } => Some(transition.deadline),
            ControllerState::Away {
                excursion_deadline, ..
            } => Some(excursion_deadline),
            ControllerState::ReturnRequired { return_by, .. }
            | ControllerState::DeferredReturn { return_by, .. } => Some(return_by),
            ControllerState::RecoveryRequired => Some(0),
            _ => None,
        }
    }

    pub(crate) fn request(
        &mut self,
        now: u64,
        home: PhyProfile,
        away: PhyProfile,
        duration_ms: u64,
    ) -> Result<Transition, ControllerError> {
        self.home = home;
        self.away = away;
        self.controller
            .request_excursion(
                now,
                Excursion {
                    target: AWAY,
                    duration_ms,
                    interruption: InterruptionPolicy::ResumableOnly,
                },
                CoverageEvidence { valid_until: None },
                PauseOutcome::Ready,
                StopCapability::Resumable,
            )?
            .ok_or(ControllerError::NotHome)
    }

    pub(crate) async fn apply<RK: RadioKind, DLY: DelayNs>(
        &mut self,
        owner: &mut V4RadioOwner<RK, DLY>,
        transition: Transition,
    ) -> Result<(), V4ExcursionError> {
        let profile = if transition.to == HOME {
            self.home
        } else {
            self.away
        };
        owner.apply_excursion_profile(&profile).await?;
        self.controller
            .acknowledge(
                embassy_time::Instant::now().as_millis(),
                transition.id,
                Acknowledgement::Completed,
            )
            .map_err(V4ExcursionError::Controller)?;
        if let Some(deadline) = self.next_deadline() {
            self.last_deadline = deadline;
        }
        Ok(())
    }

    /// Advance the board clock and begin an owed restore.  Every pause fact is `Ready`
    /// because this direct-PHY board contains no resident protocol adapter.
    pub(crate) fn tick(&mut self, now: u64) -> Result<Option<Transition>, ControllerError> {
        match self
            .controller
            .tick(now, CoverageEvidence { valid_until: None })?
        {
            ControllerEvent::ReturnRequired(_) => {
                self.controller.begin_return(now, PauseOutcome::Ready)
            }
            ControllerEvent::RecoveryRequired => Err(ControllerError::RecoveryRequired),
            ControllerEvent::Idle => Ok(None),
        }
    }
}
