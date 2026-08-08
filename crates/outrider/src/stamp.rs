//! LXMF proof-of-work stamps.
//!
//! A stamp is a 32-byte nonce scored by the number of leading zero bits in
//! `SHA256(workblock || stamp)`. The workblock deliberately makes each trial
//! expensive. Propagation stamps derive it from the transient message id with
//! 1,000 expansion rounds.

use alloc::vec::Vec;

use hkdf::Hkdf;
use sha2::{Digest, Sha256};

pub const STAMP_LEN: usize = 32;
pub const WORKBLOCK_BYTES_PER_ROUND: usize = 256;
pub const MESSAGE_WORKBLOCK_ROUNDS: u32 = 3_000;
pub const PROPAGATION_WORKBLOCK_ROUNDS: u32 = 1_000;
/// The widest MessagePack unsigned integer: a marker byte and eight big-endian bytes.
const MAX_UINT_LEN: usize = 9;

/// Derive the stamp workblock observed from stock LXMF.
///
/// Each round uses `SHA256(material || msgpack(round))` as the salt for a
/// 256-byte HKDF-SHA256 expansion of `material`, with empty info.
pub fn workblock(material: &[u8], rounds: u32) -> Vec<u8> {
    let mut block = Vec::with_capacity(rounds as usize * WORKBLOCK_BYTES_PER_ROUND);
    for round in 0..rounds {
        let start = block.len();
        block.resize(start + WORKBLOCK_BYTES_PER_ROUND, 0);
        expand_round(material, round, &mut block[start..]);
    }
    block
}

/// Derive one round of the workblock into `out`, which must be one round long.
fn expand_round(material: &[u8], round: u32, out: &mut [u8]) {
    let mut encoded = [0_u8; MAX_UINT_LEN];
    let width = write_uint(&mut encoded, u64::from(round));
    // Hashed in two updates rather than over a joined buffer: same bytes, no allocation, so
    // this is callable from a round loop that must not touch the heap.
    let mut salt = Sha256::new();
    salt.update(material);
    salt.update(&encoded[..width]);

    Hkdf::<Sha256>::new(Some(&salt.finalize()), material)
        .expand(&[], out)
        .expect("256 bytes is a valid HKDF-SHA256 output length");
}

/// Score a stamp against a workblock that is never held.
///
/// [`workblock`] materialises `rounds * WORKBLOCK_BYTES_PER_ROUND` bytes: 256 KB at
/// propagation cost, 768 KB at message cost. No board in this family can hold either, and
/// the T114's whole heap is 48 KB, so on the hardware this crate is meant to reach, the
/// materialised form is not slow but impossible.
///
/// It is also unnecessary for checking. The workblock's only use is to be fed into a hash,
/// so each round can go into the hasher as it is derived and be dropped, leaving one round
/// on the stack rather than all of them on the heap. The result is the same number by
/// construction, and `the_streamed_score_is_the_materialised_one` holds that.
///
/// This is the checking side. Minting looked like a different problem, because [`find`]
/// scores many nonces against one workblock and streaming would re-derive every round per
/// trial. It is not, and [`find_streamed`] is why: the derivation's product can be the
/// hasher rather than the bytes.
pub fn value_streamed(material: &[u8], rounds: u32, stamp: &[u8; STAMP_LEN]) -> u16 {
    let mut derivation = Derivation::new(material, rounds);
    derivation.advance(rounds);
    derivation
        .value(stamp)
        .expect("the whole round budget was advanced")
}

/// A workblock derivation in progress, advanced a budget of rounds at a time.
///
/// [`value_streamed`] and [`find_streamed`] run the whole derivation in one call, which is
/// seconds of solid CPU at real round counts. Fine on a host. On a board that call starves
/// the executor that also feeds the radio, the USB link and the watchdog; the measured
/// check was 1.9 s of silence, and message cost at 5.6 s approaches the 8 s watchdog.
///
/// This is the same derivation held as a value the caller schedules: advance a budget of
/// rounds, yield, repeat, then score or mint from the finished state as many times as
/// wanted. Rounds are 256 bytes and SHA-256 absorbs in 64-byte blocks, so the hasher's
/// buffer is empty at every pause and the finished state is a midstate of about a hundred
/// bytes that is, for scoring purposes, the whole workblock.
pub struct Derivation<'a> {
    material: &'a [u8],
    hash: Sha256,
    round: u32,
    rounds: u32,
}

