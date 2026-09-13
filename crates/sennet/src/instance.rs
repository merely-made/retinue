//! Retained, bounded Sennet text-leaf state for an embedded protocol instance.
//!
//! This module owns protocol state only. The board owns radio activation,
//! persistence, physical transmit completion, and any activation-tagged action
//! queue. Packet IDs arrive in a caller-reserved exclusive interval because a
//! reset must never reuse an AES-CTR nonce identity.

use alloc::vec::Vec;

use crate::{
    flood::{FloodConfigError, FloodDecision, FloodIgnore, ManagedFlood, ManagedFloodConfig},
    node::{Channel, NodeError, ReceivedText},
    node_info::{DirectoryConfigError, NodeDirectory, NodeDirectoryConfig, NodeInfoError},
    packet_id::{PacketIdError, PacketIdState, PacketIdentity},
    transport::Header,
};

/// A caller-durable, exclusive packet-ID interval `[start, end)` for one source.
#[derive(Debug, PartialEq, Eq)]
pub struct PacketIdLease {
    source: u32,
    start: u32,
    end: u32,
    /// The next ID restored from caller-owned durable state.
    next: u32,
}

impl PacketIdLease {
    pub const fn new(source: u32, start: u32, end: u32, next: u32) -> Result<Self, LeaseError> {
        if start >= end {
            return Err(LeaseError::EmptyOrReversed { start, end });
        }
        if next < start || next > end {
            return Err(LeaseError::NextOutside { start, end, next });
        }
        Ok(Self {
            source,
            start,
            end,
            next,
        })
    }

    pub const fn source(&self) -> u32 {
        self.source
    }
    pub const fn start(&self) -> u32 {
        self.start
    }
    pub const fn end(&self) -> u32 {
        self.end
    }
    pub const fn next(&self) -> u32 {
        self.next
    }
}

/// A newly durable exclusive interval which extends a prior lease.
#[derive(Debug, PartialEq, Eq)]
pub struct PacketIdReservation {
    source: u32,
    start: u32,
    end: u32,
}

impl PacketIdReservation {
    pub const fn new(source: u32, start: u32, end: u32) -> Result<Self, LeaseError> {
        if start >= end {
            return Err(LeaseError::EmptyOrReversed { start, end });
        }
        Ok(Self { source, start, end })
    }

    pub const fn source(&self) -> u32 {
        self.source
    }
    pub const fn start(&self) -> u32 {
        self.start
    }
    pub const fn end(&self) -> u32 {
        self.end
    }
}

/// Caller assertion authorizing a lease extension.
///
/// This is not authentication and performs no persistence. The board storage
/// owner must create it only after an A/B write and readback, or after a
/// separately trusted caller has established equivalent authority.
#[derive(Debug)]
pub struct ReservationProof(ReservationProofKind);

#[derive(Debug)]
enum ReservationProofKind {
    DurableAck,
    TrustedCaller,
}

impl ReservationProof {
    pub const fn durable_ack() -> Self {
        Self(ReservationProofKind::DurableAck)
    }
    pub const fn trusted_caller() -> Self {
        Self(ReservationProofKind::TrustedCaller)
    }
}

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
pub struct LossPermission(LossPermissionKind);

#[derive(Debug)]
enum LossPermissionKind {
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

#[derive(Debug)]
enum PendingTx {
    Queued {
        operation_id: u32,
        identity: PacketIdentity,
        frame: Vec<u8>,
        expires_at: u64,
    },
    InFlight {
        operation_id: u32,
        identity: PacketIdentity,
        expires_at: u64,
    },
}

impl PendingTx {
    const fn identity(&self) -> PacketIdentity {
        match self {
            Self::Queued { identity, .. } | Self::InFlight { identity, .. } => *identity,
        }
    }

    const fn expires_at(&self) -> u64 {
        match self {
            Self::Queued { expires_at, .. } | Self::InFlight { expires_at, .. } => *expires_at,
        }
    }

