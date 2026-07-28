//! LXMF proof-of-work stamps.
//!
//! A stamp is a 32-byte nonce scored by the number of leading zero bits in
//! `SHA256(workblock || stamp)`. The workblock deliberately makes each trial
//! expensive. Propagation stamps derive it from the transient message id with
//! 1,000 expansion rounds.

use hkdf::Hkdf;
use rmpv::Value;
use sha2::{Digest, Sha256};

pub const STAMP_LEN: usize = 32;
pub const WORKBLOCK_BYTES_PER_ROUND: usize = 256;
pub const MESSAGE_WORKBLOCK_ROUNDS: u32 = 3_000;
pub const PROPAGATION_WORKBLOCK_ROUNDS: u32 = 1_000;

/// Derive the stamp workblock observed from stock LXMF.
///
/// Each round uses `SHA256(material || msgpack(round))` as the salt for a
/// 256-byte HKDF-SHA256 expansion of `material`, with empty info.
pub fn workblock(material: &[u8], rounds: u32) -> Vec<u8> {
    let mut block = Vec::with_capacity(rounds as usize * WORKBLOCK_BYTES_PER_ROUND);
    for round in 0..rounds {
        let mut salt_input = Vec::with_capacity(material.len() + 5);
        salt_input.extend_from_slice(material);
        rmpv::encode::write_value(&mut salt_input, &Value::from(u64::from(round)))
            .expect("encoding an integer into memory cannot fail");
        let salt = Sha256::digest(&salt_input);
        let hkdf = Hkdf::<Sha256>::new(Some(&salt), material);
        let start = block.len();
        block.resize(start + WORKBLOCK_BYTES_PER_ROUND, 0);
        hkdf.expand(&[], &mut block[start..])
            .expect("256 bytes is a valid HKDF-SHA256 output length");
    }
    block
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

pub fn propagation_value(transient_id: &[u8; 32], stamp: &[u8; STAMP_LEN]) -> u16 {
    value(
        &workblock(transient_id, PROPAGATION_WORKBLOCK_ROUNDS),
        stamp,
    )
}

pub fn propagation_valid(transient_id: &[u8; 32], stamp: &[u8; STAMP_LEN], target: u16) -> bool {
    target <= 256
        && valid(
            &workblock(transient_id, PROPAGATION_WORKBLOCK_ROUNDS),
            stamp,
            target,
        )
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

    #[test]
    fn bounded_search_finds_and_scores_a_nonce() {
        let block = workblock(&[0x41; 32], 1);
        let (stamp, score) = find(&block, 8, [0; STAMP_LEN], 10_000).unwrap();
        assert!(score >= 8);
        assert_eq!(value(&block, &stamp), score);
    }
}
