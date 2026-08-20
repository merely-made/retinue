//! Reproducible UF2 application packaging.
//!
//! The public T114 route writes an application-only UF2 through the board's stock
//! mass-storage bootloader. Keeping the encoder here makes the release artifact
//! reproducible without making an owner install Python or `adafruit-nrfutil`.

use thiserror::Error;

pub const BLOCK_SIZE: usize = 512;
pub const PAYLOAD_SIZE: usize = 256;
pub const NRF52840_FAMILY_ID: u32 = 0xADA5_2840;

const MAGIC_START0: u32 = 0x0A32_4655;
const MAGIC_START1: u32 = 0x9E5D_5157;
const MAGIC_END: u32 = 0x0AB1_6F30;
const FLAG_FAMILY_ID: u32 = 0x0000_2000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum Uf2EncodeError {
    #[error("UF2 input application is empty")]
    Empty,
    #[error("UF2 application address range overflows 32-bit target flash")]
    AddressOverflow,
    #[error("UF2 application needs too many blocks")]
    TooManyBlocks,
}

/// Encode a raw application binary into deterministic 256-byte UF2 payload blocks.
///
/// The final block is zero-padded, matching Adafruit's `uf2conv.py`. Every block
/// carries the target family ID so a non-matching bootloader ignores the file.
pub fn encode_application(
    application: &[u8],
    base_address: u32,
    family_id: u32,
) -> Result<Vec<u8>, Uf2EncodeError> {
    if application.is_empty() {
        return Err(Uf2EncodeError::Empty);
    }
    let padded_length = application
        .len()
        .checked_add(PAYLOAD_SIZE - 1)
        .ok_or(Uf2EncodeError::TooManyBlocks)?
        / PAYLOAD_SIZE
        * PAYLOAD_SIZE;
    let _ = base_address
        .checked_add(u32::try_from(padded_length).map_err(|_| Uf2EncodeError::AddressOverflow)?)
        .ok_or(Uf2EncodeError::AddressOverflow)?;
    let block_count = padded_length / PAYLOAD_SIZE;
    let block_count_u32 = u32::try_from(block_count).map_err(|_| Uf2EncodeError::TooManyBlocks)?;

    let mut output = vec![0_u8; block_count * BLOCK_SIZE];
    for block_number in 0..block_count {
        let block = &mut output[block_number * BLOCK_SIZE..(block_number + 1) * BLOCK_SIZE];
        put_word(block, 0, MAGIC_START0);
        put_word(block, 4, MAGIC_START1);
        put_word(block, 8, FLAG_FAMILY_ID);
        let address = base_address
            + u32::try_from(block_number * PAYLOAD_SIZE)
                .map_err(|_| Uf2EncodeError::AddressOverflow)?;
        put_word(block, 12, address);
        put_word(block, 16, PAYLOAD_SIZE as u32);
        put_word(block, 20, block_number as u32);
        put_word(block, 24, block_count_u32);
        put_word(block, 28, family_id);
        let source_start = block_number * PAYLOAD_SIZE;
        let source_end = (source_start + PAYLOAD_SIZE).min(application.len());
        block[32..32 + source_end - source_start]
            .copy_from_slice(&application[source_start..source_end]);
        put_word(block, 508, MAGIC_END);
    }
    Ok(output)
}

fn put_word(block: &mut [u8], offset: usize, value: u32) {
    block[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(block: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn application_encoding_matches_the_stock_nrf52840_contract() {
        let input = (0..300).map(|byte| byte as u8).collect::<Vec<_>>();
        let encoded = encode_application(&input, 0x26000, NRF52840_FAMILY_ID).unwrap();
        assert_eq!(encoded.len(), 2 * BLOCK_SIZE);
        for (index, block) in encoded.chunks_exact(BLOCK_SIZE).enumerate() {
            assert_eq!(word(block, 0), MAGIC_START0);
            assert_eq!(word(block, 4), MAGIC_START1);
            assert_eq!(word(block, 8), FLAG_FAMILY_ID);
            assert_eq!(word(block, 12), 0x26000 + index as u32 * 256);
            assert_eq!(word(block, 16), 256);
            assert_eq!(word(block, 20), index as u32);
            assert_eq!(word(block, 24), 2);
            assert_eq!(word(block, 28), NRF52840_FAMILY_ID);
            assert_eq!(word(block, 508), MAGIC_END);
        }
        assert_eq!(&encoded[32..288], &input[..256]);
        assert_eq!(&encoded[BLOCK_SIZE + 32..BLOCK_SIZE + 76], &input[256..]);
        assert!(
            encoded[BLOCK_SIZE + 76..BLOCK_SIZE + 288]
                .iter()
                .all(|byte| *byte == 0)
        );
    }

    #[test]
    fn empty_application_is_refused() {
        assert_eq!(
            encode_application(&[], 0x26000, NRF52840_FAMILY_ID),
            Err(Uf2EncodeError::Empty)
        );
    }
}
