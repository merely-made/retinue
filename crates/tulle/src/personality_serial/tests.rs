use core::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;
use crate::PhyProfile;
use crate::airtime::AirtimeBudget;
use crate::direct_phy::EVENT_CONFIG;
use crate::direct_phy_serial::{DirectPhySerialConfig, DirectPhySerialLink};
use crate::personality::{
    Controller, ControllerEvent, ControllerState, CoverageEvidence, CoveragePolicy, Excursion,
    InstalledPersonalitySet, InterruptionPolicy, PauseOutcome, PersonalityId, StopCapability,
};

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