impl<'a> Derivation<'a> {
    pub fn new(material: &'a [u8], rounds: u32) -> Self {
        Self {
            material,
            hash: Sha256::new(),
            round: 0,
            rounds,
        }
    }

    /// Absorb up to `budget` more rounds. Returns the rounds still to go.
    pub fn advance(&mut self, budget: u32) -> u32 {
        let mut round_block = [0_u8; WORKBLOCK_BYTES_PER_ROUND];
        let until = self.rounds.min(self.round.saturating_add(budget));
        while self.round < until {
            expand_round(self.material, self.round, &mut round_block);
            self.hash.update(round_block);
            self.round += 1;
        }
        self.rounds - self.round
    }

    pub fn done(&self) -> bool {
        self.round == self.rounds
    }

    /// Score one stamp against the finished derivation. `None` before [`Self::done`].
    pub fn value(&self, stamp: &[u8; STAMP_LEN]) -> Option<u16> {
        if !self.done() {
            return None;
        }
        let mut hash = self.hash.clone();
        hash.update(stamp);
        Some(leading_zero_bits(&hash.finalize()))
    }

    /// Try up to `attempts` sequential nonces from `seed`, advancing it in place so the
    /// search resumes where it stopped on the next call. Also `None` before
    /// [`Self::done`], which a caller that just drained [`Self::advance`] cannot hit.
    pub fn mint(
        &self,
        target: u16,
        seed: &mut [u8; STAMP_LEN],
        attempts: u64,
    ) -> Option<([u8; STAMP_LEN], u16)> {
        if !self.done() || target > 256 {
            return None;
        }
        for _ in 0..attempts {
            let mut hash = self.hash.clone();
            hash.update(*seed);
            let score = leading_zero_bits(&hash.finalize());
            if score >= target {
                return Some((*seed, score));
            }
            increment(seed);
        }
        None
    }
}

/// Score a stamp against a previously derived workblock.
pub fn value(workblock: &[u8], stamp: &[u8; STAMP_LEN]) -> u16 {
    let mut hash = Sha256::new();
    hash.update(workblock);
    hash.update(stamp);
    leading_zero_bits(&hash.finalize())
}

pub fn valid(workblock: &[u8], stamp: &[u8; STAMP_LEN], target: u16) -> bool {
    target <= 256 && value(workblock, stamp) >= target
}

/// Check a stamp against a target without ever holding the workblock.
///
/// The counterpart to [`valid`] for callers that have the material rather than a derived
/// block. Same answer, no allocation; see [`value_streamed`] for why the materialised form
/// is not merely wasteful but impossible on the hardware this crate targets.
pub fn valid_streamed(material: &[u8], rounds: u32, stamp: &[u8; STAMP_LEN], target: u16) -> bool {
    target <= 256 && value_streamed(material, rounds, stamp) >= target
}

/// Score a propagation stamp. Streamed, so it costs one round of stack rather than 256 KB
/// of heap and runs anywhere, including a board.
pub fn propagation_value(transient_id: &[u8; 32], stamp: &[u8; STAMP_LEN]) -> u16 {
    value_streamed(transient_id, PROPAGATION_WORKBLOCK_ROUNDS, stamp)
}

pub fn propagation_valid(transient_id: &[u8; 32], stamp: &[u8; STAMP_LEN], target: u16) -> bool {
    target <= 256 && propagation_value(transient_id, stamp) >= target
}

/// Search sequential 256-bit stamp nonces, starting with `seed`.
///
/// The seed does not need to be secret. The caller controls the attempt bound
/// so embedded and interactive runtimes can enforce their own work budget.
pub fn find(
    workblock: &[u8],
    target: u16,
    mut seed: [u8; STAMP_LEN],
    max_attempts: u64,
) -> Option<([u8; STAMP_LEN], u16)> {
    if target > 256 {
        return None;
    }
    for _ in 0..max_attempts {
        let score = value(workblock, &seed);
        if score >= target {
            return Some((seed, score));
        }
        increment(&mut seed);
    }
    None
}