    const fn operation_id(&self) -> u32 {
        match self {
            Self::Queued { operation_id, .. } | Self::InFlight { operation_id, .. } => {
                *operation_id
            }
        }
    }
}

/// One retained Sennet instance in text-leaf mode.
pub struct SennetInstance {
    channel: Channel,
    packet_ids: PacketIdState,
    lease: PacketIdLease,
    flood: ManagedFlood,
    directory: NodeDirectory,
    pending_ttl: u64,
    pending: Option<PendingTx>,
    paused: bool,
    last_now: Option<u64>,
    next_operation_id: u32,
}

impl SennetInstance {
    pub fn new(config: SennetInstanceConfig, lease: PacketIdLease) -> Result<Self, InstanceError> {
        if config.pending_ttl == 0 {
            return Err(InstanceError::ZeroPendingTtl);
        }
        if config.flood.channel_hash != config.channel.hash {
            return Err(InstanceError::FloodChannelMismatch {
                flood: config.flood.channel_hash,
                channel: config.channel.hash,
            });
        }
        let flood = ManagedFlood::new(config.flood).map_err(InstanceError::FloodConfig)?;
        let directory =
            NodeDirectory::with_config(config.directory).map_err(InstanceError::DirectoryConfig)?;
        Ok(Self {
            channel: config.channel,
            packet_ids: PacketIdState::new(lease.source, lease.next),
            lease,
            flood,
            directory,
            pending_ttl: config.pending_ttl,
            pending: None,
            paused: false,
            last_now: None,
            next_operation_id: 1,
        })
    }

    pub const fn lease(&self) -> &PacketIdLease {
        &self.lease
    }
    pub const fn packet_id_state(&self) -> PacketIdState {
        self.packet_ids
    }
    pub const fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn directory(&self) -> &NodeDirectory {
        &self.directory
    }
    pub fn seen_len(&self) -> usize {
        self.flood.seen_len()
    }
    pub fn pending_identity(&self) -> Option<PacketIdentity> {
        self.pending.as_ref().map(PendingTx::identity)
    }
    pub fn pending_operation_id(&self) -> Option<u32> {
        self.pending.as_ref().map(PendingTx::operation_id)
    }
    pub fn next_deadline(&self) -> Option<u64> {
        self.pending.as_ref().map(PendingTx::expires_at)
    }

    /// Extend only a contiguous reserved interval after caller persistence evidence.
    pub fn extend_lease(
        &mut self,
        extension: PacketIdReservation,
        proof: ReservationProof,
    ) -> Result<(), InstanceError> {
        match proof.0 {
            ReservationProofKind::DurableAck | ReservationProofKind::TrustedCaller => {}
        }
        if extension.source != self.lease.source {
            return Err(InstanceError::LeaseSourceMismatch {
                expected: self.lease.source,
                actual: extension.source,
            });
        }
        if extension.start != self.lease.end {
            return Err(InstanceError::LeaseNotContiguous {
                expected_start: self.lease.end,
                actual_start: extension.start,
            });
        }
        self.lease.end = extension.end;
        Ok(())
    }

    /// Advance elapsed time, expiring an outstanding physical obligation.
    pub fn advance(&mut self, now: u64) -> Result<Option<InstanceEvent>, InstanceError> {
        self.check_now(now)?;
        if let Some(pending) = self.pending.as_ref()
            && pending.expires_at() <= now
        {
            if matches!(pending, PendingTx::InFlight { .. }) {
                return Err(InstanceError::InFlightRequiresSettlement {
                    operation_id: pending.operation_id(),
                    identity: pending.identity(),
                });
            }
            let pending = self.pending.take().expect("checked pending");
            self.record_now(now)?;
            return Ok(Some(InstanceEvent::PendingExpired {
                operation_id: pending.operation_id(),
                identity: pending.identity(),
            }));
        }
        self.record_now(now)?;
        Ok(None)
    }

    /// Assess absence without mutating retained state.
    pub fn assess_pause(&self, now: u64, return_by: u64) -> Result<PauseOutcome, InstanceError> {
        self.check_now(now)?;
        if return_by < now {
            return Err(InstanceError::ReturnBeforeNow { now, return_by });
        }
        let Some(pending) = self.pending.as_ref() else {
            return Ok(PauseOutcome::Ready);
        };
        if pending.expires_at() <= return_by {
            Ok(PauseOutcome::RequiresLoss { pending: 1 })
        } else {
            Ok(PauseOutcome::Busy {
                retry_at: pending.expires_at(),
            })
        }
    }

    /// Enter paused state only after a ready assessment.
    pub fn pause(&mut self, now: u64, return_by: u64) -> Result<(), InstanceError> {
        self.require_active()?;
        let outcome = self.assess_pause(now, return_by)?;
        if outcome != PauseOutcome::Ready {
            return Err(InstanceError::PauseNotReady(outcome));
        }
        self.record_now(now)?;
        self.paused = true;
        Ok(())
    }

