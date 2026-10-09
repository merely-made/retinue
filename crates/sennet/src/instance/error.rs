//! Instance refusals.

use super::{LeaseError, PauseOutcome};
use crate::{
    flood::FloodConfigError,
    node::NodeError,
    node_info::{DirectoryConfigError, NodeInfoError},
    packet_id::{PacketIdError, PacketIdentity},
};

#[derive(Debug, PartialEq, Eq)]
pub enum InstanceError {
    Lease(LeaseError),
    FloodConfig(FloodConfigError),
    DirectoryConfig(DirectoryConfigError),
    Node(NodeError),
    NodeInfo(NodeInfoError),
    PacketId(PacketIdError),
    Transport(crate::transport::TransportError),
    ZeroPendingTtl,
    FloodChannelMismatch {
        flood: u8,
        channel: u8,
    },
    Paused,
    NotPaused,
    TimeRegressed {
        previous: u64,
        now: u64,
    },
    ReturnBeforeNow {
        now: u64,
        return_by: u64,
    },
    PauseNotReady(PauseOutcome),
    PendingFull,
    NoPending,
    PendingExpired {
        operation_id: u32,
        identity: PacketIdentity,
    },
    InFlightRequiresSettlement {
        operation_id: u32,
        identity: PacketIdentity,
    },
    CompletionBeforeDispatch {
        operation_id: u32,
        identity: PacketIdentity,
    },
    LeaseExhausted {
        end: u32,
    },
    LeaseSourceMismatch {
        expected: u32,
        actual: u32,
    },
    LeaseNotContiguous {
        expected_start: u32,
        actual_start: u32,
    },
    DeadlineOverflow {
        now: u64,
        ttl: u64,
    },
    OperationIdExhausted,
    UnexpectedCompletion {
        expected: PacketIdentity,
        actual: PacketIdentity,
    },
    UnexpectedOperation {
        expected: u32,
        actual: u32,
    },
}
impl core::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Lease(error) => write!(f, "packet-ID lease: {error}"),
            Self::FloodConfig(error) => write!(f, "flood configuration: {error}"),
            Self::DirectoryConfig(error) => write!(f, "directory configuration: {error}"),
            Self::Node(error) => write!(f, "node: {error}"),
            Self::NodeInfo(error) => write!(f, "node info: {error}"),
            Self::PacketId(error) => write!(f, "packet ID: {error}"),
            Self::Transport(error) => write!(f, "transport: {error}"),
            Self::ZeroPendingTtl => write!(f, "pending TX TTL must be non-zero"),
            Self::FloodChannelMismatch { flood, channel } => {
                write!(f, "flood channel {flood} differs from channel {channel}")
            }
            Self::Paused => write!(f, "Sennet instance is paused"),
            Self::NotPaused => write!(f, "Sennet instance is not paused"),
            Self::TimeRegressed { previous, now } => {
                write!(f, "time regressed from {previous} to {now}")
            }
            Self::ReturnBeforeNow { now, return_by } => {
                write!(f, "return bound {return_by} is before now {now}")
            }
            Self::PauseNotReady(outcome) => write!(f, "pause is not ready: {outcome:?}"),
            Self::PendingFull => write!(f, "one pending text TX is already retained"),
            Self::NoPending => write!(f, "no pending text TX"),
            Self::PendingExpired {
                operation_id,
                identity,
            } => write!(
                f,
                "pending operation {operation_id} for {identity:?} expired; call advance to account for it"
            ),
            Self::InFlightRequiresSettlement {
                operation_id,
                identity,
            } => write!(
                f,
                "in-flight operation {operation_id} for {identity:?} must be settled or fenced by the board queue"
            ),
            Self::CompletionBeforeDispatch {
                operation_id,
                identity,
            } => write!(
                f,
                "completion for queued operation {operation_id} / {identity:?} arrived before dispatch"
            ),
            Self::LeaseExhausted { end } => write!(f, "packet-ID lease is exhausted at {end}"),
            Self::LeaseSourceMismatch { expected, actual } => {
                write!(f, "lease source {actual} differs from {expected}")
            }
            Self::LeaseNotContiguous {
                expected_start,
                actual_start,
            } => write!(
                f,
                "lease starts at {actual_start}, expected {expected_start}"
            ),
            Self::DeadlineOverflow { now, ttl } => {
                write!(f, "pending deadline overflows: {now} + {ttl}")
            }
            Self::OperationIdExhausted => write!(f, "physical operation IDs are exhausted"),
            Self::UnexpectedCompletion { expected, actual } => {
                write!(f, "completion {actual:?} does not match {expected:?}")
            }
            Self::UnexpectedOperation { expected, actual } => {
                write!(f, "operation {actual} does not match {expected}")
            }
        }
    }
}
impl core::error::Error for InstanceError {}

impl From<LeaseError> for InstanceError {
    fn from(value: LeaseError) -> Self {
        Self::Lease(value)
    }
}
