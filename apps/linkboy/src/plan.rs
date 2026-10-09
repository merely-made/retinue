//! Pure package/device compatibility and the immutable write plan.

mod admit;
#[cfg(test)]
mod tests;

use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::device::{BoardSelection, DeviceObservation};
use crate::package::{
    BoardFamily, FirmwarePartKind, FlashRange, FlashRoute, ProcessorKind, PublisherSignature,
    StateImpact,
};

pub use admit::plan_flash;

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
    pub parts: Vec<PackagePartIdentity>,
    pub publisher_signature: Option<PublisherSignature>,
}

/// The exact ordered artifacts approved for this device. A plan carries these rather than an
/// aggregate hash so a sparse write stays inspectable and cannot lose an offset in presentation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackagePartIdentity {
    pub kind: FirmwarePartKind,
    pub offset: Option<u32>,
    pub byte_length: u64,
    pub sha256: String,
}

/// The version and, where supplied, exact executable accepted for the approved write.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperIdentity {
    pub program: String,
    pub version: String,
    #[serde(default)]
    pub platform: Option<String>,
    #[serde(default)]
    pub binary_sha256: Option<String>,
    #[serde(default)]
    pub archive_sha256: Option<String>,
    #[serde(default)]
    pub archive_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FlashPlan {
    observation: DeviceObservation,
    package: PackageIdentity,
    board: BoardSelection,
    route: FlashRoute,
    helper: HelperIdentity,
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
            helper: HelperIdentity {
                program: route.helper().into(),
                version: "test".into(),
                platform: None,
                binary_sha256: None,
                archive_sha256: None,
                archive_url: None,
            },
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
        &self.helper.program
    }

    pub fn helper_identity(&self) -> &HelperIdentity {
        &self.helper
    }

    pub fn parts(&self) -> &[PackagePartIdentity] {
        &self.package.parts
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
            crate::device::DeviceTransport::SerialDfuPort(port) => port.as_str(),
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
            "flash plan\n  device: {device}\n  board: {} revision {}\n  package: {} {}\n  parts: {}\n  route: {}\n  helper: {}\n  write ranges: {}\n  preserved ranges: {}\n  state impact: {}\n  compatibility:\n{facts}\n  warnings:\n{warnings}\n  recovery before write: {}\n  recovery after failure: {}",
            self.board.family,
            self.board.revision,
            self.package.display_name,
            self.package.version,
            self.package
                .parts
                .iter()
                .map(describe_part)
                .collect::<Vec<_>>()
                .join(", "),
            self.route,
            describe_helper(&self.helper),
            describe_ranges(&self.write_ranges),
            describe_ranges(&self.preserved_ranges),
            self.state_impact,
            self.recovery_before_write,
            self.recovery_after_failure,
        )
    }
}

fn describe_helper(helper: &HelperIdentity) -> String {
    match (&helper.platform, &helper.binary_sha256) {
        (Some(platform), Some(digest)) => format!(
            "{} {} for {platform} (executable sha256 {digest})",
            helper.program, helper.version
        ),
        (_, Some(digest)) => format!("{} {} (sha256 {digest})", helper.program, helper.version),
        (_, None) => format!("{} {}", helper.program, helper.version),
    }
}

fn describe_part(part: &PackagePartIdentity) -> String {
    format!(
        "{} at {} ({} bytes, sha256 {})",
        part.kind,
        part.offset
            .map(|offset| format!("{offset:#x}"))
            .unwrap_or_else(|| "container layout".into()),
        part.byte_length,
        part.sha256,
    )
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
    #[error("helper {program} has no admitted release artifact for {platform}")]
    HelperPlatformUnsupported { program: String, platform: String },
    #[error(
        "running native-node state is durably guarded, but this package does not declare node-timebase-v1 support"
    )]
    PersistentStateCompatibilityRequired,
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

impl fmt::Display for PackageIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} {}", self.display_name, self.version)
    }
}