    pub fn resume(&mut self, now: u64) -> Result<Option<InstanceEvent>, InstanceError> {
        if !self.paused {
            return Err(InstanceError::NotPaused);
        }
        let expired = self.advance(now)?;
        self.paused = false;
        Ok(expired)
    }

    /// Account explicitly for the sole bounded pending TX obligation.
    pub fn discard_pending(
        &mut self,
        now: u64,
        permission: LossPermission,
    ) -> Result<Option<InstanceEvent>, InstanceError> {
        self.require_active()?;
        self.check_now(now)?;
        match permission.0 {
            LossPermissionKind::Operator | LossPermissionKind::TrustedPolicy => {}
        }
        if matches!(self.pending, Some(PendingTx::InFlight { .. })) {
            let pending = self.pending.as_ref().expect("matched pending");
            return Err(InstanceError::InFlightRequiresSettlement {
                operation_id: pending.operation_id(),
                identity: pending.identity(),
            });
        }
        self.record_now(now)?;
        Ok(self
            .pending
            .take()
            .map(|pending| InstanceEvent::PendingLost {
                operation_id: pending.operation_id(),
                identity: pending.identity(),
            }))
    }

    /// Construct one text frame, retaining it until the board takes and completes it.
    pub fn queue_text(
        &mut self,
        now: u64,
        mut header: Header,
        text: &str,
    ) -> Result<PacketIdentity, InstanceError> {
        self.require_active()?;
        self.check_now(now)?;
        if self.pending.is_some() {
            return Err(InstanceError::PendingFull);
        }
        let expires_at =
            now.checked_add(self.pending_ttl)
                .ok_or(InstanceError::DeadlineOverflow {
                    now,
                    ttl: self.pending_ttl,
                })?;
        let next = self.packet_ids.next_packet_id();
        if next >= self.lease.end {
            return Err(InstanceError::LeaseExhausted {
                end: self.lease.end,
            });
        }
        let candidate = PacketIdentity {
            source: self.lease.source,
            packet_id: next,
        };
        header.source = candidate.source;
        header.packet_id = candidate.packet_id;
        let frame = self
            .channel
            .seal_text(header, text)
            .map_err(InstanceError::Node)?;
        let operation_id = self.allocate_operation_id()?;
        let identity = self.packet_ids.reserve().map_err(InstanceError::PacketId)?;
        self.record_now(now)?;
        self.pending = Some(PendingTx::Queued {
            operation_id,
            identity,
            frame,
            expires_at,
        });
        Ok(identity)
    }

    /// Move the prepared frame to the board-owned output queue. It remains a pending obligation.
    pub fn take_outbound(&mut self, now: u64) -> Result<Option<OutboundText>, InstanceError> {
        self.require_active()?;
        self.check_now(now)?;
        if let Some(pending) = self.pending.as_ref()
            && pending.expires_at() <= now
        {
            return Err(InstanceError::PendingExpired {
                operation_id: pending.operation_id(),
                identity: pending.identity(),
            });
        }
        let Some(pending) = self.pending.take() else {
            return Ok(None);
        };
        match pending {
            PendingTx::Queued {
                operation_id,
                identity,
                frame,
                expires_at,
            } => {
                self.pending = Some(PendingTx::InFlight {
                    operation_id,
                    identity,
                    expires_at,
                });
                self.record_now(now)?;
                Ok(Some(OutboundText {
                    operation_id,
                    identity,
                    expires_at,
                    frame,
                }))
            }
            inflight @ PendingTx::InFlight { .. } => {
                self.pending = Some(inflight);
                Ok(None)
            }
        }
    }

    /// Record a physical completion reported by the board owner.
    pub fn complete_tx(
        &mut self,
        now: u64,
        operation_id: u32,
        identity: PacketIdentity,
    ) -> Result<InstanceEvent, InstanceError> {
        self.settle_tx(now, operation_id, identity, true)
    }

    /// Record that the board-owned physical queue lost a dispatched frame.
    ///
    /// The frame may have sat past its protocol deadline before the queue can
    /// report the failure. That late settlement is still required to release
    /// the retained obligation, so only clock regression is refused here.
    pub fn fail_tx(
        &mut self,
        now: u64,
        operation_id: u32,
        identity: PacketIdentity,
    ) -> Result<InstanceEvent, InstanceError> {
        self.settle_tx(now, operation_id, identity, false)
    }

