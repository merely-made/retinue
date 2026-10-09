//! Power-cut-safe ordering of pending and ordinary-control A/B writes.

use crate::store::{self, Slot};

use super::super::{
    BoardRecoveryFacts, DurableError, DurableLoadError, DurableState, FirstWriteError,
    FirstWriteLoadError, MAX_DURABLE_BODY, NodeId, load, load_first_write_state,
    next_first_write_record, next_record,
};
use super::{FirstWriteStatus, PairEvidence};

/// Separate pending and ordinary-control A/B storage.  Board adapters decide
/// partitions and flash alignment; this portable contract decides their safe
/// ordering.
pub trait FirstWriteStore {
    type Error;
    fn read_control(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error>;
    fn erase_control(&mut self, slot: Slot) -> Result<(), Self::Error>;
    fn program_control(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error>;
    fn read_pending(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error>;
    fn erase_pending(&mut self, slot: Slot) -> Result<(), Self::Error>;
    fn program_pending(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error>;
}

/// Fixed caller-owned scratch.  It keeps all first-write operations usable by
/// a core-only board image without an allocator.
pub struct FirstWriteScratch<'a> {
    control_a: &'a mut [u8],
    control_b: &'a mut [u8],
    pending_a: &'a mut [u8],
    pending_b: &'a mut [u8],
    record_body: &'a mut [u8; MAX_DURABLE_BODY],
    record_page: &'a mut [u8],
    readback: &'a mut [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWriteScratchError {
    UnequalSlotLengths,
}

impl<'a> FirstWriteScratch<'a> {
    /// Makes scratch only when every store read buffer has the same exact slot
    /// length. The record page may be larger, but never smaller, so a board
    /// adapter can safely copy any of its A/B slots into these buffers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        control_a: &'a mut [u8],
        control_b: &'a mut [u8],
        pending_a: &'a mut [u8],
        pending_b: &'a mut [u8],
        record_body: &'a mut [u8; MAX_DURABLE_BODY],
        record_page: &'a mut [u8],
        readback: &'a mut [u8],
    ) -> Result<Self, FirstWriteScratchError> {
        let slot_len = control_a.len();
        if control_b.len() != slot_len
            || pending_a.len() != slot_len
            || pending_b.len() != slot_len
            || readback.len() != slot_len
            || record_page.len() < slot_len
        {
            return Err(FirstWriteScratchError::UnequalSlotLengths);
        }
        Ok(Self {
            control_a,
            control_b,
            pending_a,
            pending_b,
            record_body,
            record_page,
            readback,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWriteIo {
    ReadControl(Slot),
    ReadPending(Slot),
    EraseControl(Slot),
    ProgramControl(Slot),
    ErasePending(Slot),
    ProgramPending(Slot),
    VerifyControl(Slot),
    VerifyPending(Slot),
}

#[derive(Debug, PartialEq, Eq)]
pub enum FirstWriteStorageError<E> {
    Store { operation: FirstWriteIo, error: E },
    Ineligible(FirstWriteStatus),
    Preparation(FirstWritePreparationError),
    ReadbackMismatch { pending: bool, slot: Slot },
    InvalidPending(FirstWriteError),
}

/// A local encoding failure before any erase/program operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWritePreparationError {
    Durable(DurableError),
    RecordBuffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOutcome {
    Staged,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ResumeOutcome<E> {
    AlreadyControlPresent,
    Committed,
    CommittedWithCleanupFailure(FirstWriteStorageError<E>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbandonOutcome {
    NothingStaged,
    Abandoned,
}

/// Reads and classifies both storage pairs without changing boot arbitration.
pub fn inspect_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<FirstWriteStatus, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    Ok(first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    ))
}

/// Classifies raw A/B bytes with full blank/valid/corrupt evidence.
pub fn first_write_status(
    control_a: &[u8],
    control_b: &[u8],
    pending_a: &[u8],
    pending_b: &[u8],
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> FirstWriteStatus {
    let control = match load(control_a, control_b) {
        Ok(_) => PairEvidence::Valid,
        Err(DurableLoadError::Blank) => PairEvidence::Blank,
        Err(_) => PairEvidence::Corrupt,
    };
    let pending = match load_first_write_state(pending_a, pending_b, expected_node, facts) {
        Ok(_) => PairEvidence::Valid,
        Err(FirstWriteLoadError::Blank) => PairEvidence::Blank,
        Err(_) => PairEvidence::Corrupt,
    };
    FirstWriteStatus { control, pending }
}

/// Stages only the pending pair, and only on an entirely blank board.
pub fn stage_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    state: &DurableState,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<StageOutcome, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    let status = first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    );
    if !status.claim_eligible() {
        return Err(FirstWriteStorageError::Ineligible(status));
    }
    let write = next_first_write_record(
        scratch.pending_a,
        scratch.pending_b,
        state,
        expected_node,
        facts,
        scratch.record_body,
        scratch.record_page,
    )
    .map_err(FirstWriteStorageError::InvalidPending)?;
    store
        .erase_pending(write.slot)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ErasePending(write.slot),
            error,
        })?;
    store
        .program_pending(write.slot, &scratch.record_page[..write.len])
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ProgramPending(write.slot),
            error,
        })?;
    store
        .read_pending(write.slot, scratch.readback)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::VerifyPending(write.slot),
            error,
        })?;
    let (a, b) = select_written(
        write.slot,
        scratch.readback,
        scratch.pending_a,
        scratch.pending_b,
    );
    match load_first_write_state(a, b, expected_node, facts) {
        Ok(readback) if readback == *state => Ok(StageOutcome::Staged),
        Ok(_) | Err(_) => Err(FirstWriteStorageError::ReadbackMismatch {
            pending: true,
            slot: write.slot,
        }),
    }
}

