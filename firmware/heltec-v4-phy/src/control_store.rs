//! ESP32-S3 backing for WN1's independent durable control journal.
//!
//! The settings and announce-reservation pairs remain in [`super::store`]. This module owns
//! only the next two sectors, and implements the portable `radio-hand` A/B slot seam over the
//! V4 flash peripheral.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use radio_hand::control::{
    self, AbSlotStore, DurableError, DurableLoadError, DurableState, JournalWrite, MAX_DURABLE_BODY,
};
use radio_hand::store::{self, Slot};

use super::store::{ANNOUNCE_RESERVATION_ORIGIN, SECTOR, STORE_ORIGIN, SettingsStore};

/// The WN1 control journal follows the settings and announce-reservation pairs. It has its
/// own A/B pair so a torn control write can never replace identity or announce state.
pub(crate) const CONTROL_ORIGIN: u32 = 0x3F_4000;

pub(crate) const CONTROL_SLOT_LEN: usize = store::encoded_len(MAX_DURABLE_BODY);
const _: () = assert!(
    MAX_DURABLE_BODY <= u16::MAX as usize,
    "the durable body must fit the outer record length field"
);
const STORE_END: u32 = STORE_ORIGIN + 2 * SECTOR;
const ANNOUNCE_RESERVATION_END: u32 = ANNOUNCE_RESERVATION_ORIGIN + 2 * SECTOR;
const CONTROL_END: u32 = CONTROL_ORIGIN + 2 * SECTOR;
const FLASH_END: u32 = 0x40_0000;
const _: () = assert!(ANNOUNCE_RESERVATION_ORIGIN == STORE_END);
const _: () = assert!(CONTROL_ORIGIN == ANNOUNCE_RESERVATION_END);
const _: () = assert!(CONTROL_ORIGIN % SECTOR == 0);
const _: () = assert!(CONTROL_END <= FLASH_END);
const _: () = assert!(CONTROL_SLOT_LEN <= SECTOR as usize);

/// Why the independent WN1 control journal could not be read or written.
///
/// Unlike settings, a nonblank corrupt pair is never repaired here: replacing it could
/// discard owner grants, a known-good configuration, or a cached replay result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum ControlError {
    Buffer,
    Read,
    Blank,
    Corrupt,
    State(DurableError),
    Erase,
    Write,
    Verify,
}

impl SettingsStore {
    /// Read the WN1 durable control state from its independent A/B pair.
    #[allow(dead_code)]
    pub(crate) fn load_control(&mut self) -> Result<DurableState, ControlError> {
        let (a, b) = self.read_control_slots()?;
        control::load(&a, &b).map_err(|error| match error {
            DurableLoadError::Blank => ControlError::Blank,
            DurableLoadError::Corrupt => ControlError::Corrupt,
            DurableLoadError::State(error) => ControlError::State(error),
        })
    }

    /// Persist one WN1 state to the inactive control slot and verify it from flash.
    ///
    /// The returned sequence belongs to the outer A/B journal. It is deliberately separate
    /// from the state's `ConfigGeneration`, which is semantic configuration state.
    #[allow(dead_code)]
    pub(crate) fn save_control(
        &mut self,
        state: &DurableState,
    ) -> Result<JournalWrite, ControlError> {
        let (a, b) = self.read_control_slots()?;
        let mut body = [0_u8; MAX_DURABLE_BODY];
        let mut encoded = [0xFF_u8; CONTROL_SLOT_LEN];
        let write = control::next_record(&a, &b, state, &mut body, &mut encoded)
            .map_err(ControlError::State)?;
        let expected = store::decode(&encoded).map_err(|_| ControlError::Verify)?;

        self.erase_slot(write.slot)?;
        self.program_slot(write.slot, &encoded[..write.len])?;

        let mut check = [0_u8; CONTROL_SLOT_LEN];
        self.read_slot(write.slot, &mut check)
            .map_err(|_| ControlError::Verify)?;
        let actual = store::decode(&check).map_err(|_| ControlError::Verify)?;
        if actual.sequence != write.sequence || actual.body != expected.body {
            return Err(ControlError::Verify);
        }
        control::decode_durable(actual.body).map_err(ControlError::State)?;
        Ok(write)
    }

    fn read_control_slots(
        &mut self,
    ) -> Result<([u8; CONTROL_SLOT_LEN], [u8; CONTROL_SLOT_LEN]), ControlError> {
        let mut a = [0_u8; CONTROL_SLOT_LEN];
        let mut b = [0_u8; CONTROL_SLOT_LEN];
        self.read_slot(Slot::A, &mut a)?;
        self.read_slot(Slot::B, &mut b)?;
        Ok((a, b))
    }

    fn control_offset(&self, slot: Slot) -> u32 {
        match slot {
            Slot::A => CONTROL_ORIGIN,
            Slot::B => CONTROL_ORIGIN + SECTOR,
        }
    }
}

/// Hardware implementation of the portable WN1 A/B slot seam. This pair is intentionally
/// separate from the settings and announce-reservation pairs.
impl AbSlotStore for SettingsStore {
    type Error = ControlError;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error> {
        if out.len() != CONTROL_SLOT_LEN {
            return Err(ControlError::Buffer);
        }
        self.flash
            .read(self.control_offset(slot), out)
            .map_err(|_| ControlError::Read)
    }

    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error> {
        let offset = self.control_offset(slot);
        self.flash
            .erase(offset, offset + SECTOR)
            .map_err(|_| ControlError::Erase)
    }

    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error> {
        if record.is_empty() || record.len() > CONTROL_SLOT_LEN || !record.len().is_multiple_of(4) {
            return Err(ControlError::Buffer);
        }
        self.flash
            .write(self.control_offset(slot), record)
            .map_err(|_| ControlError::Write)
    }
}