/// MessagePack's encoding of an unsigned integer, in the narrowest form that holds it.
///
/// This one call was the only thing tying stamps to `rmpv`, and so the only thing keeping
/// proof-of-work off a board. It is written out rather than called out to, and the width
/// rules are not a style choice: these bytes are hashed into the workblock salt, so an
/// encoding one byte wider than the stock implementation's is not a different spelling of the
/// same number, it is a different workblock, against which every stamp ever minted or checked
/// scores wrong. `the_narrow_encoding_is_the_one_rmpv_writes` holds that at every boundary.
fn write_uint(out: &mut [u8; MAX_UINT_LEN], value: u64) -> usize {
    fn tagged(out: &mut [u8; MAX_UINT_LEN], marker: u8, bytes: &[u8]) -> usize {
        out[0] = marker;
        out[1..=bytes.len()].copy_from_slice(bytes);
        1 + bytes.len()
    }
    match value {
        0..=0x7f => {
            out[0] = value as u8;
            1
        }
        0x80..=0xff => tagged(out, 0xcc, &[value as u8]),
        0x100..=0xffff => tagged(out, 0xcd, &(value as u16).to_be_bytes()),
        0x1_0000..=0xffff_ffff => tagged(out, 0xce, &(value as u32).to_be_bytes()),
        _ => tagged(out, 0xcf, &value.to_be_bytes()),
    }
}

/// Search sequential nonces without ever holding the workblock.
///
/// [`find`] scores each trial against materialised workblock bytes, which no board in this
/// family can hold. Here the one streamed derivation ends in [`Derivation`]'s midstate,
/// each trial clones that and pays one compression for the stamp block, and the nonce walk
/// is [`find`]'s exactly, so the two searches return the same stamp from the same seed.
/// `streamed_and_materialised_mints_agree` holds them together.
///
/// The expected trial count for a target is `2^target` with a geometric tail, so the caller
/// still sets `max_attempts` to what its patience allows. On the T114 a trial costs one
/// compression, roughly 65 us, so expected trial time passes the 1.9 s derivation near
/// target 15; below that, minting costs about what one check costs.
pub fn find_streamed(
    material: &[u8],
    rounds: u32,
    target: u16,
    mut seed: [u8; STAMP_LEN],
    max_attempts: u64,
) -> Option<([u8; STAMP_LEN], u16)> {
    if target > 256 {
        return None;
    }
    let mut derivation = Derivation::new(material, rounds);
    derivation.advance(rounds);
    derivation.mint(target, &mut seed, max_attempts)
}

fn increment(stamp: &mut [u8; STAMP_LEN]) {
    for byte in stamp.iter_mut().rev() {
        let (next, carried) = byte.overflowing_add(1);
        *byte = next;
        if !carried {
            break;
        }
    }
}

