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
/// This is the checking side, which is the side a board needs: it must weigh the work on
/// what arrives. Minting a stamp is a different problem, because [`find`] scores many
/// nonces against one workblock, and streaming would re-derive every round for every trial.
/// A board that must mint stamps needs that addressed separately, not this function.
pub fn value_streamed(material: &[u8], rounds: u32, stamp: &[u8; STAMP_LEN]) -> u16 {
    let mut hash = Sha256::new();
    let mut round_block = [0_u8; WORKBLOCK_BYTES_PER_ROUND];
    for round in 0..rounds {
        expand_round(material, round, &mut round_block);
        hash.update(round_block);
    }
    hash.update(stamp);
    leading_zero_bits(&hash.finalize())
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
        let sweep = boundaries
            .iter()
            .copied()
            .chain(0..4096)
            .chain([MESSAGE_WORKBLOCK_ROUNDS as u64, PROPAGATION_WORKBLOCK_ROUNDS as u64]);
        for value in sweep {
            let mut buffer = [0_u8; MAX_UINT_LEN];
            let width = write_uint(&mut buffer, value);
            let mine = &buffer[..width];

            let mut theirs = Vec::new();
            rmpv::encode::write_value(&mut theirs, &rmpv::Value::from(value)).unwrap();

            assert_eq!(mine, &theirs[..], "{value} encodes differently than rmpv writes it");
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

    #[test]
    fn bounded_search_finds_and_scores_a_nonce() {
        let block = workblock(&[0x41; 32], 1);
        let (stamp, score) = find(&block, 8, [0; STAMP_LEN], 10_000).unwrap();
        assert!(score >= 8);
        assert_eq!(value(&block, &stamp), score);
    }
}
