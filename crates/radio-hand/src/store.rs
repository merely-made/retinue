//! An A/B slot record format for settings a board must not lose.
//!
//! A board keeps its device identity in flash. Writing that in place is not
//! atomic: a power cut part-way through leaves a record that is neither the old
//! one nor the new one. So the board keeps two slots and alternates. A write
//! goes to whichever slot is not currently authoritative, and only becomes
//! authoritative once its checksum validates and its sequence exceeds the other
//! slot's. A torn write fails its checksum and the previous slot still stands.
//!
//! ```text
//! 0..4    magic "RHS0"
//! 4..6    version, u16 LE
//! 6..8    body length, u16 LE
//! 8..12   sequence, u32 LE
//! 12..16  CRC-32 of bytes 0..12 followed by the body, u32 LE
//! 16..    body, opaque to this module
//! ```
//!
//! The body is bytes. This module does not know that a device identity lives in
//! it, in the same way `selvage` does not know what a UI snapshot contains.

/// Marks a slot that holds a record. Erased flash reads as `0xFF`, so a blank
/// slot is distinguishable from a corrupt one.
pub const MAGIC: [u8; 4] = *b"RHS0";

/// The only record version this module reads or writes.
pub const VERSION: u16 = 1;

/// Bytes ahead of the body.
pub const HEADER_LEN: usize = 16;

/// Which of the two slots a record came from, or goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    A,
    B,
}

impl Slot {
    /// The other slot.
    pub fn other(self) -> Self {
        match self {
            Slot::A => Slot::B,
            Slot::B => Slot::A,
        }
    }
}

/// Why a slot did not yield a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotError {
    /// Erased flash. Expected on a board that has never been written.
    Blank,
    /// Fewer bytes than a header.
    Truncated,
    /// Not erased, and not a record either.
    BadMagic,
    /// A record written by a version this build does not read.
    UnsupportedVersion(u16),
    /// The declared body runs past the end of the slot.
    BadLength,
    /// Header and body do not match the checksum. The expected outcome of a
    /// write interrupted by power loss.
    BadCrc,
}

/// Why a record could not be written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    /// The body is longer than the length field can describe.
    BodyTooLong,
    /// The destination cannot hold the header, the body, and word padding.
    SlotTooSmall { needed: usize, available: usize },
}

/// A record read out of a slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    pub sequence: u32,
    pub body: &'a [u8],
}

/// What a pair of slots says to do next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selection<'a> {
    /// The authoritative record, if either slot holds a valid one.
    pub active: Option<(Slot, Record<'a>)>,
    /// The slot the next write goes to. Never the active one.
    pub next: Slot,
    /// The sequence the next write carries.
    pub next_sequence: u32,
}

/// Bytes [`encode`] will write for a body of this length, including the padding
/// that keeps the total a multiple of four.
///
/// The nRF52 flash controller writes whole words, so a caller handing this
/// result to `NorFlash::write` gets an aligned length without doing the
/// arithmetic itself. It is `const` so a caller can size a buffer with it.
pub const fn encoded_len(body_len: usize) -> usize {
    (HEADER_LEN + body_len + 3) & !3
}

/// Write a record into `out`, returning the number of bytes used.
///
/// Padding to the next word boundary is written as `0xFF`, matching erased
/// flash. It sits past the declared body length, so [`decode`] ignores it and
/// the checksum does not cover it.
pub fn encode(sequence: u32, body: &[u8], out: &mut [u8]) -> Result<usize, EncodeError> {
    let body_len = u16::try_from(body.len()).map_err(|_| EncodeError::BodyTooLong)?;
    let needed = encoded_len(body.len());
    if out.len() < needed {
        return Err(EncodeError::SlotTooSmall {
            needed,
            available: out.len(),
        });
    }

    out[0..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&VERSION.to_le_bytes());
    out[6..8].copy_from_slice(&body_len.to_le_bytes());
    out[8..12].copy_from_slice(&sequence.to_le_bytes());
    out[16..16 + body.len()].copy_from_slice(body);

    let mut crc = Crc32::new();
    crc.update(&out[0..12]);
    crc.update(body);
    out[12..16].copy_from_slice(&crc.finish().to_le_bytes());

    out[HEADER_LEN + body.len()..needed].fill(0xFF);
    Ok(needed)
}

