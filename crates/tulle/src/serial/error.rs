//! Transmit and pump errors.

use std::fmt;
use std::io;

/// A frame rejected after it reached the radio pump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TransmitError {
    TooLong {
        max: usize,
    },
    Unsupported,
    DutyCycleImpossible,
    AnnouncementDisabled,
    Transport(String),
    /// The radio is down and a supervised link is reopening it; the frame was dropped.
    Offline,
    Stopped,
}

impl fmt::Display for TransmitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong { max } => write!(f, "frame exceeds max length {max}"),
            Self::Unsupported => write!(f, "radio parameters are unsupported"),
            Self::DutyCycleImpossible => {
                write!(f, "frame cannot fit the configured airtime budget")
            }
            Self::AnnouncementDisabled => {
                write!(
                    f,
                    "announce egress is disabled by this interface's pacing policy"
                )
            }
            Self::Transport(message) => write!(f, "serial transport error: {message}"),
            Self::Offline => write!(f, "radio offline while its port is reopened"),
            Self::Stopped => write!(f, "serial pump stopped"),
        }
    }
}

impl std::error::Error for TransmitError {}

/// Error opening, awaiting, or shutting down a pump.
#[derive(Debug)]
pub enum PumpError {
    Io(io::Error),
    Fault(String),
    Stopped,
    Task(tokio::task::JoinError),
}

impl fmt::Display for PumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "serial I/O failed: {error}"),
            Self::Fault(message) => write!(f, "serial pump failed: {message}"),
            Self::Stopped => write!(f, "serial pump stopped"),
            Self::Task(error) => write!(f, "serial pump task failed: {error}"),
        }
    }
}

impl std::error::Error for PumpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Task(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for PumpError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