    fn settle_tx(
        &mut self,
        now: u64,
        operation_id: u32,
        identity: PacketIdentity,
        completed: bool,
    ) -> Result<InstanceEvent, InstanceError> {
        self.require_active()?;
        self.check_now(now)?;
        let Some(pending) = self.pending.as_ref() else {
            return Err(InstanceError::NoPending);
        };
        if pending.identity() != identity {
            return Err(InstanceError::UnexpectedCompletion {
                expected: pending.identity(),
                actual: identity,
            });
        }
        if pending.operation_id() != operation_id {
            return Err(InstanceError::UnexpectedOperation {
                expected: pending.operation_id(),
                actual: operation_id,
            });
        }
        if !matches!(pending, PendingTx::InFlight { .. }) {
            return Err(InstanceError::CompletionBeforeDispatch {
                operation_id,
                identity,
            });
        }
        self.record_now(now)?;
        self.pending = None;
        Ok(if completed {
            InstanceEvent::TxCompleted {
                operation_id,
                identity,
            }
        } else {
            InstanceEvent::PendingLost {
                operation_id,
                identity,
            }
        })
    }

    /// Observe one radio frame. Text leaf mode deduplicates but never relays.
    pub fn receive(&mut self, now: u64, frame: &[u8]) -> Result<ReceiveOutcome, InstanceError> {
        self.require_active()?;
        self.record_now(now)?;
        match self
            .flood
            .consider(frame)
            .map_err(InstanceError::Transport)?
        {
            FloodDecision::Ignore(FloodIgnore::Channel) => Ok(ReceiveOutcome::IgnoredChannel),
            FloodDecision::Ignore(FloodIgnore::Duplicate) => {
                let packet =
                    crate::transport::Packet::decode(frame).map_err(InstanceError::Transport)?;
                Ok(ReceiveOutcome::Duplicate {
                    identity: PacketIdentity {
                        source: packet.header.source,
                        packet_id: packet.header.packet_id,
                    },
                })
            }
            FloodDecision::Ignore(FloodIgnore::HopLimit) | FloodDecision::Relay { .. } => {
                match self.channel.open_text(frame).map_err(InstanceError::Node)? {
                    Some(text) => Ok(ReceiveOutcome::Text(text)),
                    None => Ok(ReceiveOutcome::NotText),
                }
            }
        }
    }

    pub fn ingest_node_info(&mut self, now: u64, bytes: &[u8]) -> Result<bool, InstanceError> {
        self.require_active()?;
        self.record_now(now)?;
        self.directory
            .ingest_from_radio(bytes)
            .map_err(InstanceError::NodeInfo)
    }

    fn require_active(&self) -> Result<(), InstanceError> {
        if self.paused {
            Err(InstanceError::Paused)
        } else {
            Ok(())
        }
    }

    fn check_now(&self, now: u64) -> Result<(), InstanceError> {
        if let Some(previous) = self.last_now
            && now < previous
        {
            return Err(InstanceError::TimeRegressed { previous, now });
        }
        Ok(())
    }

    fn record_now(&mut self, now: u64) -> Result<(), InstanceError> {
        self.check_now(now)?;
        self.last_now = Some(now);
        Ok(())
    }