/// Read a record out of one slot.
pub fn decode(slot: &[u8]) -> Result<Record<'_>, SlotError> {
    if slot.len() < HEADER_LEN {
        return Err(SlotError::Truncated);
    }
    if slot[0..4] == [0xFF; 4] {
        return Err(SlotError::Blank);
    }
    if slot[0..4] != MAGIC {
        return Err(SlotError::BadMagic);
    }

    let version = u16::from_le_bytes([slot[4], slot[5]]);
    if version != VERSION {
        return Err(SlotError::UnsupportedVersion(version));
    }

    let body_len = usize::from(u16::from_le_bytes([slot[6], slot[7]]));
    let end = HEADER_LEN + body_len;
    if end > slot.len() {
        return Err(SlotError::BadLength);
    }

    let sequence = u32::from_le_bytes([slot[8], slot[9], slot[10], slot[11]]);
    let expected = u32::from_le_bytes([slot[12], slot[13], slot[14], slot[15]]);
    let body = &slot[HEADER_LEN..end];

    let mut crc = Crc32::new();
    crc.update(&slot[0..12]);
    crc.update(body);
    if crc.finish() != expected {
        return Err(SlotError::BadCrc);
    }

    Ok(Record { sequence, body })
}

/// Decide which slot is authoritative and where the next write goes.
///
/// The sequence counter is not checked for wraparound because it cannot wrap:
/// the nRF52840 endures on the order of 10,000 erase cycles per page, so two
/// alternating slots die after roughly 20,000 writes, and a `u32` turns over
/// after four billion.
pub fn select<'a>(a: &'a [u8], b: &'a [u8]) -> Selection<'a> {
    let decoded_a = decode(a).ok();
    let decoded_b = decode(b).ok();

    // On a tie, A wins: a torn write fails its checksum rather than landing on
    // the same sequence, so equal sequences mean a duplicate, not a race.
    let active = match (decoded_a, decoded_b) {
        (Some(ra), Some(rb)) if rb.sequence > ra.sequence => Some((Slot::B, rb)),
        (Some(ra), Some(_)) => Some((Slot::A, ra)),
        (Some(ra), None) => Some((Slot::A, ra)),
        (None, Some(rb)) => Some((Slot::B, rb)),
        (None, None) => None,
    };

    match active {
        Some((slot, record)) => Selection {
            active: Some((slot, record)),
            next: slot.other(),
            next_sequence: record.sequence.wrapping_add(1),
        },
        None => Selection {
            active: None,
            next: Slot::A,
            next_sequence: 0,
        },
    }
}

/// CRC-32 as used by IEEE 802.3, computed a bit at a time.
///
/// A table would cost 1 KiB of flash to save microseconds on a record read once
/// per boot. The board has neither the need nor the room to spare.
struct Crc32(u32);

impl Crc32 {
    fn new() -> Self {
        Crc32(0xFFFF_FFFF)
    }

    fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 ^= u32::from(byte);
            for _ in 0..8 {
                let mask = (self.0 & 1).wrapping_neg();
                self.0 = (self.0 >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
    }

    fn finish(self) -> u32 {
        !self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 4096;

    fn blank() -> [u8; PAGE] {
        [0xFF; PAGE]
    }

    fn written(sequence: u32, body: &[u8]) -> [u8; PAGE] {
        let mut page = blank();
        encode(sequence, body, &mut page).expect("body fits a page");
        page
    }

    #[test]
    fn round_trip() {
        let body = [7u8; 64];
        let page = written(3, &body);
        let record = decode(&page).expect("valid record");
        assert_eq!(record.sequence, 3);
        assert_eq!(record.body, &body[..]);
    }

    #[test]
    fn empty_body_round_trips() {
        let page = written(0, &[]);
        let record = decode(&page).expect("valid record");
        assert_eq!(record.sequence, 0);
        assert_eq!(record.body, &[] as &[u8]);
    }

    #[test]
    fn encoded_length_is_word_aligned() {
        for body_len in 0..16 {
            let len = encoded_len(body_len);
            assert_eq!(len % 4, 0, "body_len {body_len} produced {len}");
            assert!(len >= HEADER_LEN + body_len);
        }
    }

    #[test]
    fn erased_flash_is_blank_not_corrupt() {
        assert_eq!(decode(&blank()), Err(SlotError::Blank));
    }

    #[test]
    fn short_slot_is_truncated() {
        assert_eq!(decode(&[0u8; HEADER_LEN - 1]), Err(SlotError::Truncated));
    }

    #[test]
    fn foreign_bytes_are_bad_magic() {
        let mut page = written(1, &[1, 2, 3]);
        page[0] = b'X';
        assert_eq!(decode(&page), Err(SlotError::BadMagic));
    }

    #[test]
    fn future_version_is_refused_by_number() {
        let mut page = written(1, &[1, 2, 3]);
        page[4..6].copy_from_slice(&9u16.to_le_bytes());
        assert_eq!(decode(&page), Err(SlotError::UnsupportedVersion(9)));
    }

    #[test]
    fn body_running_past_the_slot_is_bad_length() {
        let mut page = [0xFFu8; 32];
        encode(1, &[1, 2, 3], &mut page).expect("fits");
        page[6..8].copy_from_slice(&64u16.to_le_bytes());
        assert_eq!(decode(&page), Err(SlotError::BadLength));
    }

    #[test]
    fn a_flipped_body_bit_is_caught() {
        let mut page = written(1, &[0u8; 64]);
        page[HEADER_LEN + 10] ^= 0x01;
        assert_eq!(decode(&page), Err(SlotError::BadCrc));
    }

    #[test]
    fn a_flipped_sequence_bit_is_caught() {
        let mut page = written(1, &[0u8; 64]);
        page[8] ^= 0x01;
        assert_eq!(decode(&page), Err(SlotError::BadCrc));
    }

    #[test]
    fn padding_is_outside_the_checksum() {
        let body = [5u8; 5];
        let mut page = written(2, &body);
        let len = encoded_len(body.len());
        page[len - 1] = 0x00;
        let record = decode(&page).expect("padding is not covered");
        assert_eq!(record.body, &body[..]);
    }

    #[test]
    fn body_longer_than_the_length_field_is_refused() {
        let body = [0u8; 70_000];
        let mut out = [0u8; 70_016];
        assert_eq!(encode(0, &body, &mut out), Err(EncodeError::BodyTooLong));
    }

    #[test]
    fn a_slot_too_small_reports_both_sizes() {
        let mut out = [0u8; 8];
        assert_eq!(
            encode(0, &[1, 2, 3], &mut out),
            Err(EncodeError::SlotTooSmall {
                needed: 20,
                available: 8,
            })
        );
    }

    #[test]
    fn a_fresh_board_writes_slot_a_at_sequence_zero() {
        let (a, b) = (blank(), blank());
        let selection = select(&a, &b);
        assert_eq!(selection.active, None);
        assert_eq!(selection.next, Slot::A);
        assert_eq!(selection.next_sequence, 0);
    }

    #[test]
    fn the_higher_sequence_wins_in_either_position() {
        let older = written(4, b"old");
        let newer = written(5, b"new");

        let selection = select(&newer, &older);
        assert_eq!(selection.active.map(|(s, _)| s), Some(Slot::A));
        assert_eq!(selection.next, Slot::B);
        assert_eq!(selection.next_sequence, 6);

        let selection = select(&older, &newer);
        assert_eq!(selection.active.map(|(s, _)| s), Some(Slot::B));
        assert_eq!(selection.next, Slot::A);
        assert_eq!(selection.next_sequence, 6);
    }

    #[test]
    fn a_torn_write_leaves_the_previous_record_standing() {
        let good = written(9, b"survivor");
        let mut torn = written(10, b"interrupted");
        torn[HEADER_LEN + 2] ^= 0xFF;

        let selection = select(&good, &torn);
        let (slot, record) = selection.active.expect("the intact slot still stands");
        assert_eq!(slot, Slot::A);
        assert_eq!(record.sequence, 9);
        assert_eq!(record.body, b"survivor");
        // The next write retries the slot that lost its record.
        assert_eq!(selection.next, Slot::B);
        assert_eq!(selection.next_sequence, 10);
    }

    #[test]
    fn a_half_erased_slot_reads_as_blank_and_is_reused() {
        let good = written(2, b"kept");
        let empty = blank();
        let selection = select(&good, &empty);
        assert_eq!(selection.next, Slot::B);
        assert_eq!(selection.next_sequence, 3);
    }

    #[test]
    fn writes_alternate_slots_and_climb() {
        let mut a = blank();
        let mut b = blank();

        for expected_sequence in 0..8u32 {
            let selection = select(&a, &b);
            assert_eq!(selection.next_sequence, expected_sequence);
            let target = selection.next;
            let mut page = blank();
            encode(expected_sequence, b"body", &mut page).expect("fits");
            match target {
                Slot::A => a = page,
                Slot::B => b = page,
            }
            assert_eq!(
                select(&a, &b).active.map(|(slot, _)| slot),
                Some(target),
                "the slot just written became authoritative"
            );
        }
    }

    #[test]
    fn checksum_matches_the_published_vector() {
        // "123456789" is the standard CRC-32 check value.
        let mut crc = Crc32::new();
        crc.update(b"123456789");
        assert_eq!(crc.finish(), 0xCBF4_3926);
    }
}