/// Commits an exact valid pending state to ordinary control before any pending
/// cleanup.  A cleanup fault is returned as a durable-commit outcome, not as a
/// claim that the control write failed.
pub fn resume_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<ResumeOutcome<S::Error>, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    let status = first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    );
    if matches!(status.control, PairEvidence::Valid) {
        return Ok(ResumeOutcome::AlreadyControlPresent);
    }
    if !status.resume_eligible() {
        return Err(FirstWriteStorageError::Ineligible(status));
    }
    let pending =
        load_first_write_state(scratch.pending_a, scratch.pending_b, expected_node, facts)
            .map_err(|error| match error {
                FirstWriteLoadError::Corrupt(reason) => {
                    FirstWriteStorageError::InvalidPending(reason)
                }
                FirstWriteLoadError::Blank => FirstWriteStorageError::Ineligible(status),
            })?;
    let write = if matches!(status.control, PairEvidence::Corrupt) {
        let body_len =
            super::super::encode_durable(&pending, scratch.record_body).map_err(|error| {
                FirstWriteStorageError::Preparation(FirstWritePreparationError::Durable(error))
            })?;
        // The outer record may be CRC-valid even though its durable body is
        // malformed. Preserve its sequence ordering so this repair wins A/B
        // selection. At MAX, overwrite the selected malformed slot at MAX:
        // equal-sequence selection deterministically keeps that same slot.
        let selection = store::select(scratch.control_a, scratch.control_b);
        let (slot, sequence) = match selection.active {
            Some((slot, record)) if record.sequence == u32::MAX => (slot, u32::MAX),
            _ => (selection.next, selection.next_sequence),
        };
        let len = store::encode(
            sequence,
            &scratch.record_body[..body_len],
            scratch.record_page,
        )
        .map_err(|_| {
            FirstWriteStorageError::Preparation(FirstWritePreparationError::RecordBuffer)
        })?;
        super::super::JournalWrite {
            slot,
            sequence,
            len,
        }
    } else {
        next_record(
            scratch.control_a,
            scratch.control_b,
            &pending,
            scratch.record_body,
            scratch.record_page,
        )
        .map_err(|error| {
            FirstWriteStorageError::Preparation(FirstWritePreparationError::Durable(error))
        })?
    };
    store
        .erase_control(write.slot)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::EraseControl(write.slot),
            error,
        })?;
    store
        .program_control(write.slot, &scratch.record_page[..write.len])
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ProgramControl(write.slot),
            error,
        })?;
    store
        .read_control(write.slot, scratch.readback)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::VerifyControl(write.slot),
            error,
        })?;
    let (a, b) = select_written(
        write.slot,
        scratch.readback,
        scratch.control_a,
        scratch.control_b,
    );
    if !matches!(load(a, b), Ok(readback) if readback == pending) {
        return Err(FirstWriteStorageError::ReadbackMismatch {
            pending: false,
            slot: write.slot,
        });
    }
    for slot in [Slot::A, Slot::B] {
        if let Err(error) = erase_pending_and_verify(store, slot, scratch.readback) {
            return Ok(ResumeOutcome::CommittedWithCleanupFailure(error));
        }
    }
    Ok(ResumeOutcome::Committed)
}

/// Erases staged work only while ordinary control is proven blank.  It never
/// removes stale pending data after a valid control record exists.
pub fn abandon_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<AbandonOutcome, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    let status = first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    );
    if status.ordinary_service_eligible() {
        return Ok(AbandonOutcome::NothingStaged);
    }
    if !status.abandon_eligible() {
        return Err(FirstWriteStorageError::Ineligible(status));
    }
    for slot in [Slot::A, Slot::B] {
        erase_pending_and_verify(store, slot, scratch.readback)?;
    }
    Ok(AbandonOutcome::Abandoned)
}

fn read_all<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
) -> Result<(), FirstWriteStorageError<S::Error>> {
    store
        .read_control(Slot::A, scratch.control_a)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadControl(Slot::A),
            error,
        })?;
    store
        .read_control(Slot::B, scratch.control_b)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadControl(Slot::B),
            error,
        })?;
    store
        .read_pending(Slot::A, scratch.pending_a)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadPending(Slot::A),
            error,
        })?;
    store
        .read_pending(Slot::B, scratch.pending_b)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadPending(Slot::B),
            error,
        })?;
    Ok(())
}

fn select_written<'a>(
    slot: Slot,
    written: &'a [u8],
    a: &'a [u8],
    b: &'a [u8],
) -> (&'a [u8], &'a [u8]) {
    match slot {
        Slot::A => (written, b),
        Slot::B => (a, written),
    }
}

fn erase_pending_and_verify<S: FirstWriteStore>(
    store: &mut S,
    slot: Slot,
    readback: &mut [u8],
) -> Result<(), FirstWriteStorageError<S::Error>> {
    store
        .erase_pending(slot)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ErasePending(slot),
            error,
        })?;
    store
        .read_pending(slot, readback)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::VerifyPending(slot),
            error,
        })?;
    if !readback.iter().all(|byte| *byte == 0xff) {
        return Err(FirstWriteStorageError::ReadbackMismatch {
            pending: true,
            slot,
        });
    }
    Ok(())
}
