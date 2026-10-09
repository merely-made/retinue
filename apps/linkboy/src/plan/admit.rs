use crate::device::{DeviceObservation, FirmwareState, NativeNodeState};
use crate::package::{FlashPackage, FlashRange, FlashRoute, StateImpact};

use super::{
    CompatibilityFact, FlashPlan, HelperIdentity, PackageIdentity, PackagePartIdentity,
    PlanWarning, Refusal, RefusalReason,
};

/// Pure decision function. It does not open a port, read a volume, inspect PATH, or mutate the
/// package. All values needed for a decision are already in its arguments.
pub fn plan_flash(
    observation: &DeviceObservation,
    package: &FlashPackage,
) -> Result<FlashPlan, Refusal> {
    let manifest = package.manifest();
    let mut refusals = observation
        .contradictions
        .iter()
        .cloned()
        .map(RefusalReason::ContradictoryEvidence)
        .collect::<Vec<_>>();

    let Some(board) = observation.selected_board.as_ref() else {
        refusals.push(RefusalReason::BoardSelectionRequired);
        return Err(Refusal::new(refusals));
    };
    if !board.confirmed_by_owner {
        refusals.push(RefusalReason::BoardConfirmationRequired);
    }
    if let FirmwareState::Retinue { family: observed } = &observation.firmware
        && observed != &board.family
    {
        refusals.push(RefusalReason::RunningBoardConflict {
            selected: board.family.clone(),
            observed: observed.clone(),
        });
    }

    let Some(target) = manifest
        .targets
        .iter()
        .find(|target| target.family == board.family && target.revision == board.revision)
    else {
        if !manifest
            .targets
            .iter()
            .any(|target| target.family == board.family)
        {
            refusals.push(RefusalReason::UnsupportedBoard(board.family.clone()));
        } else {
            refusals.push(RefusalReason::UnsupportedRevision {
                family: board.family.clone(),
                revision: board.revision.clone(),
            });
        }
        return Err(Refusal::new(refusals));
    };

    if observation.native_node_state == NativeNodeState::Armed
        && !manifest
            .persistent_state
            .as_ref()
            .is_some_and(|state| state.supports_native_node_guard())
    {
        refusals.push(RefusalReason::PersistentStateCompatibilityRequired);
    }

    let hardware = &observation.hardware;
    let running_identity_is_authoritative = matches!(
        (&observation.firmware, &observation.status_reply),
        (FirmwareState::Retinue { family }, Some(_)) if family == &board.family
    );
    match &hardware.processor {
        Some(observed) if observed != &target.processor => {
            refusals.push(RefusalReason::ProcessorConflict {
                observed: observed.clone(),
                required: target.processor.clone(),
            })
        }
        None if !running_identity_is_authoritative => {
            refusals.push(RefusalReason::ProcessorMissing)
        }
        Some(_) => {}
        None => {}
    }
    match hardware.flash_size {
        Some(observed) if observed != target.flash_size => {
            refusals.push(RefusalReason::FlashSizeConflict {
                observed,
                required: target.flash_size,
            })
        }
        None if !running_identity_is_authoritative => {
            refusals.push(RefusalReason::FlashSizeMissing)
        }
        Some(_) => {}
        None => {}
    }
    match &hardware.bootloader {
        Some(observed) if observed != &target.bootloader => {
            refusals.push(RefusalReason::BootloaderConflict {
                observed: observed.clone(),
                required: target.bootloader.clone(),
            })
        }
        None if !running_identity_is_authoritative => {
            refusals.push(RefusalReason::BootloaderMissing)
        }
        Some(_) => {}
        None => {}
    }
    let write_ranges = manifest.write_ranges();
    if has_protected_overlap(&write_ranges, &manifest.preserved_ranges) {
        refusals.push(RefusalReason::ProtectedRangeOverlap);
    }
    for range in &write_ranges {
        match range.end() {
            Some(end) if end <= target.flash_size => {}
            Some(end) => refusals.push(RefusalReason::RangeOutsideFlash {
                start: range.start,
                end,
                flash_size: target.flash_size,
            }),
            None => refusals.push(RefusalReason::RangeOutsideFlash {
                start: range.start,
                end: u32::MAX,
                flash_size: target.flash_size,
            }),
        }
    }
    if manifest.recovery.before_write.trim().is_empty()
        || manifest.recovery.after_failure.trim().is_empty()
    {
        refusals.push(RefusalReason::RecoveryMissing);
    }
    let helper = manifest
        .helper_for(&target.route)
        .expect("validated package target has exactly one helper");
    let helper_artifact = helper.artifact_for_current_platform();
    if !target.route.uses_builtin_writer()
        && !helper.artifacts.is_empty()
        && helper_artifact.is_none()
    {
        refusals.push(RefusalReason::HelperPlatformUnsupported {
            program: helper.program.clone(),
            platform: crate::package::helper_platform(),
        });
    }
    if !refusals.is_empty() {
        return Err(Refusal::new(refusals));
    }

    let mut warnings = Vec::new();
    if manifest.state_impact == StateImpact::Unknown {
        warnings.push(PlanWarning {
            message: "persistent identity and settings impact is unknown; owner confirmation is required before writing".into(),
            requires_confirmation: true,
        });
    }
    let fact_source = if running_identity_is_authoritative {
        "running Retinue identity; checked against package"
    } else if target.route == FlashRoute::Uf2MassStorage {
        "owner selection, checked against UF2 bootloader record"
    } else if hardware.loader_route.as_deref() == Some("captured-t114-loader-snapshot") {
        "captured HT-n5262 UF2 and SoftDevice record"
    } else {
        "supported loader"
    };
    let family_source = if running_identity_is_authoritative {
        "owner selection, checked against running status"
    } else if hardware.loader_route.as_deref() == Some("captured-t114-loader-snapshot") {
        "owner-selected current board, checked against captured HT-n5262 loader record"
    } else {
        "owner selection"
    };
    let compatibility = vec![
        CompatibilityFact {
            name: "board family".into(),
            value: board.family.to_string(),
            source: family_source.into(),
        },
        CompatibilityFact {
            name: "board revision".into(),
            value: board.revision.clone(),
            source: board.evidence.describe(),
        },
        CompatibilityFact {
            name: "processor".into(),
            value: target.processor.to_string(),
            source: fact_source.into(),
        },
        CompatibilityFact {
            name: "flash size".into(),
            value: format!("{} bytes", target.flash_size),
            source: fact_source.into(),
        },
        CompatibilityFact {
            name: "bootloader".into(),
            value: target.bootloader.clone(),
            source: fact_source.into(),
        },
        CompatibilityFact {
            name: "native-node persistent state".into(),
            value: observation.native_node_state.describe().into(),
            source: match observation.native_node_state {
                NativeNodeState::Armed => "running status token state=node-timebase-v1",
                NativeNodeState::Unarmed => "running status token state=node-unarmed",
                NativeNodeState::Unknown => {
                    "running status; absent or non-guard token remains non-authoritative"
                }
            }
            .into(),
        },
        CompatibilityFact {
            name: "package node-timebase support".into(),
            value: manifest
                .persistent_state
                .as_ref()
                .filter(|state| state.supports_native_node_guard())
                .map(|state| {
                    format!(
                        "schema {}; preserves {:#x}..{:#x}",
                        state.schema,
                        state.preserved_range.start,
                        state.preserved_range.end().unwrap_or(u32::MAX)
                    )
                })
                .unwrap_or_else(|| "not declared".into()),
            source: "package manifest".into(),
        },
    ];
    Ok(FlashPlan {
        observation: observation.clone(),
        package: PackageIdentity {
            package_id: manifest.package_id.clone(),
            display_name: manifest.display_name.clone(),
            version: manifest.version.clone(),
            parts: package
                .parts()
                .iter()
                .map(|part| PackagePartIdentity {
                    kind: part.declaration().kind.clone(),
                    offset: part.declaration().offset,
                    byte_length: part.declaration().byte_length,
                    sha256: part.declaration().sha256.clone(),
                })
                .collect(),
            publisher_signature: manifest.publisher_signature.clone(),
        },
        board: board.clone(),
        route: target.route.clone(),
        helper: HelperIdentity {
            program: helper.program.clone(),
            version: helper.version.clone(),
            platform: helper_artifact.map(|artifact| artifact.platform.clone()),
            binary_sha256: helper.expected_binary_sha256().map(ToOwned::to_owned),
            archive_sha256: helper_artifact.map(|artifact| artifact.archive_sha256.clone()),
            archive_url: helper_artifact.map(|artifact| artifact.archive_url.clone()),
        },
        write_ranges,
        preserved_ranges: manifest.preserved_ranges.clone(),
        state_impact: manifest.state_impact.clone(),
        compatibility,
        warnings,
        recovery_before_write: manifest.recovery.before_write.clone(),
        recovery_after_failure: manifest.recovery.after_failure.clone(),
    })
}

fn has_protected_overlap(writes: &[FlashRange], preserved: &[FlashRange]) -> bool {
    writes
        .iter()
        .any(|write| preserved.iter().any(|range| write.overlaps(range)))
}
