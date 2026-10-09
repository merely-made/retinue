//! Structural manifest and part validation.

use super::{
    ESP_FLASH_SECTOR_SIZE, FirmwarePartKind, FlashPackageManifest, FlashRange, FlashRoute,
    NODE_TIMEBASE_PRESERVED_RANGE, PACKAGE_SCHEMA, PERSISTENT_STATE_SCHEMA, PackageError,
    PackagePart, PayloadFormat,
};

pub(super) fn validate_manifest(manifest: &FlashPackageManifest) -> Result<(), PackageError> {
    if manifest.schema != PACKAGE_SCHEMA {
        return Err(PackageError::UnsupportedSchema(manifest.schema));
    }
    if let Some(state) = &manifest.persistent_state {
        if state.schema != PERSISTENT_STATE_SCHEMA {
            return Err(PackageError::InvalidField(
                "unsupported persistent state schema".to_string(),
            ));
        }
        if state.native_node_guard && state.preserved_range != NODE_TIMEBASE_PRESERVED_RANGE {
            return Err(PackageError::InvalidField(
                "native-node guard must preserve the T114 reservation range".to_string(),
            ));
        }
        if state.native_node_guard
            && !manifest
                .preserved_ranges
                .iter()
                .any(|range| fully_covers(range, &state.preserved_range))
        {
            return Err(PackageError::InvalidField(
                "native-node guard range must be covered by preserved_ranges".to_string(),
            ));
        }
    }
    for (name, value) in [
        ("package_id", &manifest.package_id),
        ("display_name", &manifest.display_name),
        ("version", &manifest.version),
        ("publisher", &manifest.publisher),
        ("license", &manifest.license),
        ("notices", &manifest.notices),
        ("source_revision", &manifest.source_revision),
        ("source_url", &manifest.source_url),
        ("origin_url", &manifest.origin_url),
        ("recovery.before_write", &manifest.recovery.before_write),
        ("recovery.after_failure", &manifest.recovery.after_failure),
    ] {
        if value.trim().is_empty() {
            return Err(PackageError::InvalidField(name.to_string()));
        }
    }
    if manifest.payload.is_some() == !manifest.parts.is_empty() {
        return Err(PackageError::InvalidField(
            "exactly one of payload and parts must be present".to_string(),
        ));
    }
    let declared_parts = manifest.declared_parts();
    for part in &declared_parts {
        validate_part(part)?;
    }
    if manifest.payload.is_some() && manifest.write_ranges.is_empty() {
        return Err(PackageError::InvalidField(
            "one-file payload needs write_ranges".to_string(),
        ));
    }
    if !manifest.parts.is_empty() && !manifest.write_ranges.is_empty() {
        return Err(PackageError::InvalidField(
            "sparse parts derive write ranges and cannot also name write_ranges".to_string(),
        ));
    }
    if manifest.targets.is_empty()
        || manifest.helpers.is_empty()
        || manifest.expected_application.version.trim().is_empty()
        || manifest
            .expected_application
            .manual_check
            .as_deref()
            .is_some_and(|instruction| instruction.trim().is_empty())
    {
        return Err(PackageError::InvalidField(
            "targets, helpers, and expected application must not be empty".to_string(),
        ));
    }
    if manifest.expected_application.manual_check.is_none()
        && (manifest.regions.is_empty() || manifest.channel_capabilities.is_empty())
    {
        return Err(PackageError::InvalidField(
            "a status-verified package needs regions and channel capabilities".to_string(),
        ));
    }
    if !manifest
        .targets
        .iter()
        .any(|target| target.family == manifest.expected_application.board)
    {
        return Err(PackageError::InvalidField(
            "expected application board is not one of the package targets".to_string(),
        ));
    }
    let write_ranges = manifest.write_ranges();
    if write_ranges.len() != declared_parts.len() && !manifest.parts.is_empty() {
        return Err(PackageError::InvalidField(
            "every sparse part needs an in-range offset and size".to_string(),
        ));
    }
    for target in &manifest.targets {
        if target.revision.trim().is_empty()
            || target.bootloader.trim().is_empty()
            || target.flash_size == 0
        {
            return Err(PackageError::InvalidField("target".to_string()));
        }
        if write_ranges
            .iter()
            .chain(manifest.preserved_ranges.iter())
            .any(|range| range.end().is_none() || range.end().unwrap() > target.flash_size)
        {
            return Err(PackageError::InvalidField(format!(
                "range outside {} flash",
                target.family
            )));
        }
        validate_parts_for_route(&declared_parts, &target.route)?;
        let helpers = manifest
            .helpers
            .iter()
            .filter(|helper| helper.route == target.route)
            .collect::<Vec<_>>();
        let [helper] = helpers.as_slice() else {
            return Err(PackageError::InvalidField(format!(
                "package needs exactly one helper for {}",
                target.route
            )));
        };
        if helper.program.trim().is_empty()
            || helper.version.trim().is_empty()
            || helper.license.trim().is_empty()
            || helper.source_url.trim().is_empty()
            || helper.notice.trim().is_empty()
            || helper
                .binary_sha256
                .as_deref()
                .is_some_and(|digest| !is_sha256(digest))
            || (!helper.artifacts.is_empty() && helper.binary_sha256.is_some())
            || helper.artifacts.iter().any(|artifact| {
                artifact.platform.trim().is_empty()
                    || !is_sha256(&artifact.binary_sha256)
                    || !is_sha256(&artifact.archive_sha256)
                    || !artifact.archive_url.starts_with("https://")
            })
            || {
                let mut platforms = helper
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.platform.as_str())
                    .collect::<Vec<_>>();
                platforms.sort_unstable();
                platforms.windows(2).any(|pair| pair[0] == pair[1])
            }
        {
            return Err(PackageError::InvalidField(format!(
                "invalid helper metadata for {}",
                target.route
            )));
        }
    }
    if has_overlap(&write_ranges)
        || has_overlap(&manifest.preserved_ranges)
        || write_ranges.iter().any(|write| {
            manifest
                .preserved_ranges
                .iter()
                .any(|preserved| write.overlaps(preserved))
        })
    {
        return Err(PackageError::ProtectedRangeOverlap);
    }
    if !manifest.parts.is_empty()
        && write_ranges.iter().filter_map(erase_span).any(|span| {
            manifest
                .preserved_ranges
                .iter()
                .any(|preserved| span.overlaps(preserved))
        })
    {
        return Err(PackageError::ProtectedRangeOverlap);
    }
    if let Some(signature) = &manifest.publisher_signature
        && (signature.key_id.trim().is_empty()
            || !signature.signed_manifest_url.starts_with("https://")
            || !is_sha256(&signature.signed_manifest_sha256)
            || signature.signature.trim().is_empty())
    {
        return Err(PackageError::InvalidField(
            "publisher_signature".to_string(),
        ));
    }
    Ok(())
}

