//! The retained instance and its single pending-TX obligation.

use alloc::vec::Vec;

use super::lease::ReservationProofKind;
use super::types::LossPermissionKind;
use super::{
    InstanceError, InstanceEvent, LossPermission, OutboundText, PacketIdLease, PacketIdReservation,
    PauseOutcome, ReceiveOutcome, ReservationProof, SennetInstanceConfig,
};
use crate::{
    flood::{FloodIgnore, ManagedFlood},
    node::Channel,
    node_info::NodeDirectory,
    packet_id::{PacketIdState, PacketIdentity},
    transport::Header,
};

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
        let packet = crate::transport::Packet::decode(frame).map_err(InstanceError::Transport)?;
        if packet.header.channel_hash != self.channel.hash {
            return Ok(ReceiveOutcome::IgnoredChannel);
        }
        if packet.header.destination != crate::transport::BROADCAST_DESTINATION
            && packet.header.destination != self.packet_ids.source()
        {
            return Ok(ReceiveOutcome::IgnoredDestination);
        }
        // Decode before remembering: malformed application input must not make
        // a later valid packet with the same public identity disappear. Channel
        // AES-CTR remains unauthenticated, regardless of successful parsing.
        let text = self.channel.open_text(frame).map_err(InstanceError::Node)?;
        if self.flood.observe(packet.header) == Err(FloodIgnore::Duplicate) {
            return Ok(ReceiveOutcome::Duplicate {
                identity: PacketIdentity {
                    source: packet.header.source,
                    packet_id: packet.header.packet_id,
                },
            });
        }
        Ok(match text {
            Some(text) => ReceiveOutcome::Text(text),
            None => ReceiveOutcome::NotText,
        })
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
