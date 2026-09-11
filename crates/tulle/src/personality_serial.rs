//! Host-driven, exclusive direct-PHY runtime for [`crate::personality::Controller`].
//!
//! This keeps one serial pump open and owns its monotonic epoch. Protocol callers
//! retain all protocol/session state and report side-effect-free readiness to the
//! controller. The runtime acknowledges a transition only after the firmware's
//! profile acknowledgement. It does not implement autonomous board switching.
//!
//! A direct-PHY profile request already taken by the pump cannot be cancelled.
//! An outer deadline or serial failure therefore makes the physical profile
//! uncertain. The runtime retires that session, latches recovery, and rejects
//! every later send, receive, or transition instead of restarting a stale pump.

use core::time::Duration;

use tokio::time::{Instant, timeout};

use crate::PhyProfile;
use crate::direct_phy_serial::{DirectPhySerialLink, ReconfigureError};
use crate::link::Received;
use crate::lora::LoRaParams;
use crate::personality::{
    Acknowledgement, Controller, ControllerError, ControllerEvent, ControllerState,
    CoverageEvidence, Excursion, PauseOutcome, PersonalityId, StopCapability, Transition,
};
use crate::serial::TransmitError;

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

/// Construction, hardware, or deadline failures in [`PersonalitySerialRuntime`].
#[derive(Debug)]
pub enum PersonalitySerialError {
    Controller(ControllerError),
    RecoveryRequired,
    ProfileConfiguration,
    WrongTransition {
        expected: Option<u64>,
        received: u64,
    },
    Reconfigure(ReconfigureError),
    DeadlineExpired,
    ReceiveDeadline,
    ExcursionWindowTooShort,
    Transmit(TransmitError),
}

impl From<ControllerError> for PersonalitySerialError {
    fn from(error: ControllerError) -> Self {
        Self::Controller(error)
    }
}

