//! ESP32-S3 backing for WN1's staged first-write A/B pair.
//!
//! This pair is deliberately separate from the ordinary RHD1 control journal. This bounded
//! pair owns first-write staging only. It remains separate from ordinary control recovery so a
//! torn first owner write cannot alter a known-good control journal.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use radio_hand::store::Slot;

use crate::control_store::{CONTROL_ORIGIN, CONTROL_SLOT_LEN};
use crate::store::{ANNOUNCE_RESERVATION_ORIGIN, SECTOR, STORE_ORIGIN, SettingsStore};

/// The staged first-write A/B pair follows the ordinary control pair.
pub(crate) const PENDING_SLOT_A_ORIGIN: u32 = 0x3F_6000;
pub(crate) const PENDING_SLOT_B_ORIGIN: u32 = 0x3F_7000;

const STORE_END: u32 = STORE_ORIGIN + 2 * SECTOR;
const ANNOUNCE_RESERVATION_END: u32 = ANNOUNCE_RESERVATION_ORIGIN + 2 * SECTOR;
const CONTROL_END: u32 = CONTROL_ORIGIN + 2 * SECTOR;
const PENDING_END: u32 = PENDING_SLOT_B_ORIGIN + SECTOR;
const FLASH_END: u32 = 0x40_0000;

const _: () = assert!(ANNOUNCE_RESERVATION_ORIGIN == STORE_END);
const _: () = assert!(CONTROL_ORIGIN == ANNOUNCE_RESERVATION_END);
const _: () = assert!(PENDING_SLOT_A_ORIGIN == CONTROL_END);
const _: () = assert!(PENDING_SLOT_B_ORIGIN == PENDING_SLOT_A_ORIGIN + SECTOR);
const _: () = assert!(PENDING_SLOT_A_ORIGIN % SECTOR == 0);
const _: () = assert!(PENDING_END <= FLASH_END);
const _: () = assert!(CONTROL_SLOT_LEN <= SECTOR as usize);

/// Why a staged first-write slot could not be read or changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum PendingStoreError {
    Buffer,
    Read,
    Erase,
    Write,
}

#[allow(dead_code)]
impl SettingsStore {
    /// Read one raw staged first-write slot.
    pub(crate) fn read_pending_slot(
        &mut self,
        slot: Slot,
        out: &mut [u8],
    ) -> Result<(), PendingStoreError> {
        if out.len() != CONTROL_SLOT_LEN {
            return Err(PendingStoreError::Buffer);
        }
        self.flash
            .read(self.pending_offset(slot), out)
            .map_err(|_| PendingStoreError::Read)
    }

    /// Erase one staged first-write slot. Callers own the pre-radio write
    /// window and must read back the erased sector before reporting success.
    pub(crate) fn erase_pending_slot(&mut self, slot: Slot) -> Result<(), PendingStoreError> {
        let offset = self.pending_offset(slot);
        self.flash
            .erase(offset, offset + SECTOR)
            .map_err(|_| PendingStoreError::Erase)
    }

    /// Program one staged outer record. The record format and its later
    /// readback verification belong to `radio-hand`'s portable first-write
    /// transaction, not this flash adapter.
    pub(crate) fn program_pending_slot(
        &mut self,
        slot: Slot,
        record: &[u8],
    ) -> Result<(), PendingStoreError> {
        if record.is_empty() || record.len() > CONTROL_SLOT_LEN || !record.len().is_multiple_of(4) {
            return Err(PendingStoreError::Buffer);
        }
        self.flash
            .write(self.pending_offset(slot), record)
            .map_err(|_| PendingStoreError::Write)
    }

    fn pending_offset(&self, slot: Slot) -> u32 {
        match slot {
            Slot::A => PENDING_SLOT_A_ORIGIN,
            Slot::B => PENDING_SLOT_B_ORIGIN,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_pair_is_contiguous_after_control_and_within_flash() {
        assert_eq!(PENDING_SLOT_A_ORIGIN, CONTROL_END);
        assert_eq!(PENDING_SLOT_B_ORIGIN, PENDING_SLOT_A_ORIGIN + SECTOR);
        assert!(PENDING_END <= FLASH_END);
    }
}
