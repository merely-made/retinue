//! Errors from the direct-PHY control lanes.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReconfigureError {
    InvalidProfile(String),
    Rejected { result: u8 },
    TimedOut,
    Stopped,
}

impl core::fmt::Display for ReconfigureError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidProfile(message) => write!(f, "invalid direct-PHY profile: {message}"),
            Self::Rejected { result } => {
                write!(
                    f,
                    "firmware rejected direct-PHY profile with result {result}"
                )
            }
            Self::TimedOut => f.write_str("direct-PHY profile acknowledgement timed out"),
            Self::Stopped => f.write_str("direct-PHY link stopped"),
        }
    }
}

impl core::error::Error for ReconfigureError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UiSnapshotError {
    TooLong { actual: usize },
    Rejected { result: u8 },
    TimedOut,
    Stopped,
}

impl core::fmt::Display for UiSnapshotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::TooLong { actual } => {
                write!(f, "UI snapshot exceeds the firmware limit: {actual} bytes")
            }
            Self::Rejected { result } => {
                write!(f, "firmware rejected UI snapshot with result {result}")
            }
            Self::TimedOut => f.write_str("UI snapshot acknowledgement timed out"),
            Self::Stopped => f.write_str("direct-PHY link stopped"),
        }
    }
}

impl core::error::Error for UiSnapshotError {}