impl core::fmt::Display for PersonalitySerialError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Controller(error) => write!(f, "personality controller error: {error:?}"),
            Self::RecoveryRequired => f.write_str("personality serial runtime requires recovery"),
            Self::ProfileConfiguration => {
                f.write_str("profiles must map every installed personality exactly once")
            }
            Self::WrongTransition { expected, received } => write!(
                f,
                "transition {received} is not current serial transition {expected:?}"
            ),
            Self::Reconfigure(error) => write!(f, "direct-PHY reconfiguration failed: {error}"),
            Self::DeadlineExpired => {
                f.write_str("controller deadline expired before direct-PHY completion")
            }
            Self::ReceiveDeadline => {
                f.write_str("receive reached the active excursion return deadline")
            }
            Self::ExcursionWindowTooShort => {
                f.write_str("frame airtime exceeds the remaining excursion window")
            }
            Self::Transmit(error) => write!(f, "direct-PHY transmit failed: {error}"),
        }
    }
}
impl core::error::Error for PersonalitySerialError {}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::airtime::AirtimeBudget;
    use crate::direct_phy::EVENT_CONFIG;
    use crate::direct_phy_serial::DirectPhySerialConfig;
    use crate::personality::{
        CoveragePolicy, Excursion, InstalledPersonalitySet, InterruptionPolicy,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const HOME: PersonalityId = PersonalityId(1);
    const OTHER: PersonalityId = PersonalityId(2);

    fn profile(sf: u8) -> PhyProfile {
        let mut profile = PhyProfile::meshtastic_long_fast(906_875_000);
        profile.spreading_factor = sf;
        profile
    }

    #[tokio::test]
    async fn acknowledges_only_the_firmware_accepted_profile() {
        let home = profile(11);
        let other = profile(9);
        let (host, mut firmware) = tokio::io::duplex(2048);
        let link = DirectPhySerialLink::spawn_test_io(
            host,
            home,
            AirtimeBudget::new(60_000, 60_000),
            DirectPhySerialConfig {
                open_settle: Duration::ZERO,
                ..Default::default()
            },
        );
        let firmware_task = tokio::spawn(async move {
            let mut status = [0_u8; 7];
            firmware.read_exact(&mut status).await.unwrap();
            assert_eq!(&status, b"status\n");
            firmware
                .write_all(b"tulle/test phy online\r\n")
                .await
                .unwrap();

            let mut command = [0_u8; selvage::CONFIG_COMMAND_LEN];
            firmware.read_exact(&mut command).await.unwrap();
            assert_eq!(selvage::decode_config_command(&command), Ok(home));
            firmware.write_all(&[EVENT_CONFIG, 0]).await.unwrap();

            firmware.read_exact(&mut command).await.unwrap();
            assert_eq!(selvage::decode_config_command(&command), Ok(other));
            firmware.write_all(&[EVENT_CONFIG, 0]).await.unwrap();
        });

        let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
        let controller = Controller::new(
            crate::personality::ControllerConfig {
                home: HOME,
                pin: None,
                installed,
                coverage: CoveragePolicy::AllowGap,
                max_excursion_ms: 1_000,
                return_budget_ms: 100,
                max_defer_ms: 10,
                transition_timeout_ms: 500,
            },
            0,
        )
        .unwrap();
        let mut runtime = PersonalitySerialRuntime::new(
            controller,
            link,
            0,
            &[
                PersonalityProfile {
                    personality: HOME,
                    profile: home,
                },
                PersonalityProfile {
                    personality: OTHER,
                    profile: other,
                },
            ],
        )
        .unwrap();
        let transition = runtime
            .request_excursion(
                Excursion {
                    target: OTHER,
                    duration_ms: 100,
                    interruption: InterruptionPolicy::ResumableOnly,
                },
                CoverageEvidence { valid_until: None },
                PauseOutcome::Ready,
                StopCapability::Resumable,
            )
            .unwrap()
            .unwrap();
        let applied = runtime.apply_transition(transition).await.unwrap();
        assert_eq!(applied.discarded_rx, 0);
        assert!(matches!(
            runtime.controller().state(),
            ControllerState::Away { target: OTHER, .. }
        ));
        assert!(matches!(
            runtime.release_at_home(),
            Err(PersonalitySerialError::RecoveryRequired)
        ));
        firmware_task.await.unwrap();
    }

    #[tokio::test]
    async fn rejects_duplicate_and_uninstalled_profile_mappings() {
        let (host, _firmware) = tokio::io::duplex(64);
        let link = DirectPhySerialLink::spawn_test_io(
            host,
            profile(11),
            AirtimeBudget::new(60_000, 60_000),
            DirectPhySerialConfig::default(),
        );
        let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
        let controller = Controller::new(
            crate::personality::ControllerConfig {
                home: HOME,
                pin: None,
                installed,
                coverage: CoveragePolicy::AllowGap,
                max_excursion_ms: 100,
                return_budget_ms: 10,
                max_defer_ms: 10,
                transition_timeout_ms: 10,
            },
            0,
        )
        .unwrap();
        assert!(matches!(
            PersonalitySerialRuntime::new(
                controller,
                link,
                0,
                &[
                    PersonalityProfile {
                        personality: HOME,
                        profile: profile(11)
                    },
                    PersonalityProfile {
                        personality: HOME,
                        profile: profile(9)
                    },
                    PersonalityProfile {
                        personality: OTHER,
                        profile: profile(9)
                    },
                ],
            ),
            Err(PersonalitySerialError::ProfileConfiguration)
        ));
    }

    #[tokio::test]
    async fn cancelled_profile_future_latches_recovery_and_refuses_send() {
        let home = profile(11);
        let other = profile(9);
        let (host, mut firmware) = tokio::io::duplex(2048);
        let link = DirectPhySerialLink::spawn_test_io(
            host,
            home,
            AirtimeBudget::new(60_000, 60_000),
            DirectPhySerialConfig {
                open_settle: Duration::ZERO,
                ..Default::default()
            },
        );
        let firmware_task = tokio::spawn(async move {
            let mut status = [0_u8; 7];
            firmware.read_exact(&mut status).await.unwrap();
            firmware
                .write_all(b"tulle/test phy online\r\n")
                .await
                .unwrap();
            let mut command = [0_u8; selvage::CONFIG_COMMAND_LEN];
            firmware.read_exact(&mut command).await.unwrap();
            firmware.write_all(&[EVENT_CONFIG, 0]).await.unwrap();
            // The second command is intentionally never acknowledged.
            firmware.read_exact(&mut command).await.unwrap();
        });
        let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
        let controller = Controller::new(
            crate::personality::ControllerConfig {
                home: HOME,
                pin: None,
                installed,
                coverage: CoveragePolicy::AllowGap,
                max_excursion_ms: 1_000,
                return_budget_ms: 100,
                max_defer_ms: 10,
                transition_timeout_ms: 500,
            },
            0,
        )
        .unwrap();
        let mut runtime = PersonalitySerialRuntime::new(
            controller,
            link,
            0,
            &[
                PersonalityProfile {
                    personality: HOME,
                    profile: home,
                },
                PersonalityProfile {
                    personality: OTHER,
                    profile: other,
                },
            ],
        )
        .unwrap();
        let transition = runtime
            .request_excursion(
                Excursion {
                    target: OTHER,
                    duration_ms: 100,
                    interruption: InterruptionPolicy::ResumableOnly,
                },
                CoverageEvidence { valid_until: None },
                PauseOutcome::Ready,
                StopCapability::Resumable,
            )
            .unwrap()
            .unwrap();
        {
            let pending = runtime.apply_transition(transition);
            tokio::pin!(pending);
            tokio::select! {
                _ = &mut pending => panic!("firmware withheld the profile acknowledgement"),
                _ = tokio::task::yield_now() => {}
            }
        }
        assert!(runtime.is_recovery_required());
        assert!(matches!(
            runtime.send(HOME, vec![1]).await,
            Err(PersonalitySerialError::RecoveryRequired)
        ));
        assert!(matches!(
            runtime.release_at_home(),
            Err(PersonalitySerialError::RecoveryRequired)
        ));
        firmware_task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn away_receive_stops_at_excursion_deadline_before_return_budget() {
        let home = profile(11);
        let other = profile(9);
        let (host, mut firmware) = tokio::io::duplex(2048);
        let link = DirectPhySerialLink::spawn_test_io(
            host,
            home,
            AirtimeBudget::new(60_000, 60_000),
            DirectPhySerialConfig {
                open_settle: Duration::ZERO,
                ..Default::default()
            },
        );
        let firmware_task = tokio::spawn(async move {
            let mut status = [0_u8; 7];
            firmware.read_exact(&mut status).await.unwrap();
            firmware
                .write_all(b"tulle/test phy online\r\n")
                .await
                .unwrap();
            let mut command = [0_u8; selvage::CONFIG_COMMAND_LEN];
            firmware.read_exact(&mut command).await.unwrap();
            firmware.write_all(&[EVENT_CONFIG, 0]).await.unwrap();
            firmware.read_exact(&mut command).await.unwrap();
            firmware.write_all(&[EVENT_CONFIG, 0]).await.unwrap();
            tokio::time::sleep(Duration::from_secs(1)).await;
        });
        let installed = InstalledPersonalitySet::new(&[HOME, OTHER]).unwrap();
        let controller = Controller::new(
            crate::personality::ControllerConfig {
                home: HOME,
                pin: None,
                installed,
                coverage: CoveragePolicy::AllowGap,
                max_excursion_ms: 100,
                return_budget_ms: 40,
                max_defer_ms: 10,
                transition_timeout_ms: 500,
            },
            0,
        )
        .unwrap();
        let mut runtime = PersonalitySerialRuntime::new(
            controller,
            link,
            0,
            &[
                PersonalityProfile {
                    personality: HOME,
                    profile: home,
                },
                PersonalityProfile {
                    personality: OTHER,
                    profile: other,
                },
            ],
        )
        .unwrap();
        let transition = runtime
            .request_excursion(
                Excursion {
                    target: OTHER,
                    duration_ms: 100,
                    interruption: InterruptionPolicy::ResumableOnly,
                },
                CoverageEvidence { valid_until: None },
                PauseOutcome::Ready,
                StopCapability::Resumable,
            )
            .unwrap()
            .unwrap();
        runtime.apply_transition(transition).await.unwrap();
        tokio::time::advance(Duration::from_millis(100)).await;
        assert!(matches!(
            runtime.recv().await,
            Err(PersonalitySerialError::ReceiveDeadline)
        ));
        assert!(matches!(
            runtime.tick(CoverageEvidence { valid_until: None }),
            Ok(ControllerEvent::ReturnRequired(_))
        ));
        assert!(
            matches!(runtime.controller().state(), ControllerState::ReturnRequired { return_by, .. } if return_by >= 140)
        );
        firmware_task.await.unwrap();
    }
}
