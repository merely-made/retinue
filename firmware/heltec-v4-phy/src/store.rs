//! Board glue for the `radio-hand` settings store, ESP32-S3 side.
//!
//! `radio-hand` owns the record format and the A/B decision; this owns the two flash sectors
//! the records live in and the peripherals that reach them. Pressure point 5 predicted
//! exactly this shape: "a persistence backend for `radio-hand::store`'s record format, which
//! is deliberately format-only: NVMC here, a partition on ESP32". The format ported
//! unchanged; only this file is new.
//!
//! # Entropy, and why this is not `Rng`
//!
//! The obvious `esp_hal::rng::Rng` would have been wrong, quietly. On the ESP32-S3 it is
//! *true* random only while the RF subsystem is running or an ADC is feeding the sampler —
//! and this firmware runs neither, because its radio is an external SX1262 and Wi-Fi and
//! Bluetooth are off. Wiring it would have generated the board's **identity key** from a
//! pseudo-random sequence, which is a predictable private key: a silent security defect that
//! nothing downstream could detect.
//!
//! So this holds a [`TrngSource`] over ADC1 for the life of the board, which is the
//! condition the hardware documents for real entropy, and [`Trng::try_new`] fails loudly if
//! that source is ever absent. The T114 sets the standard here — a physical noise source with
//! bias correction — and a second board joins on the same terms or refuses.
//!
//! # When this writes
//!
//! The same invariant the T114's store states: a write is either before the radio starts, or
//! immediately before a reset. See that module's header for why that discharges the plan's
//! pressure point 3 without a staged-commit window.

use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use esp_hal::peripherals::{ADC1, FLASH, RNG};
use esp_hal::rng::{Trng, TrngSource};
use esp_storage::FlashStorage;
use radio_hand::executive::{BoardStore, StoreFault};
use radio_hand::settings::{self, Settings};
use radio_hand::store::{self, HEADER_LEN, Slot, SlotError};

/// Where the settings live in the ESP32-S3's flash.
///
/// Chosen above the application image and below the 4 MB part's end, on a 4 KiB sector
/// boundary. This board has no partition table this firmware honours — it is flashed as a
/// bare image — so the region is a firmware fact, and moving it strands existing records the
/// same way re-carving the T114's `memory.x` would.
const STORE_ORIGIN: u32 = 0x3F_0000;

/// The flash sector size the ESP32-S3 erases in.
const SECTOR: u32 = 4096;

/// Bytes read out of each slot: the header, the largest body this build writes, and room for
/// a longer body a later firmware might have left.
const SLOT_READ_LEN: usize = HEADER_LEN + settings::ENCODED_LEN + 32;

/// What the boot path found in flash. Mirrors the T114's vocabulary deliberately: the two
/// boards should describe the same situations with the same words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Loaded { slot: Slot, sequence: u32 },
    Created { slot: Slot },
    Replaced { slot: Slot, reason: SlotError },
}

/// Why the board could not reach a usable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A sector would not erase.
    Erase,
    /// A sector would not take the record.
    Write,
    /// The record was written but did not read back.
    Verify,
    /// No true-random source, so no identity was generated. Refusing beats minting a
    /// predictable key.
    NoEntropy,
}

/// The two flash sectors, and the entropy source that seeds an identity.
pub struct SettingsStore {
    flash: FlashStorage<'static>,
    /// Held for the life of the store: dropping it would disable the ADC entropy source and
    /// silently downgrade every later `random` call to pseudo-random.
    _trng_source: TrngSource<'static>,
}

impl SettingsStore {
    pub fn new(flash: FLASH<'static>, rng: RNG<'static>, adc: ADC1<'static>) -> Self {
        Self {
            flash: FlashStorage::new(flash),
            _trng_source: TrngSource::new(rng, adc),
        }
    }

    /// Read the board's settings, generating and persisting an identity if flash holds
    /// nothing this build can read.
    pub fn load_or_create(&mut self) -> Result<(Settings, Outcome), Error> {
        let mut page_a = [0_u8; SLOT_READ_LEN];
        let mut page_b = [0_u8; SLOT_READ_LEN];
        if self.flash.read(STORE_ORIGIN, &mut page_a).is_err()
            || self.flash.read(STORE_ORIGIN + SECTOR, &mut page_b).is_err()
        {
            return Err(Error::Verify);
        }

        let selection = store::select(&page_a, &page_b);
        if let Some((slot, record)) = selection.active
            && let Ok(settings) = Settings::decode(record.body)
        {
            return Ok((
                settings,
                Outcome::Loaded {
                    slot,
                    sequence: record.sequence,
                },
            ));
        }

        let reason = match (store::decode(&page_a), store::decode(&page_b)) {
            (Err(SlotError::Blank), Err(SlotError::Blank)) => None,
            (Err(other), _) => Some(other),
            (_, Err(other)) => Some(other),
            _ => Some(SlotError::BadLength),
        };

        let mut identity = [0_u8; settings::IDENTITY_LEN];
        self.random(&mut identity).map_err(|_| Error::NoEntropy)?;
        let settings = Settings::new(identity);
        let slot = selection.next;
        self.persist(slot, selection.next_sequence, &settings)?;

        Ok((
            settings,
            match reason {
                None => Outcome::Created { slot },
                Some(reason) => Outcome::Replaced { slot, reason },
            },
        ))
    }

