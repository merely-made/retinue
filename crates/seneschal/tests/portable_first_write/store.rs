//! A fault-injecting in-memory first-write store.

use seneschal::control::{FirstWriteScratch, FirstWriteStore, StageOutcome, stage_first_write};
use seneschal::store::Slot;

use super::{NODE, PAGE, facts, state};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fault {
    None,
    ReadControl,
    ReadPending,
    EraseControl,
    ProgramControl,
    VerifyControl,
    ErasePending,
    ProgramPending,
    VerifyPending,
    TornProgramControl,
    TornProgramPending,
    ErasePendingB,
    VerifyPendingB,
}

#[derive(Clone)]
pub(super) struct Store {
    pub(super) control: [[u8; PAGE]; 2],
    pub(super) pending: [[u8; PAGE]; 2],
    fault: Fault,
    control_reads: u8,
    pending_reads: u8,
}

impl Store {
    pub(super) fn blank() -> Self {
        Self {
            control: [[0xff; PAGE]; 2],
            pending: [[0xff; PAGE]; 2],
            fault: Fault::None,
            control_reads: 0,
            pending_reads: 0,
        }
    }
    fn slot(slot: Slot) -> usize {
        match slot {
            Slot::A => 0,
            Slot::B => 1,
        }
    }
    fn fail(&self, value: Fault) -> bool {
        self.fault == value
    }
    pub(super) fn inject(&mut self, fault: Fault) {
        self.fault = fault;
        self.control_reads = 0;
        self.pending_reads = 0;
    }
}

impl FirstWriteStore for Store {
    type Error = Fault;
    fn read_control(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ReadControl)
            || self.fail(Fault::VerifyControl) && self.control_reads >= 2
        {
            return Err(if self.fault == Fault::ReadControl {
                Fault::ReadControl
            } else {
                Fault::VerifyControl
            });
        }
        out.copy_from_slice(&self.control[Self::slot(slot)]);
        self.control_reads += 1;
        Ok(())
    }
    fn erase_control(&mut self, slot: Slot) -> Result<(), Self::Error> {
        if self.fail(Fault::EraseControl) {
            return Err(Fault::EraseControl);
        }
        self.control[Self::slot(slot)].fill(0xff);
        Ok(())
    }
    fn program_control(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ProgramControl) {
            return Err(Fault::ProgramControl);
        }
        let torn = self.fail(Fault::TornProgramControl);
        let page = &mut self.control[Self::slot(slot)];
        page.fill(0xff);
        page[..record.len()].copy_from_slice(record);
        if torn {
            page[0] ^= 1;
        }
        Ok(())
    }
    fn read_pending(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ReadPending)
            || self.fail(Fault::VerifyPending) && self.pending_reads >= 2
            || self.fail(Fault::VerifyPendingB)
                && self.pending_reads >= 2
                && matches!(slot, Slot::B)
        {
            return Err(if self.fault == Fault::ReadPending {
                Fault::ReadPending
            } else if self.fault == Fault::VerifyPendingB {
                Fault::VerifyPendingB
            } else {
                Fault::VerifyPending
            });
        }
        out.copy_from_slice(&self.pending[Self::slot(slot)]);
        self.pending_reads += 1;
        Ok(())
    }
    fn erase_pending(&mut self, slot: Slot) -> Result<(), Self::Error> {
        if self.fail(Fault::ErasePending) {
            return Err(Fault::ErasePending);
        }
        if self.fail(Fault::ErasePendingB) && matches!(slot, Slot::B) {
            return Err(Fault::ErasePendingB);
        }
        self.pending[Self::slot(slot)].fill(0xff);
        Ok(())
    }
    fn program_pending(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        if self.fail(Fault::ProgramPending) {
            return Err(Fault::ProgramPending);
        }
        let torn = self.fail(Fault::TornProgramPending);
        let page = &mut self.pending[Self::slot(slot)];
        page.fill(0xff);
        page[..record.len()].copy_from_slice(record);
        if torn {
            page[0] ^= 1;
        }
        Ok(())
    }
}

pub(super) fn run_with<R>(
    store: &mut Store,
    f: impl FnOnce(&mut Store, &mut FirstWriteScratch<'_>) -> R,
) -> R {
    let mut control_a = [0; PAGE];
    let mut control_b = [0; PAGE];
    let mut pending_a = [0; PAGE];
    let mut pending_b = [0; PAGE];
    let mut body = [0; seneschal::control::MAX_DURABLE_BODY];
    let mut page = [0; PAGE];
    let mut readback = [0; PAGE];
    f(
        store,
        &mut FirstWriteScratch::new(
            &mut control_a,
            &mut control_b,
            &mut pending_a,
            &mut pending_b,
            &mut body,
            &mut page,
            &mut readback,
        )
        .unwrap(),
    )
}

pub(super) fn staged_store() -> Store {
    let mut store = Store::blank();
    assert_eq!(
        run_with(&mut store, |store, scratch| stage_first_write(
            store,
            scratch,
            &state(),
            NODE,
            &facts()
        )),
        Ok(StageOutcome::Staged)
    );
    store
}
