//! Configuration, permissions and outcomes exchanged with the board.

use alloc::vec::Vec;

use crate::{
    flood::ManagedFloodConfig,
    node::{Channel, ReceivedText},
    node_info::NodeDirectoryConfig,
    packet_id::PacketIdentity,
};

/// Configuration retained by one installed Sennet instance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SennetInstanceConfig {
    pub channel: Channel,
    pub flood: ManagedFloodConfig,
    pub directory: NodeDirectoryConfig,
    /// Maximum time an unconfirmed outbound text remains an obligation.
    pub pending_ttl: u64,
}

/// A frame released to the board-owned physical output queue.
#[derive(Debug, PartialEq, Eq)]
pub struct OutboundText {
    /// Board-visible physical-work token, unique while this instance lives.
    pub operation_id: u32,
    pub identity: PacketIdentity,
    /// The board must settle or fence this work before this deadline.
    pub expires_at: u64,
    pub frame: Vec<u8>,
}

/// Explicit authority to account for an unqueued text as lost.
///
/// This is an audit boundary, not authentication. An in-flight frame cannot be
/// discarded here because it may already be owned by the board output queue.
#[derive(Debug)]
pub struct LossPermission(pub(super) LossPermissionKind);

#[derive(Debug)]
pub(super) enum LossPermissionKind {
    Operator,
    TrustedPolicy,
}

impl LossPermission {
    pub const fn operator() -> Self {
        Self(LossPermissionKind::Operator)
    }
    pub const fn trusted_policy() -> Self {
        Self(LossPermissionKind::TrustedPolicy)
    }
}

/// A receive result. Text leaf mode does not create relay output.
#[derive(Debug, PartialEq, Eq)]
pub enum ReceiveOutcome {
    IgnoredChannel,
    IgnoredDestination,
    Duplicate { identity: PacketIdentity },
    Text(ReceivedText),
    NotText,
}

/// One lifecycle-relevant protocol event.
#[derive(Debug, PartialEq, Eq)]
pub enum InstanceEvent {
    PendingExpired {
        operation_id: u32,
        identity: PacketIdentity,
    },
    PendingLost {
        operation_id: u32,
        identity: PacketIdentity,
    },
    TxCompleted {
        operation_id: u32,
        identity: PacketIdentity,
    },
}

/// Result of assessing whether the instance can remain absent through `return_by`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PauseOutcome {
    Ready,
    /// Pending work is still an obligation; reassess at its expiry.
    Busy {
        retry_at: u64,
    },
    /// The requested absence spans the pending deadline; account for loss first.
    RequiresLoss {
        pending: usize,
    },
}
