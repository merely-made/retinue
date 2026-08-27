//! Pure post-flash application verification.

use thiserror::Error;

use crate::package::{BoardFamily, ExpectedApplication};
use crate::receipt::ApplicationVerification;

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum VerificationFailure {
    #[error("expected application board {expected}, found {actual}")]
    Board {
        expected: BoardFamily,
        actual: BoardFamily,
    },
    #[error("expected application version {expected}, found {actual}")]
    Version { expected: String, actual: String },
    #[error("application reported unsupported region {region}")]
    Region { region: String },
    #[error("application reported unsupported channel {channel}")]
    Channel { channel: String },
}

/// Check only facts that the package can state and the application can report.
///
/// A missing region or channel remains visible in the receipt and is not silently turned into
/// a successful claim. A reported region and channel are checked against the package's declared
/// capabilities; the receipt still records only the selected runtime values.
pub fn verify_application(
    expected: &ExpectedApplication,
    observed: &ApplicationVerification,
    supported_regions: &[String],
    channel_capabilities: &[String],
) -> Result<(), VerificationFailure> {
    if observed.board != expected.board {
        return Err(VerificationFailure::Board {
            expected: expected.board.clone(),
            actual: observed.board.clone(),
        });
    }
    if observed.version != expected.version {
        return Err(VerificationFailure::Version {
            expected: expected.version.clone(),
            actual: observed.version.clone(),
        });
    }
    if let Some(region) = &observed.region
        && !supported_regions.is_empty()
        && !supported_regions.iter().any(|value| value == region)
    {
        return Err(VerificationFailure::Region {
            region: region.clone(),
        });
    }
    if let Some(channel) = &observed.channel
        && !channel_capabilities.is_empty()
        && !channel_capabilities.iter().any(|value| value == channel)
    {
        return Err(VerificationFailure::Channel {
            channel: channel.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected() -> ExpectedApplication {
        ExpectedApplication {
            board: BoardFamily::HeltecV4,
            version: "0.0.1".into(),
            manual_check: None,
        }
    }

    fn observed() -> ApplicationVerification {
        ApplicationVerification {
            board: BoardFamily::HeltecV4,
            version: "0.0.1".into(),
            region: Some("US915".into()),
            channel: Some("rnode".into()),
        }
    }

    #[test]
    fn accepts_matching_application_and_region() {
        assert!(
            verify_application(
                &expected(),
                &observed(),
                &["US915".into()],
                &["modem".into(), "rnode".into()]
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_wrong_version_and_region() {
        let mut value = observed();
        value.version = "0.0.2".into();
        assert!(matches!(
            verify_application(
                &expected(),
                &value,
                &["US915".into()],
                &["modem".into(), "rnode".into()],
            ),
            Err(VerificationFailure::Version { .. })
        ));

        let mut value = observed();
        value.region = Some("EU868".into());
        assert!(matches!(
            verify_application(
                &expected(),
                &value,
                &["US915".into()],
                &["modem".into(), "rnode".into()],
            ),
            Err(VerificationFailure::Region { .. })
        ));

        let mut value = observed();
        value.channel = Some("node".into());
        assert!(matches!(
            verify_application(
                &expected(),
                &value,
                &["US915".into()],
                &["modem".into(), "rnode".into()],
            ),
            Err(VerificationFailure::Channel { .. })
        ));
    }
}
