//! Failures reported by the personality serial runtime.

use crate::direct_phy_serial::ReconfigureError;
use crate::personality::ControllerError;
use crate::serial::TransmitError;

/// Construction, hardware, or deadline failures in [`PersonalitySerialRuntime`](super::PersonalitySerialRuntime).
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
