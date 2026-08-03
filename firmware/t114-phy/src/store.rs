//! Board glue for the `radio-hand` identity store.
//!
//! `radio-hand` owns the record format and the A/B decision. This module owns
//! the two flash pages the records live in and the nRF52840 peripherals that
//! reach them, because those are board facts rather than portable ones.
//!
//! # When this writes, and why that is the whole of pressure point 3
//!
//! A page erase stalls the CPU for tens of milliseconds, which blanks receive. The plan's
//! third pressure point rules that writes must therefore not scatter through runtime, and
//! that anything needing them stages in RAM and commits in a declared quiet window.
//!
//! That machinery has no caller, because there are exactly two write paths and both are
//! quiet by construction:
//!
//! 1. **[`SettingsStore::load_or_create`]**, on a boot that finds nothing valid. Before the
//!    radio is configured, so nothing is listening to blank.
//! 2. **[`SettingsStore::save`]**, from the `region` and `channel` probes — each of which
//!    *resets the board* immediately afterwards, unconditionally, even if the host vanished
//!    before the reply landed. The erase is followed by a reboot, so the window it blanks is
//!    one the board was about to leave anyway.
//!
//! The invariant to keep, stated for whoever adds the third path: **a write is either before
//! the radio starts, or immediately before a reset.** A caller that wants neither — a
//! runtime-persisted peer table, a crash record written where it happened — is the one that
//! must build the staged-commit window, and it should re-read pressure point 3 first.

use embassy_nrf::Peri;
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::peripherals::{NVMC, RNG};
use embassy_nrf::rng::Rng;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use radio_hand::executive::{BoardStore, StoreFault};
use radio_hand::settings::{self, Settings};
use radio_hand::store::{self, HEADER_LEN, Slot, SlotError};

include!(concat!(env!("OUT_DIR"), "/store_region.rs"));

// build.rs checks the store against the linker's FLASH region. This checks it
// against the erase granularity the code assumes, so both halves of the claim
// that two whole pages are reserved are enforced at compile time.
const _: () = assert!(
    STORE_LENGTH as usize == 2 * PAGE_SIZE,
    "the store spans exactly the A and B pages"
);

/// Bytes of a device identity, re-exported so callers do not reach past this
/// module for it.
pub const IDENTITY_LEN: usize = settings::IDENTITY_LEN;

/// Bytes read out of each slot: the header and the largest body this build
/// writes, plus room for a longer body a later firmware might have left. Reading
/// past what we understand is how a downgrade keeps someone else's fields
/// instead of truncating them into nonsense.
const SLOT_READ_LEN: usize = HEADER_LEN + settings::ENCODED_LEN + 32;

/// What the boot path found in flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Stored settings were read back. The expected result of every boot after
    /// the first.
    Loaded { slot: Slot, sequence: u32 },
    /// Both slots were erased, so this board had no identity yet and one was
    /// generated and written.
    Created { slot: Slot },
    /// Both slots held something, and neither decoded. A fresh identity
    /// replaced them. The board stays usable and says so rather than hanging.
    Replaced { slot: Slot, reason: SlotError },
}

/// Why the board could not reach a usable identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// A slot would not erase.
    Erase,
    /// A slot would not take the record.
    Write,
    /// The record was written but did not read back. Flash is failing, or the
    /// region is not the one the linker reserved.
    Verify,
}

/// The two flash pages, and the peripherals that reach them.
pub struct SettingsStore<'d> {
    nvmc: Nvmc<'d>,
    rng: Rng<'d, Blocking>,
}

impl<'d> SettingsStore<'d> {
    pub fn new(nvmc: Peri<'d, NVMC>, rng: Peri<'d, RNG>) -> Self {
        let rng = Rng::new_blocking(rng);
        // The nRF52840 RNG is a physical noise source whose raw output is
        // slightly biased. Correction costs throughput, which does not matter
        // for 64 bytes taken once, and this is key material.
        rng.set_bias_correction(true);
        Self {
            nvmc: Nvmc::new(nvmc),
            rng,
        }
    }