    /// Write new settings, keeping the identity already stored.
    ///
    /// Erase stalls and blanks receive, so every caller resets the board immediately after.
    pub fn save(&mut self, settings: &Settings) -> Result<Outcome, Error> {
        let mut page_a = [0_u8; SLOT_READ_LEN];
        let mut page_b = [0_u8; SLOT_READ_LEN];
        if self.flash.read(STORE_ORIGIN, &mut page_a).is_err()
            || self.flash.read(STORE_ORIGIN + SECTOR, &mut page_b).is_err()
        {
            return Err(Error::Verify);
        }
        let selection = store::select(&page_a, &page_b);
        let slot = selection.next;
        self.persist(slot, selection.next_sequence, settings)?;
        Ok(Outcome::Loaded {
            slot,
            sequence: selection.next_sequence,
        })
    }

    /// Erase one sector, write the record, and read it back.
    fn persist(&mut self, slot: Slot, sequence: u32, settings: &Settings) -> Result<(), Error> {
        let offset = self.offset(slot);

        let mut body = [0_u8; settings::ENCODED_LEN];
        let body_len = settings.encode(&mut body);

        let mut encoded = [0_u8; store::encoded_len(settings::ENCODED_LEN)];
        let written =
            store::encode(sequence, &body[..body_len], &mut encoded).map_err(|_| Error::Write)?;

        self.flash
            .erase(offset, offset + SECTOR)
            .map_err(|_| Error::Erase)?;
        self.flash
            .write(offset, &encoded[..written])
            .map_err(|_| Error::Write)?;

        // Flash that accepts a write and does not return it is worse than flash that
        // refuses: the failure would surface as a lost identity one power cycle later.
        let mut check = [0_u8; SLOT_READ_LEN];
        self.flash
            .read(offset, &mut check)
            .map_err(|_| Error::Verify)?;
        match store::decode(&check) {
            Ok(record) if record.sequence == sequence && record.body == &body[..body_len] => Ok(()),
            _ => Err(Error::Verify),
        }
    }

    fn offset(&self, slot: Slot) -> u32 {
        match slot {
            Slot::A => STORE_ORIGIN,
            Slot::B => STORE_ORIGIN + SECTOR,
        }
    }
}

/// The store, as the executive sees it. Same contract as the T114's.
impl BoardStore for SettingsStore {
    /// True random or nothing. [`Trng::try_new`] fails when no entropy source is active,
    /// which is the case this refuses rather than papering over — see the module header.
    fn random(&mut self, out: &mut [u8]) -> Result<(), StoreFault> {
        let trng = Trng::try_new().map_err(|_| StoreFault::Unavailable)?;
        trng.read(out);
        Ok(())
    }

    fn save(&mut self, settings: &Settings) -> Result<(), StoreFault> {
        SettingsStore::save(self, settings)
            .map(|_| ())
            .map_err(|_| StoreFault::Write)
    }
}

/// A boot line reporting where the identity came from. Same shape as the T114's, so one
/// bench script reads both boards.
pub fn describe(outcome: Outcome, out: &mut [u8; 48]) -> usize {
    let (label, slot, sequence) = match outcome {
        Outcome::Loaded { slot, sequence } => (&b"loaded"[..], slot, Some(sequence)),
        Outcome::Created { slot } => (&b"created"[..], slot, None),
        Outcome::Replaced { slot, .. } => (&b"replaced"[..], slot, None),
    };

    let mut at = 0;
    let mut push = |bytes: &[u8], at: &mut usize| {
        let end = (*at + bytes.len()).min(out.len());
        out[*at..end].copy_from_slice(&bytes[..end - *at]);
        *at = end;
    };

    push(b"identity=", &mut at);
    push(label, &mut at);
    push(b" slot=", &mut at);
    push(
        match slot {
            Slot::A => b"A",
            Slot::B => b"B",
        },
        &mut at,
    );
    if let Some(sequence) = sequence {
        push(b" seq=", &mut at);
        let mut digits = [0_u8; 10];
        let mut count = 0;
        let mut value = sequence;
        loop {
            digits[count] = b'0' + (value % 10) as u8;
            count += 1;
            value /= 10;
            if value == 0 {
                break;
            }
        }
        for index in (0..count).rev() {
            push(&[digits[index]], &mut at);
        }
    }
    push(b"\r\n", &mut at);
    at
}
