//! Bootloader entry, application rediscovery, and application verification.

use std::time::Duration;

use thiserror::Error;

use crate::package::ExpectedApplication;
use crate::receipt::ApplicationVerification;

const APPLICATION_STARTUP_GRACE: Duration = Duration::from_secs(2);

pub trait DeviceRunner {
    fn enter_bootloader(
        &mut self,
        current_port: &str,
        patience: Duration,
    ) -> Result<String, DeviceFailure>;

    fn rediscover_application(
        &mut self,
        original_port: &str,
        bootloader_port: &str,
        expected: &ExpectedApplication,
        patience: Duration,
    ) -> Result<String, DeviceFailure>;

    fn verify_application(
        &mut self,
        application_port: &str,
        expected: &ExpectedApplication,
    ) -> Result<ApplicationVerification, DeviceFailure>;
}

#[derive(Default)]
pub struct LiveDeviceRunner;

impl DeviceRunner for LiveDeviceRunner {
    fn enter_bootloader(
        &mut self,
        current_port: &str,
        patience: Duration,
    ) -> Result<String, DeviceFailure> {
        crate::enter_bootloader(current_port, patience).map_err(DeviceFailure::from)
    }

    fn rediscover_application(
        &mut self,
        original_port: &str,
        bootloader_port: &str,
        expected: &ExpectedApplication,
        patience: Duration,
    ) -> Result<String, DeviceFailure> {
        let deadline = std::time::Instant::now() + patience;

        // A port can enumerate before its application answers. Probing then asserts DTR against
        // a half-started T114 and can strand its first CDC session, so wait one bounded window.
        std::thread::sleep(APPLICATION_STARTUP_GRACE.min(patience));
        while std::time::Instant::now() < deadline {
            let Ok(ports) = crate::ports() else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            };

            if let Some(application_port) = select_application_port(
                &ports,
                original_port,
                bootloader_port,
                expected,
                identifies_expected_family,
            )? {
                return Ok(application_port);
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(DeviceFailure::ApplicationTimeout(patience))
    }

    fn verify_application(
        &mut self,
        application_port: &str,
        expected: &ExpectedApplication,
    ) -> Result<ApplicationVerification, DeviceFailure> {
        let found = crate::identify(application_port);
        let Some(board) = found.board else {
            return Err(DeviceFailure::Silence(application_port.into()));
        };
        let Some(family) = crate::package::BoardFamily::from_board(&board) else {
            return Err(DeviceFailure::UnexpectedApplication {
                detail: format!("unknown board banner: {}", found.banner.trim()),
            });
        };
        if family != expected.board {
            return Err(DeviceFailure::UnexpectedApplication {
                detail: format!(
                    "expected {expected_board}, found {family}",
                    expected_board = expected.board
                ),
            });
        }
        let version = crate::field(&found.banner, "version=").ok_or_else(|| {
            DeviceFailure::UnexpectedApplication {
                detail: "application did not report version=".into(),
            }
        })?;
        if version != expected.version {
            return Err(DeviceFailure::UnexpectedApplication {
                detail: format!("expected version {}, found {version}", expected.version),
            });
        }
        Ok(ApplicationVerification {
            board: family,
            version,
            region: found.region,
            channel: found.channel,
        })
    }
}

fn identifies_expected_family(port: &str, expected: &ExpectedApplication) -> bool {
    matches_expected_family(&crate::identify(port), expected)
}

pub(super) fn select_application_port(
    ports: &[String],
    original_port: &str,
    bootloader_port: &str,
    expected: &ExpectedApplication,
    mut identifies: impl FnMut(&str, &ExpectedApplication) -> bool,
) -> Result<Option<String>, DeviceFailure> {
    // Every path remains only a location candidate. Re-identify it before accepting it,
    // because a cable reset or reconnect may have put another device there.
    if ports.iter().any(|port| port == original_port) && identifies(original_port, expected) {
        return Ok(Some(original_port.to_string()));
    }

    // A T114 can leave its bootloader and return as the application on the same COM number.
    if bootloader_port != original_port
        && ports.iter().any(|port| port == bootloader_port)
        && identifies(bootloader_port, expected)
    {
        return Ok(Some(bootloader_port.to_string()));
    }

    // Some boards return on an entirely new application port. Accept exactly one responsive
    // board of the family the immutable package expects. A COM number is never carried over as
    // identity, and an unrelated Retinue board must not turn a transfer into false success.
    let responsive: Vec<_> = ports
        .iter()
        .filter(|port| port.as_str() != bootloader_port && port.as_str() != original_port)
        .filter(|port| identifies(port, expected))
        .cloned()
        .collect();
    match responsive.as_slice() {
        [application_port] => Ok(Some(application_port.clone())),
        [] => Ok(None),
        ports => Err(DeviceFailure::UnexpectedPort {
            expected: original_port.into(),
            found: ports.join(", "),
        }),
    }
}

pub(super) fn matches_expected_family(
    found: &crate::Found,
    expected: &ExpectedApplication,
) -> bool {
    found
        .board
        .as_ref()
        .and_then(crate::package::BoardFamily::from_board)
        .as_ref()
        == Some(&expected.board)
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum DeviceFailure {
    #[error("bootloader did not appear within {0:?}")]
    Timeout(Duration),
    #[error("application did not answer within {0:?}")]
    ApplicationTimeout(Duration),
    #[error("device disappeared from {0}")]
    Disappeared(String),
    #[error("unexpected new port: expected {expected}, found {found}")]
    UnexpectedPort { expected: String, found: String },
    #[error("application on {0} was silent")]
    Silence(String),
    #[error("unexpected application: {detail}")]
    UnexpectedApplication { detail: String },
    #[error("{0}")]
    Other(String),
}

impl From<crate::Error> for DeviceFailure {
    fn from(error: crate::Error) -> Self {
        match error {
            crate::Error::NoBootloader(patience) => Self::Timeout(patience),
            crate::Error::Io(error) => Self::Other(error.to_string()),
            other => Self::Other(other.to_string()),
        }
    }
}