    /// Read the board's settings, generating and persisting an identity if flash
    /// holds nothing this build can read.
    pub fn load_or_create(&mut self) -> Result<(Settings, Outcome), Error> {
        let mut page_a = [0_u8; SLOT_READ_LEN];
        let mut page_b = [0_u8; SLOT_READ_LEN];
        // A short read cannot fail on a region the linker reserved, but the
        // whole point of this module is to not assume that.
        if self.nvmc.read(STORE_ORIGIN, &mut page_a).is_err()
            || self
                .nvmc
                .read(STORE_ORIGIN + PAGE_SIZE as u32, &mut page_b)
                .is_err()
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

        // Nothing usable. Distinguish a board that has never been written from
        // one whose records went bad, because they mean very different things
        // to whoever is holding it.
        let reason = match (store::decode(&page_a), store::decode(&page_b)) {
            (Err(SlotError::Blank), Err(SlotError::Blank)) => None,
            (Err(other), _) => Some(other),
            (_, Err(other)) => Some(other),
            // Both decoded but the body did not: a record this build cannot
            // read, which is a length fault as far as the slot is concerned.
            _ => Some(SlotError::BadLength),
        };

        let mut identity = [0_u8; IDENTITY_LEN];
        self.rng.blocking_fill_bytes(&mut identity);
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

    /// Write new settings, keeping the identity that is already stored.
    ///
    /// Erase stalls the CPU for tens of milliseconds, blanking receive. Every caller must
    /// therefore reset the board immediately afterwards — see this module's header for the
    /// invariant and why it discharges the plan's pressure point 3.
    pub fn save(&mut self, settings: &Settings) -> Result<Outcome, Error> {
        let mut page_a = [0_u8; SLOT_READ_LEN];
        let mut page_b = [0_u8; SLOT_READ_LEN];
        if self.nvmc.read(STORE_ORIGIN, &mut page_a).is_err()
            || self
                .nvmc
                .read(STORE_ORIGIN + PAGE_SIZE as u32, &mut page_b)
                .is_err()
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

    /// Erase one slot, write the record, and read it back.
    fn persist(&mut self, slot: Slot, sequence: u32, settings: &Settings) -> Result<(), Error> {
        let offset = self.offset(slot);

        let mut body = [0_u8; settings::ENCODED_LEN];
        let body_len = settings.encode(&mut body);

        let mut encoded = [0_u8; store::encoded_len(settings::ENCODED_LEN)];
        // The body length is fixed by ENCODED_LEN and the buffer is sized from
        // it, so neither encode error is reachable here.
        let written =
            store::encode(sequence, &body[..body_len], &mut encoded).map_err(|_| Error::Write)?;

        self.nvmc
            .erase(offset, offset + PAGE_SIZE as u32)
            .map_err(|_| Error::Erase)?;
        self.nvmc
            .write(offset, &encoded[..written])
            .map_err(|_| Error::Write)?;

        // Flash that accepts a write and does not return it is worse than flash
        // that refuses, because the failure would surface as a lost identity
        // one power cycle later instead of now.
        let mut check = [0_u8; SLOT_READ_LEN];
        self.nvmc
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
            Slot::B => STORE_ORIGIN + PAGE_SIZE as u32,
        }
    }
}

/// The store, as the executive sees it.
///
/// The executive owns the flash and the entropy, per structural decision 4, and reaches both
/// through this. Bundling them is not a convenience: they are one object here, because the
/// same struct holds the NVMC pages and the hardware RNG.
impl BoardStore for SettingsStore<'_> {
    /// The nRF52840's physical noise source, bias-corrected — see [`SettingsStore::new`].
    /// Infallible on this board: the peripheral is always present, so the only reason this
    /// returns a `Result` is that other boards have no such thing.
    fn random(&mut self, out: &mut [u8]) -> Result<(), StoreFault> {
        self.rng.blocking_fill_bytes(out);
        Ok(())
    }

    fn save(&mut self, settings: &Settings) -> Result<(), StoreFault> {
        SettingsStore::save(self, settings)
            .map(|_| ())
            .map_err(|_| StoreFault::Write)
    }
}

/// A boot line reporting where the identity came from.
///
/// Slot and sequence are not secret and are exactly what proves persistence: a
/// power cycle must report the same pair. Key material is never rendered.
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
