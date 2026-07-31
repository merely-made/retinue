//! Board glue for the `radio-hand` identity store.
//!
//! `radio-hand` owns the record format and the A/B decision. This module owns
//! the two flash pages the records live in and the nRF52840 peripherals that
//! reach them, because those are board facts rather than portable ones.
//!
//! The store is written once, on the first boot that finds nothing valid. Flash
//! erase stalls the CPU for tens of milliseconds per page, so a write must never
//! land in the middle of a transfer; boot is the only place this module writes.

use embassy_nrf::Peri;
use embassy_nrf::mode::Blocking;
use embassy_nrf::nvmc::{Nvmc, PAGE_SIZE};
use embassy_nrf::peripherals::{NVMC, RNG};
use embassy_nrf::rng::Rng;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};
use radio_hand::store::{self, HEADER_LEN, Slot, SlotError};

include!(concat!(env!("OUT_DIR"), "/store_region.rs"));

// build.rs checks the store against the linker's FLASH region. This checks it
// against the erase granularity the code assumes, so both halves of the claim
// that two whole pages are reserved are enforced at compile time.
const _: () = assert!(
    STORE_LENGTH as usize == 2 * PAGE_SIZE,
    "the store spans exactly the A and B pages"
);

/// Bytes of a device identity: an ed25519 signing key followed by an x25519
/// encryption key, the shape `retinue`'s `PrivateIdentity::from_secret_bytes`
/// consumes.
///
/// The board treats these as opaque key material until gate N3 gives it a node
/// to hand them to. They never leave the board.
pub const IDENTITY_LEN: usize = 64;

/// Bytes read out of each slot. Only the header and one identity are needed, so
/// the boot path never buffers a whole 4 KiB page. Growing the body means
/// growing this.
const SLOT_READ_LEN: usize = HEADER_LEN + IDENTITY_LEN;

/// What the boot path found in flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// A stored identity was read back. The expected result of every boot after
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
pub struct IdentityStore<'d> {
    nvmc: Nvmc<'d>,
    rng: Rng<'d, Blocking>,
}

impl<'d> IdentityStore<'d> {
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

    /// Read the device identity, generating and persisting one if flash holds
    /// nothing this build can read.
    pub fn load_or_create(&mut self) -> Result<([u8; IDENTITY_LEN], Outcome), Error> {
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
            && record.body.len() == IDENTITY_LEN
        {
            let mut identity = [0_u8; IDENTITY_LEN];
            identity.copy_from_slice(record.body);
            return Ok((
                identity,
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
            // Both decoded but the body was the wrong length: a record this
            // build does not understand, which is a length fault.
            _ => Some(SlotError::BadLength),
        };

        let mut identity = [0_u8; IDENTITY_LEN];
        self.rng.blocking_fill_bytes(&mut identity);
        let slot = selection.next;
        self.persist(slot, selection.next_sequence, &identity)?;

        Ok((
            identity,
            match reason {
                None => Outcome::Created { slot },
                Some(reason) => Outcome::Replaced { slot, reason },
            },
        ))
    }

    /// Erase one slot, write the record, and read it back.
    fn persist(&mut self, slot: Slot, sequence: u32, body: &[u8]) -> Result<(), Error> {
        let offset = self.offset(slot);

        let mut encoded = [0_u8; store::encoded_len(IDENTITY_LEN)];
        // The body length is fixed by IDENTITY_LEN and the buffer is sized from
        // it, so neither encode error is reachable here.
        let written = store::encode(sequence, body, &mut encoded).map_err(|_| Error::Write)?;

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
            Ok(record) if record.sequence == sequence && record.body == body => Ok(()),
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
