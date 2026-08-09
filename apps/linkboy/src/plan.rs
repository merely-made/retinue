//! Pure package/device compatibility and the immutable write plan.

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device::{BoardSelection, DeviceObservation, FirmwareState};
use crate::package::{
    BoardFamily, FlashPackage, FlashRange, FlashRoute, ProcessorKind, StateImpact,
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompatibilityFact {
    pub name: String,
    pub value: String,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanWarning {
    pub message: String,
    pub requires_confirmation: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageIdentity {
    pub package_id: String,
    pub display_name: String,
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashPlan {
    observation: DeviceObservation,
    package: PackageIdentity,
    board: BoardSelection,
    route: FlashRoute,
    helper: String,
    write_ranges: Vec<FlashRange>,
    preserved_ranges: Vec<FlashRange>,
    state_impact: StateImpact,
    compatibility: Vec<CompatibilityFact>,
    warnings: Vec<PlanWarning>,
    recovery_before_write: String,
    recovery_after_failure: String,
}

impl FlashPlan {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub fn for_test(
        observation: DeviceObservation,
        package: PackageIdentity,
        board: BoardSelection,
        route: FlashRoute,
        write_ranges: Vec<FlashRange>,
        preserved_ranges: Vec<FlashRange>,
        state_impact: StateImpact,
        compatibility: Vec<CompatibilityFact>,
        warnings: Vec<PlanWarning>,
        recovery_before_write: String,
        recovery_after_failure: String,
    ) -> Self {
        Self {
            observation,
            package,
            board,
            helper: route.helper().into(),
            route,
            write_ranges,
            preserved_ranges,
            state_impact,
            compatibility,
            warnings,
            recovery_before_write,
            recovery_after_failure,
        }
    }

    pub fn observation(&self) -> &DeviceObservation {
        &self.observation
    }

    pub fn package(&self) -> &PackageIdentity {
        &self.package
    }

    pub fn board(&self) -> &BoardSelection {
        &self.board
    }

    pub fn route(&self) -> &FlashRoute {
        &self.route
    }

    pub fn helper(&self) -> &str {
        &self.helper
    }

    pub fn write_ranges(&self) -> &[FlashRange] {
        &self.write_ranges
    }

    pub fn preserved_ranges(&self) -> &[FlashRange] {
        &self.preserved_ranges
    }

    pub fn state_impact(&self) -> &StateImpact {
        &self.state_impact
    }

    pub fn compatibility(&self) -> &[CompatibilityFact] {
        &self.compatibility
    }

    pub fn warnings(&self) -> &[PlanWarning] {
        &self.warnings
    }

    pub fn recovery_before_write(&self) -> &str {
        &self.recovery_before_write
    }

    pub fn recovery_after_failure(&self) -> &str {
        &self.recovery_after_failure
    }

    pub fn describe(&self) -> String {
        let device = match &self.observation.transport {
            crate::device::DeviceTransport::SerialPort(port) => port.as_str(),
            crate::device::DeviceTransport::MountedVolume(volume) => volume.as_str(),
        };
        let facts = self
            .compatibility
            .iter()
            .map(|fact| format!("    {}: {} [{}]", fact.name, fact.value, fact.source))
            .collect::<Vec<_>>()
            .join("\n");
        let warnings = if self.warnings.is_empty() {
            "    none".to_string()
        } else {
            self.warnings
                .iter()
                .map(|warning| format!("    {}", warning.message))
                .collect::<Vec<_>>()
                .join("\n")
        };
        format!(
            "flash plan\n  device: {device}\n  board: {} revision {}\n  package: {} {}\n  payload sha256: {}\n  route: {}\n  helper: {}\n  write ranges: {}\n  preserved ranges: {}\n  state impact: {}\n  compatibility:\n{facts}\n  warnings:\n{warnings}\n  recovery before write: {}\n  recovery after failure: {}",
            self.board.family,
            self.board.revision,
            self.package.display_name,
            self.package.version,
            self.package.sha256,
            self.route,
            self.helper,
            describe_ranges(&self.write_ranges),
            describe_ranges(&self.preserved_ranges),
            self.state_impact,
            self.recovery_before_write,
            self.recovery_after_failure,
        )
    }
}

fn describe_ranges(ranges: &[FlashRange]) -> String {
    ranges
        .iter()
        .map(|range| match range.end() {
            Some(end) => format!("{:#x}..{:#x}", range.start, end),
            None => format!("{:#x}..overflow", range.start),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Every reason is structured so a CLI and a future graphical face can render the same refusal.
#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum RefusalReason {
    #[error("the exact board revision was not selected by the owner")]
    BoardSelectionRequired,
    #[error("the selected board is not owner-confirmed")]
    BoardConfirmationRequired,
    #[error("selected board family {selected} conflicts with running firmware family {observed}")]
    RunningBoardConflict {
        selected: BoardFamily,
        observed: BoardFamily,
    },
    #[error("package does not support board family {0}")]
    UnsupportedBoard(BoardFamily),
    #[error("package does not support board revision {revision} for {family}")]
    UnsupportedRevision {
        family: BoardFamily,
        revision: String,
    },
    #[error("loader reported processor {observed}, package requires {required}")]
    ProcessorConflict {
        observed: ProcessorKind,
        required: ProcessorKind,
    },
    #[error("processor fact is missing")]
    ProcessorMissing,
    #[error("loader reported {observed} bytes of flash, package requires {required}")]
    FlashSizeConflict { observed: u32, required: u32 },
    #[error("flash-size fact is missing")]
    FlashSizeMissing,
    #[error("loader reported bootloader {observed}, package requires {required}")]
    BootloaderConflict { observed: String, required: String },
    #[error("bootloader fact is missing")]
    BootloaderMissing,
    #[error("contradictory device evidence: {0}")]
    ContradictoryEvidence(String),
    #[error("write range {start:#x}..{end:#x} exceeds {flash_size:#x} bytes of target flash")]
    RangeOutsideFlash {
        start: u32,
        end: u32,
        flash_size: u32,
    },
    #[error("write range overlaps a preserved range")]
    ProtectedRangeOverlap,
    #[error("package does not provide complete recovery instructions")]
    RecoveryMissing,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error(
    "refused:\n{}",
    .reasons.iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
)]
pub struct Refusal {
    pub reasons: Vec<RefusalReason>,
}

impl Refusal {
    fn new(reasons: Vec<RefusalReason>) -> Self {
        debug_assert!(!reasons.is_empty());
        Self { reasons }
    }
}

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
    if let FirmwareState::Retinue { family: observed } = &observation.firmware {
        if observed != &board.family {
            refusals.push(RefusalReason::RunningBoardConflict {
                selected: board.family.clone(),
                observed: observed.clone(),
            });
        }
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
    if has_protected_overlap(&manifest.write_ranges, &manifest.preserved_ranges) {
        refusals.push(RefusalReason::ProtectedRangeOverlap);
    }
    for range in &manifest.write_ranges {
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
    } else {
        "supported loader"
    };
    let compatibility = vec![
        CompatibilityFact {
            name: "board family".into(),
            value: board.family.to_string(),
            source: "owner selection, checked against running status".into(),
        },
        CompatibilityFact {
            name: "board revision".into(),
            value: board.revision.clone(),
            source: "owner confirmation".into(),
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
    ];
    Ok(FlashPlan {
        observation: observation.clone(),
        package: PackageIdentity {
            package_id: manifest.package_id.clone(),
            display_name: manifest.display_name.clone(),
            version: manifest.version.clone(),
            sha256: manifest.payload.sha256.clone(),
        },
        board: board.clone(),
        route: target.route.clone(),
        helper: target.route.helper().into(),
        write_ranges: manifest.write_ranges.clone(),
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

impl fmt::Display for PackageIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.display_name, self.version)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceTransport, HardwareFacts};
    use crate::package::{
        ExpectedApplication, FlashPackageManifest, PACKAGE_SCHEMA, PackagePayload, PackageTarget,
        PayloadFormat, RecoveryInstructions,
    };

    fn package() -> FlashPackage {
        let bytes = b"package-bytes".to_vec();
        let manifest = FlashPackageManifest {
            schema: PACKAGE_SCHEMA,
            package_id: "test.v4".into(),
            display_name: "Test V4".into(),
            version: "1".into(),
            publisher: "Test".into(),
            helpers: vec![crate::package::HelperRequirement {
                route: FlashRoute::EspRom,
                program: "espflash".into(),
                version: "4.5.0".into(),
                license: "MIT OR Apache-2.0".into(),
                source_url: "https://example.invalid/espflash".into(),
                notice: "Test helper notice".into(),
            }],
            payload: PackagePayload {
                path: "payload".into(),
                format: PayloadFormat::EspflashElf,
                byte_length: bytes.len() as u64,
                sha256: crate::package::sha256_hex(&bytes),
                write_bytes: bytes.len() as u64,
            },
            targets: vec![PackageTarget {
                family: BoardFamily::HeltecV4,
                revision: "4.2".into(),
                processor: ProcessorKind::Esp32S3,
                flash_size: 4 * 1024 * 1024,
                bootloader: "esp-rom".into(),
                route: FlashRoute::EspRom,
            }],
            write_ranges: vec![FlashRange {
                start: 0,
                length: 0x3f0000,
            }],
            preserved_ranges: vec![FlashRange {
                start: 0x3f0000,
                length: 0x10000,
            }],
            regions: vec!["US915".into()],
            channel_capabilities: vec!["modem".into(), "rnode".into()],
            state_impact: StateImpact::Preserved,
            expected_application: ExpectedApplication {
                board: BoardFamily::HeltecV4,
                version: "0.0.1".into(),
            },
            license: "MPL-2.0".into(),
            notices: "Notices".into(),
            source_revision: "test".into(),
            source_url: "https://example.invalid/source".into(),
            origin_url: "https://example.invalid/package".into(),
            publisher_signature: None,
            recovery: RecoveryInstructions {
                before_write: "Keep cable attached.".into(),
                after_failure: "Use ROM entry.".into(),
            },
        };
        FlashPackage::from_parts(manifest, "manifest", "payload", bytes).unwrap()
    }

    fn t114_package() -> FlashPackage {
        let bytes = b"package-t114".to_vec();
        let manifest = FlashPackageManifest {
            schema: PACKAGE_SCHEMA,
            package_id: "test.t114".into(),
            display_name: "Test T114".into(),
            version: "1".into(),
            publisher: "Test".into(),
            helpers: vec![crate::package::HelperRequirement {
                route: FlashRoute::AdafruitDfu,
                program: "adafruit-nrfutil".into(),
                version: "0.5.3.post16".into(),
                license: "test".into(),
                source_url: "https://example.invalid/adafruit-nrfutil".into(),
                notice: "Test helper notice".into(),
            }],
            payload: PackagePayload {
                path: "payload".into(),
                format: PayloadFormat::NrfDfuZip,
                byte_length: bytes.len() as u64,
                sha256: crate::package::sha256_hex(&bytes),
                write_bytes: bytes.len() as u64,
            },
            targets: vec![PackageTarget {
                family: BoardFamily::T114,
                revision: "2.x".into(),
                processor: ProcessorKind::Nrf52840,
                flash_size: 1024 * 1024,
                bootloader: "s140-v6".into(),
                route: FlashRoute::AdafruitDfu,
            }],
            write_ranges: vec![FlashRange {
                start: 0x26000,
                length: bytes.len() as u32,
            }],
            preserved_ranges: vec![FlashRange {
                start: 0x26000 + bytes.len() as u32,
                length: 1,
            }],
            regions: vec!["US915".into()],
            channel_capabilities: vec!["modem".into(), "node".into(), "rnode".into()],
            state_impact: StateImpact::Preserved,
            expected_application: ExpectedApplication {
                board: BoardFamily::T114,
                version: "0.0.1".into(),
            },
            license: "MPL-2.0".into(),
            notices: "Notices".into(),
            source_revision: "test".into(),
            source_url: "https://example.invalid/source".into(),
            origin_url: "https://example.invalid/package".into(),
            publisher_signature: None,
            recovery: RecoveryInstructions {
                before_write: "Keep cable attached.".into(),
                after_failure: "Use DFU entry.".into(),
            },
        };
        FlashPackage::from_parts(manifest, "manifest", "payload", bytes).unwrap()
    }

    fn observation() -> DeviceObservation {
        DeviceObservation {
            transport: DeviceTransport::SerialPort("COM7".into()),
            status_reply: Some("tulle/heltec-v4 phy online".into()),
            hardware: HardwareFacts {
                processor: Some(ProcessorKind::Esp32S3),
                flash_size: Some(4 * 1024 * 1024),
                bootloader: Some("esp-rom".into()),
                loader_route: Some("esp-rom".into()),
                bootloader_usb: None,
            },
            selected_board: Some(BoardSelection::owner_confirmed(
                BoardFamily::HeltecV4,
                "4.2",
            )),
            firmware: FirmwareState::Retinue {
                family: BoardFamily::HeltecV4,
            },
            confidence: crate::device::EvidenceConfidence::OwnerConfirmed,
            contradictions: Vec::new(),
        }
    }

    #[test]
    fn compatible_observation_produces_an_explainable_plan() {
        let plan = plan_flash(&observation(), &package()).expect("facts are compatible");
        assert_eq!(plan.route(), &FlashRoute::EspRom);
        assert_eq!(plan.helper(), "espflash");
        assert_eq!(plan.state_impact(), &StateImpact::Preserved);
        assert!(plan.describe().contains("recovery before write"));
    }

    #[test]
    fn running_retinue_identity_can_plan_without_reopening_the_loader() {
        let mut observation = observation();
        observation.hardware = HardwareFacts::default();
        let plan = plan_flash(&observation, &package())
            .expect("a running, self-identified Retinue board has a known route");
        assert!(plan.compatibility().iter().all(|fact| {
            fact.name == "board family"
                || fact.name == "board revision"
                || fact.source == "running Retinue identity; checked against package"
        }));
    }

    #[test]
    fn v4_package_is_refused_for_t114() {
        let mut observation = observation();
        observation.selected_board =
            Some(BoardSelection::owner_confirmed(BoardFamily::T114, "2.1"));
        observation.firmware = FirmwareState::Retinue {
            family: BoardFamily::T114,
        };
        let refusal = plan_flash(&observation, &package()).expect_err("wrong board must refuse");
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::UnsupportedBoard(BoardFamily::T114)))
        );
    }

    #[test]
    fn t114_package_is_refused_for_v4() {
        let refusal = plan_flash(&observation(), &t114_package())
            .expect_err("the T114 package must not plan for a V4");
        assert!(refusal.reasons.iter().any(|reason| matches!(
            reason,
            RefusalReason::UnsupportedBoard(BoardFamily::HeltecV4)
        )));
    }

    #[test]
    fn conflicting_loader_evidence_is_refused() {
        let mut observation = observation();
        observation.hardware.processor = Some(ProcessorKind::Nrf52840);
        let refusal =
            plan_flash(&observation, &package()).expect_err("processor conflict must refuse");
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::ProcessorConflict { .. }))
        );
    }

    #[test]
    fn revision_and_confirmation_are_not_guessed() {
        let mut observation = observation();
        observation.selected_board = None;
        let refusal =
            plan_flash(&observation, &package()).expect_err("missing owner choice must refuse");
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::BoardSelectionRequired))
        );

        observation.selected_board = Some(BoardSelection {
            family: BoardFamily::HeltecV4,
            revision: "4.1".into(),
            confirmed_by_owner: true,
        });
        let refusal =
            plan_flash(&observation, &package()).expect_err("unsupported revision must refuse");
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::UnsupportedRevision { .. }))
        );
    }

    #[test]
    fn missing_loader_facts_are_refused_without_opening_a_port() {
        let mut observation = observation();
        observation.hardware = HardwareFacts::default();
        observation.firmware = FirmwareState::Bootloader;
        observation.status_reply = None;
        let refusal = plan_flash(&observation, &package()).expect_err("missing facts must refuse");
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::ProcessorMissing))
        );
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::FlashSizeMissing))
        );
        assert!(
            refusal
                .reasons
                .iter()
                .any(|reason| matches!(reason, RefusalReason::BootloaderMissing))
        );
    }
}