    fn allocate_operation_id(&mut self) -> Result<u32, InstanceError> {
        let operation_id = self.next_operation_id;
        self.next_operation_id = operation_id
            .checked_add(1)
            .ok_or(InstanceError::OperationIdExhausted)?;
        Ok(operation_id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    EmptyOrReversed { start: u32, end: u32 },
    NextOutside { start: u32, end: u32, next: u32 },
}
impl core::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyOrReversed { start, end } => {
                write!(f, "packet-ID lease is empty or reversed: {start}..{end}")
            }
            Self::NextOutside { start, end, next } => {
                write!(f, "packet-ID lease next {next} is outside {start}..={end}")
            }
        }
    }
}
impl core::error::Error for LeaseError {}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        flood::RelayDelayWindow,
        node_info::NodeDirectoryConfig,
        transport::{BROADCAST_DESTINATION, ChannelKey},
    };

    fn config() -> SennetInstanceConfig {
        SennetInstanceConfig {
            channel: Channel {
                hash: 8,
                key: ChannelKey::Aes128([7; 16]),
            },
            flood: ManagedFloodConfig {
                channel_hash: 8,
                relay_node: 4,
                seen_capacity: 2,
                delay: RelayDelayWindow::new(
                    core::time::Duration::ZERO,
                    core::time::Duration::ZERO,
                )
                .unwrap(),
            },
            directory: NodeDirectoryConfig::default(),
            pending_ttl: 10,
        }
    }
    fn instance() -> SennetInstance {
        SennetInstance::new(
            config(),
            PacketIdLease::new(0x0102_0304, 10, 13, 10).unwrap(),
        )
        .unwrap()
    }
    fn header() -> Header {
        Header {
            destination: BROADCAST_DESTINATION,
            source: 0,
            packet_id: 0,
            hop_limit: 3,
            want_ack: false,
            via_mqtt: false,
            hop_start: 3,
            channel_hash: 0,
            next_hop: 0,
            relay_node: 4,
        }
    }

    #[test]
    fn retained_channel_ids_and_dedup_survive_lifecycle() {
        let mut instance = instance();
        let first = instance.queue_text(1, header(), "one").unwrap();
        assert_eq!(first.packet_id, 10);
        let outbound = instance.take_outbound(1).unwrap().unwrap();
        assert_eq!(
            instance
                .complete_tx(2, outbound.operation_id, outbound.identity)
                .unwrap(),
            InstanceEvent::TxCompleted {
                operation_id: outbound.operation_id,
                identity: first
            }
        );
        let second = instance.queue_text(3, header(), "two").unwrap();
        assert_eq!(second.packet_id, 11);
        let frame = instance.take_outbound(3).unwrap().unwrap().frame;
        assert!(matches!(
            instance.receive(4, &frame).unwrap(),
            ReceiveOutcome::Text(_)
        ));
        assert_eq!(
            instance.receive(5, &frame).unwrap(),
            ReceiveOutcome::Duplicate { identity: second }
        );
        assert_eq!(instance.seen_len(), 1);
    }

    #[test]
    fn denied_pause_does_not_mutate_state() {
        let mut instance = instance();
        let identity = instance.queue_text(10, header(), "wait").unwrap();
        assert_eq!(
            instance.assess_pause(11, 15),
            Ok(PauseOutcome::Busy { retry_at: 20 })
        );
        assert_eq!(
            instance.pause(11, 15),
            Err(InstanceError::PauseNotReady(PauseOutcome::Busy {
                retry_at: 20
            }))
        );
        assert!(!instance.is_paused());
        assert_eq!(instance.pending_identity(), Some(identity));
        assert_eq!(instance.packet_id_state().next_packet_id(), 11);
    }

    #[test]
    fn absence_advances_time_and_requires_explicit_loss() {
        let mut instance = instance();
        let identity = instance.queue_text(1, header(), "loss").unwrap();
        assert_eq!(
            instance.assess_pause(2, 11),
            Ok(PauseOutcome::RequiresLoss { pending: 1 })
        );
        assert_eq!(
            instance
                .discard_pending(2, LossPermission::operator())
                .unwrap(),
            Some(InstanceEvent::PendingLost {
                operation_id: 1,
                identity
            })
        );
        instance.pause(2, 11).unwrap();
        assert!(instance.resume(12).unwrap().is_none());
        assert_eq!(instance.packet_id_state().next_packet_id(), 11);
    }

    #[test]
    fn expiry_time_and_lease_extension_are_checked() {
        let mut instance = instance();
        let identity = instance.queue_text(1, header(), "expiry").unwrap();
        assert_eq!(
            instance.advance(11).unwrap(),
            Some(InstanceEvent::PendingExpired {
                operation_id: 1,
                identity
            })
        );
        assert_eq!(
            instance.advance(10),
            Err(InstanceError::TimeRegressed {
                previous: 11,
                now: 10
            })
        );
        assert_eq!(
            instance.extend_lease(
                PacketIdReservation::new(0x0102_0304, 13, 15).unwrap(),
                ReservationProof::durable_ack()
            ),
            Ok(())
        );
        assert_eq!(
            instance.extend_lease(
                PacketIdReservation::new(0x0102_0304, 14, 16).unwrap(),
                ReservationProof::trusted_caller()
            ),
            Err(InstanceError::LeaseNotContiguous {
                expected_start: 15,
                actual_start: 14
            })
        );
        let overflow = SennetInstance::new(
            SennetInstanceConfig {
                pending_ttl: u64::MAX,
                ..config()
            },
            PacketIdLease::new(1, 1, 2, 1).unwrap(),
        )
        .unwrap();
        let mut overflow = overflow;
        assert_eq!(
            overflow.queue_text(1, header(), "x"),
            Err(InstanceError::DeadlineOverflow {
                now: 1,
                ttl: u64::MAX
            })
        );
    }

    #[test]
    fn paused_operations_are_refused() {
        let mut instance = instance();
        instance.pause(1, 1).unwrap();
        assert_eq!(
            instance.queue_text(1, header(), "no"),
            Err(InstanceError::Paused)
        );
        assert_eq!(instance.take_outbound(1), Err(InstanceError::Paused));
        assert_eq!(instance.receive(1, &[]), Err(InstanceError::Paused));
    }

    #[test]
    fn invalid_composition_does_not_burn_ids_or_operations() {
        let mut instance = instance();
        let mut invalid = header();
        invalid.hop_limit = 8;
        assert!(matches!(
            instance.queue_text(1, invalid, "bad"),
            Err(InstanceError::Node(NodeError::Transport(_)))
        ));
        let too_long = "x".repeat(crate::application::MAX_APPLICATION_PAYLOAD + 1);
        assert!(matches!(
            instance.queue_text(1, header(), &too_long),
            Err(InstanceError::Node(NodeError::Application(_)))
        ));
        assert_eq!(instance.packet_id_state().next_packet_id(), 10);
        assert_eq!(instance.pending_operation_id(), None);
        let identity = instance.queue_text(1, header(), "good").unwrap();
        let outbound = instance.take_outbound(1).unwrap().unwrap();
        assert_eq!(identity.packet_id, 10);
        assert_eq!(outbound.operation_id, 1);
    }

    #[test]
    fn expiry_boundary_stays_queued_until_advance_accounts_for_it() {
        let mut instance = instance();
        let identity = instance.queue_text(1, header(), "deadline").unwrap();
        assert_eq!(instance.next_deadline(), Some(11));
        assert_eq!(
            instance.take_outbound(11),
            Err(InstanceError::PendingExpired {
                operation_id: 1,
                identity
            })
        );
        assert_eq!(
            instance.advance(11).unwrap(),
            Some(InstanceEvent::PendingExpired {
                operation_id: 1,
                identity
            })
        );
        assert_eq!(instance.pending_identity(), None);
    }

    #[test]
    fn queued_completion_is_refused_without_mutation() {
        let mut instance = instance();
        let identity = instance.queue_text(1, header(), "queue").unwrap();
        assert_eq!(
            instance.complete_tx(2, 1, identity),
            Err(InstanceError::CompletionBeforeDispatch {
                operation_id: 1,
                identity
            })
        );
        assert_eq!(instance.pending_identity(), Some(identity));
        assert_eq!(instance.take_outbound(2).unwrap().unwrap().operation_id, 1);
    }

    #[test]
    fn in_flight_loss_is_refused_without_mutation() {
        let mut instance = instance();
        let identity = instance.queue_text(1, header(), "sent").unwrap();
        let outbound = instance.take_outbound(1).unwrap().unwrap();
        assert_eq!(
            instance.discard_pending(2, LossPermission::operator()),
            Err(InstanceError::InFlightRequiresSettlement {
                operation_id: outbound.operation_id,
                identity
            })
        );
        assert_eq!(instance.pending_identity(), Some(identity));
        assert_eq!(instance.pending_operation_id(), Some(outbound.operation_id));
    }

    #[test]
    fn dispatched_failure_settles_late_but_queued_failure_is_refused() {
        let mut instance = instance();
        let identity = instance.queue_text(1, header(), "queue").unwrap();
        assert_eq!(
            instance.fail_tx(2, 1, identity),
            Err(InstanceError::CompletionBeforeDispatch {
                operation_id: 1,
                identity
            })
        );
        let outbound = instance.take_outbound(2).unwrap().unwrap();
        assert_eq!(
            instance.fail_tx(12, outbound.operation_id, outbound.identity),
            Ok(InstanceEvent::PendingLost {
                operation_id: outbound.operation_id,
                identity
            })
        );
        assert_eq!(instance.pending_identity(), None);
        assert_eq!(instance.next_deadline(), None);
    }

    #[test]
    fn absence_keeps_directory_and_advances_the_instance_clock() {
        let mut instance = instance();
        let record = [
            0x22, 0x12, 0x08, 0x01, 0x12, 0x0e, 0x0a, 0x02, b'i', b'd', 0x12, 0x04, b'n', b'a',
            b'm', b'e', 0x1a, 0x02, b'n', b'm',
        ];
        assert!(instance.ingest_node_info(1, &record).unwrap());
        instance.pause(2, 2).unwrap();
        assert_eq!(instance.resume(20).unwrap(), None);
        assert_eq!(instance.directory().len(), 1);
        assert_eq!(
            instance.queue_text(19, header(), "old"),
            Err(InstanceError::TimeRegressed {
                previous: 20,
                now: 19
            })
        );
    }
}