fn leading_zero_bits(bytes: &[u8]) -> u16 {
    let mut count = 0;
    for byte in bytes {
        if *byte == 0 {
            count += 8;
        } else {
            count += byte.leading_zeros() as u16;
            break;
        }
    }
    count
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_round_matches_the_stock_oracle() {
        let material: Vec<u8> = (0..32).collect();
        let block = workblock(&material, 1);
        assert_eq!(block.len(), 256);
        assert_eq!(
            hex::encode(&block[..32]),
            "c025bbe68a4017092b9878de5c0819fafc668096b2208a3f1caa61563d5d7bd4"
        );
        assert_eq!(
            hex::encode(Sha256::digest(&block)),
            "7fbf49b7a5e79e4a70268b4267e6f9abb6540f8b265b6fb13ed9b58494c7a581"
        );
    }

    #[test]
    fn captured_propagation_stamp_has_stock_value_fourteen() {
        let transient_id =
            hex::decode("511062e686831fddd01061401b86c69d1e7d672595be43a54a7046f1c118698c")
                .unwrap()
                .try_into()
                .unwrap();
        let stamp = hex::decode("016782ee98406598318eb0bc18a5065b2f6d64e8ac419a1a30c816180e4da7e5")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(propagation_value(&transient_id, &stamp), 14);
        assert!(propagation_valid(&transient_id, &stamp, 13));
        assert!(!propagation_valid(&transient_id, &stamp, 15));
    }

    /// The hand-written integer encoding is byte-for-byte what `rmpv` wrote, at every width
    /// boundary and on both sides of each. Kept as a test rather than settled once by reading,
    /// because it is what stops a future edit from silently re-salting every workblock in the
    /// network. The oracle vectors above would catch a change at round 0..3000; this catches
    /// one anywhere.
    #[cfg(feature = "std")]
    #[test]
    fn the_narrow_encoding_is_the_one_rmpv_writes() {
        let boundaries = [
            0_u64,
            1,
            0x7f,
            0x80,
            0xff,
            0x100,
            0xffff,
            0x1_0000,
            0xffff_ffff,
            0x1_0000_0000,
            u64::MAX,
        ];
        let sweep = boundaries.iter().copied().chain(0..4096).chain([
            MESSAGE_WORKBLOCK_ROUNDS as u64,
            PROPAGATION_WORKBLOCK_ROUNDS as u64,
        ]);
        for value in sweep {
            let mut buffer = [0_u8; MAX_UINT_LEN];
            let width = write_uint(&mut buffer, value);
            let mine = &buffer[..width];

            let mut theirs = Vec::new();
            rmpv::encode::write_value(&mut theirs, &rmpv::Value::from(value)).unwrap();

            assert_eq!(
                mine,
                &theirs[..],
                "{value} encodes differently than rmpv writes it"
            );
        }
    }

    /// Streaming and materialising are the same number, at round counts where holding the
    /// workblock is still possible on a host. The captured propagation vector above is the
    /// same claim at the real 1,000 rounds, where a board cannot hold it at all.
    #[test]
    fn the_streamed_score_is_the_materialised_one() {
        let material: Vec<u8> = (0..32).collect();
        let stamp = [0x5a_u8; STAMP_LEN];
        for rounds in [0_u32, 1, 2, 7, 64] {
            assert_eq!(
                value_streamed(&material, rounds, &stamp),
                value(&workblock(&material, rounds), &stamp),
                "{rounds} rounds",
            );
        }
    }

    /// The streamed mint and the materialised mint walk the same nonce path to the same
    /// stamp, which is the whole claim: the cloned hasher is the workblock, for scoring.
    #[test]
    fn streamed_and_materialised_mints_agree() {
        let material: Vec<u8> = (0..32).collect();
        for rounds in [1_u32, 3, 16] {
            let block = workblock(&material, rounds);
            let materialised = find(&block, 6, [7; STAMP_LEN], 100_000).unwrap();
            let streamed = find_streamed(&material, rounds, 6, [7; STAMP_LEN], 100_000).unwrap();
            assert_eq!(streamed, materialised, "{rounds} rounds");
            assert_eq!(
                value_streamed(&material, rounds, &streamed.0),
                streamed.1,
                "{rounds} rounds",
            );
        }
    }

    /// Slicing the derivation changes nothing but when the CPU is held: uneven budgets,
    /// including zero-size ones, end at the same midstate and the same score.
    #[test]
    fn a_derivation_advanced_in_slices_scores_the_same() {
        let material: Vec<u8> = (0..32).collect();
        let stamp = [0x5a_u8; STAMP_LEN];
        let whole = value_streamed(&material, 64, &stamp);

        let mut derivation = Derivation::new(&material, 64);
        assert!(
            derivation.value(&stamp).is_none(),
            "unfinished must refuse to score"
        );
        for budget in [1_u32, 0, 7, 17, 3].iter().cycle() {
            if derivation.advance(*budget) == 0 {
                break;
            }
        }
        assert!(derivation.done());
        assert_eq!(derivation.value(&stamp), Some(whole));
    }

    /// A mint paused between attempt budgets resumes where it stopped and finds the stamp
    /// the uninterrupted search finds, because the seed carries the whole search position.
    #[test]
    fn a_mint_paused_and_resumed_finds_the_same_stamp() {
        let material: Vec<u8> = (0..32).collect();
        let expected = find_streamed(&material, 3, 6, [7; STAMP_LEN], 100_000).unwrap();

        let mut derivation = Derivation::new(&material, 3);
        derivation.advance(3);
        let mut seed = [7_u8; STAMP_LEN];
        let mut found = None;
        for _ in 0..100_000 {
            if let Some(hit) = derivation.mint(6, &mut seed, 16) {
                found = Some(hit);
                break;
            }
        }
        assert_eq!(found, Some(expected));
    }

    #[test]
    fn bounded_search_finds_and_scores_a_nonce() {
        let block = workblock(&[0x41; 32], 1);
        let (stamp, score) = find(&block, 8, [0; STAMP_LEN], 10_000).unwrap();
        assert!(score >= 8);
        assert_eq!(value(&block, &stamp), score);
    }
}