fn validate_part(part: &PackagePart) -> Result<(), PackageError> {
    if part.path.trim().is_empty()
        || part.byte_length == 0
        || part.write_bytes == 0
        || part.write_bytes > part.byte_length
        || !is_sha256(&part.sha256)
    {
        return Err(PackageError::InvalidField("package part".to_string()));
    }
    Ok(())
}

fn validate_parts_for_route(parts: &[PackagePart], route: &FlashRoute) -> Result<(), PackageError> {
    let container = matches!(
        (route, parts),
        (
            FlashRoute::AdafruitDfu,
            [PackagePart {
                kind: FirmwarePartKind::Application,
                format: PayloadFormat::NrfDfuZip,
                offset: None,
                ..
            }]
        ) | (
            FlashRoute::EspRom,
            [PackagePart {
                kind: FirmwarePartKind::Application,
                format: PayloadFormat::EspflashElf,
                offset: None,
                ..
            }]
        ) | (
            FlashRoute::Uf2MassStorage,
            [PackagePart {
                kind: FirmwarePartKind::Application,
                format: PayloadFormat::Uf2,
                offset: None,
                ..
            }]
        )
    );
    let sparse_esp = matches!(
        (route, parts),
        (
            FlashRoute::EspRom,
            [
                PackagePart {
                    kind: FirmwarePartKind::Bootloader,
                    format: PayloadFormat::RawBinary,
                    offset: Some(_),
                    ..
                },
                PackagePart {
                    kind: FirmwarePartKind::PartitionTable,
                    format: PayloadFormat::RawBinary,
                    offset: Some(_),
                    ..
                },
                PackagePart {
                    kind: FirmwarePartKind::Application,
                    format: PayloadFormat::RawBinary,
                    offset: Some(_),
                    ..
                }
            ]
        )
    );
    if !container && !sparse_esp {
        return Err(PackageError::InvalidField(format!(
            "parts do not form a supported {} package",
            route
        )));
    }
    if sparse_esp
        && parts.iter().any(|part| {
            part.offset
                .is_some_and(|offset| offset % ESP_FLASH_SECTOR_SIZE != 0)
        })
    {
        return Err(PackageError::InvalidField(
            "sparse ESP part offset is not erase-sector aligned".to_string(),
        ));
    }
    let mut paths = parts.iter().map(|part| &part.path).collect::<Vec<_>>();
    paths.sort();
    if paths.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(PackageError::InvalidField(
            "package parts cannot repeat a path".to_string(),
        ));
    }
    Ok(())
}

fn erase_span(range: &FlashRange) -> Option<FlashRange> {
    let end = range.end()?;
    let rounded_end =
        end.checked_add(ESP_FLASH_SECTOR_SIZE - 1)? / ESP_FLASH_SECTOR_SIZE * ESP_FLASH_SECTOR_SIZE;
    Some(FlashRange {
        start: range.start,
        length: rounded_end.checked_sub(range.start)?,
    })
}

fn has_overlap(ranges: &[FlashRange]) -> bool {
    ranges.iter().enumerate().any(|(index, range)| {
        ranges
            .iter()
            .skip(index + 1)
            .any(|other| range.overlaps(other))
    })
}

fn fully_covers(container: &FlashRange, required: &FlashRange) -> bool {
    match (container.end(), required.end()) {
        (Some(container_end), Some(required_end)) => {
            container.start <= required.start && container_end >= required_end
        }
        _ => false,
    }
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
