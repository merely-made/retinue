use super::{
    FlashPackageManifest, FlashRange, FlashRoute, PackageError, ProcessorKind, VerifiedPackagePart,
};

const UF2_BLOCK_SIZE: usize = 512;
const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_ID: u32 = 0x0000_2000;

/// UF2 carries target addresses inside its fixed-size blocks. Checking them here keeps a
/// one-file package's declared write ranges real rather than an optimistic side note beside
/// an opaque blob.
pub(super) fn validate_uf2_layout(
    manifest: &FlashPackageManifest,
    parts: &[VerifiedPackagePart],
) -> Result<(), PackageError> {
    let is_uf2 = manifest
        .targets
        .iter()
        .any(|target| target.route == FlashRoute::Uf2MassStorage);
    if !is_uf2 {
        return Ok(());
    }
    let [part] = parts else {
        return Err(PackageError::InvalidField(
            "UF2 package needs exactly one verified part".into(),
        ));
    };
    let bytes = part.bytes();
    if bytes.is_empty() || bytes.len() % UF2_BLOCK_SIZE != 0 {
        return Err(PackageError::InvalidField(
            "UF2 payload is not a non-empty sequence of 512-byte blocks".into(),
        ));
    }

    let mut blocks = Vec::with_capacity(bytes.len() / UF2_BLOCK_SIZE);
    let mut seen_block_numbers = vec![false; bytes.len() / UF2_BLOCK_SIZE];
    let mut total_payload = 0_u64;
    for (index, block) in bytes.as_chunks::<UF2_BLOCK_SIZE>().0.iter().enumerate() {
        let word = |offset| u32::from_le_bytes(block[offset..offset + 4].try_into().unwrap());
        if word(0) != UF2_MAGIC_START0 || word(4) != UF2_MAGIC_START1 || word(508) != UF2_MAGIC_END
        {
            return Err(PackageError::InvalidField(format!(
                "UF2 block {index} has invalid magic"
            )));
        }
        let address = word(12);
        let payload_size = word(16);
        let block_number = word(20);
        let block_total = word(24);
        let flags = word(8);
        let family_id = word(28);
        let nrf52840_target = manifest
            .targets
            .iter()
            .any(|target| target.processor == ProcessorKind::Nrf52840);
        if nrf52840_target
            && (flags & UF2_FLAG_FAMILY_ID == 0 || family_id != crate::uf2::NRF52840_FAMILY_ID)
        {
            return Err(PackageError::InvalidField(format!(
                "UF2 block {index} does not carry the nRF52840 family id"
            )));
        }
        if payload_size == 0 || payload_size > 476 {
            return Err(PackageError::InvalidField(format!(
                "UF2 block {index} has invalid payload size {payload_size}"
            )));
        }
        if block_total as usize != seen_block_numbers.len()
            || block_number as usize >= seen_block_numbers.len()
            || seen_block_numbers[block_number as usize]
        {
            return Err(PackageError::InvalidField(format!(
                "UF2 block {index} has an inconsistent block number or count"
            )));
        }
        seen_block_numbers[block_number as usize] = true;
        let end = address.checked_add(payload_size).ok_or_else(|| {
            PackageError::InvalidField(format!("UF2 block {index} address overflows"))
        })?;
        total_payload += u64::from(payload_size);
        blocks.push(FlashRange {
            start: address,
            length: end - address,
        });
    }
    if seen_block_numbers.iter().any(|seen| !seen) {
        return Err(PackageError::InvalidField(
            "UF2 block numbers are not a complete sequence".into(),
        ));
    }
    if total_payload != part.declaration().write_bytes {
        return Err(PackageError::InvalidField(format!(
            "UF2 write_bytes is {}, but blocks carry {total_payload}",
            part.declaration().write_bytes
        )));
    }

    blocks.sort_by_key(|range| range.start);
    let mut merged = Vec::<FlashRange>::new();
    for block in blocks {
        if let Some(previous) = merged.last_mut() {
            if previous.end() == Some(block.start) {
                previous.length += block.length;
                continue;
            }
            if previous.overlaps(&block) {
                return Err(PackageError::InvalidField(
                    "UF2 target blocks overlap".into(),
                ));
            }
        }
        merged.push(block);
    }
    if merged != manifest.write_ranges {
        return Err(PackageError::InvalidField(
            "UF2 target ranges do not match package write_ranges".into(),
        ));
    }
    Ok(())
}
