//! Strict firmware package metadata and payload verification.
//!
//! A package is a strict manifest plus the exact bytes named by that manifest. Planning and a
//! future executor receive this verified object, never an image path whose contents may have
//! changed since the decision was made.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PACKAGE_SCHEMA: u32 = 1;

/// A carrier-board family, not a processor family.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardFamily {
    T114,
    HeltecV4,
}

impl BoardFamily {
    pub fn label(&self) -> &'static str {
        match self {
            Self::T114 => "T114",
            Self::HeltecV4 => "Heltec V4",
        }
    }

    pub fn from_board(board: &crate::Board) -> Option<Self> {
        match board {
            crate::Board::T114 => Some(Self::T114),
            crate::Board::HeltecV4 => Some(Self::HeltecV4),
            crate::Board::Unknown(_) => None,
        }
    }
}

impl fmt::Display for BoardFamily {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Processor identities that a loader can prove for the first two routes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProcessorKind {
    Nrf52840,
    Esp32S3,
}

impl fmt::Display for ProcessorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Nrf52840 => "nRF52840",
            Self::Esp32S3 => "ESP32-S3",
        })
    }
}

/// The concrete transport route an executor will use.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlashRoute {
    AdafruitDfu,
    EspRom,
}

impl FlashRoute {
    pub fn helper(&self) -> &'static str {
        match self {
            Self::AdafruitDfu => "adafruit-nrfutil",
            Self::EspRom => "espflash",
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::AdafruitDfu => "serial DFU (adafruit-nrfutil)",
            Self::EspRom => "ESP ROM loader (espflash)",
        }
    }
}

impl fmt::Display for FlashRoute {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PayloadFormat {
    NrfDfuZip,
    EspflashElf,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateImpact {
    Preserved,
    Replaced,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperRequirement {
    pub route: FlashRoute,
    pub program: String,
    pub version: String,
    pub license: String,
    pub source_url: String,
    pub notice: String,
}

impl fmt::Display for StateImpact {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Preserved => "preserved",
            Self::Replaced => "replaced",
            Self::Unknown => "unknown",
        })
    }
}

/// A half-open byte range in target flash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashRange {
    pub start: u32,
    pub length: u32,
}

impl FlashRange {
    pub fn end(&self) -> Option<u32> {
        self.start.checked_add(self.length)
    }

    pub fn overlaps(&self, other: &Self) -> bool {
        let Some(end) = self.end() else {
            return true;
        };
        let Some(other_end) = other.end() else {
            return true;
        };
        self.start < other_end && other.start < end
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagePayload {
    pub path: String,
    pub format: PayloadFormat,
    pub byte_length: u64,
    pub sha256: String,
    /// Bytes represented by the application image in a container format. This is distinct from
    /// byte_length, which is always the exact file length that was hashed.
    pub write_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageTarget {
    pub family: BoardFamily,
    pub revision: String,
    pub processor: ProcessorKind,
    pub flash_size: u32,
    pub bootloader: String,
    pub route: FlashRoute,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedApplication {
    pub board: BoardFamily,
    pub version: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInstructions {
    pub before_write: String,
    pub after_failure: String,
}

/// The strict on-disk shape. Unknown keys are rejected so a typo cannot silently weaken a plan.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlashPackageManifest {
    pub schema: u32,
    pub package_id: String,
    pub display_name: String,
    pub version: String,
    pub publisher: String,
    pub helpers: Vec<HelperRequirement>,
    pub payload: PackagePayload,
    pub targets: Vec<PackageTarget>,
    pub write_ranges: Vec<FlashRange>,
    pub preserved_ranges: Vec<FlashRange>,
    pub regions: Vec<String>,
    pub channel_capabilities: Vec<String>,
    pub state_impact: StateImpact,
    pub expected_application: ExpectedApplication,
    pub license: String,
    pub notices: String,
    pub source_revision: String,
    pub source_url: String,
    pub origin_url: String,
    pub publisher_signature: Option<String>,
    pub recovery: RecoveryInstructions,
}

impl FlashPackageManifest {
    pub fn helper_for(&self, route: &FlashRoute) -> Option<&HelperRequirement> {
        self.helpers.iter().find(|helper| &helper.route == route)
    }
}

/// A manifest whose payload has been read and verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashPackage {
    manifest: FlashPackageManifest,
    manifest_path: PathBuf,
    payload_path: PathBuf,
    payload: Vec<u8>,
}

impl FlashPackage {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PackageError> {
        let manifest_path = path.as_ref().to_path_buf();
        let text = fs::read_to_string(&manifest_path)?;
        let manifest: FlashPackageManifest = toml::from_str(&text)?;
        let parent = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        let payload_path = parent.join(&manifest.payload.path);
        let payload = fs::read(&payload_path)?;
        Self::from_parts(manifest, manifest_path, payload_path, payload)
    }

    /// Test and embedding seam for callers that already have the manifest and bytes. It never
    /// opens a serial port or consults the operating system beyond the supplied values.
    pub fn from_parts(
        manifest: FlashPackageManifest,
        manifest_path: impl Into<PathBuf>,
        payload_path: impl Into<PathBuf>,
        payload: Vec<u8>,
    ) -> Result<Self, PackageError> {
        validate_manifest(&manifest)?;
        let payload_path = payload_path.into();
        let actual_length = payload.len() as u64;
        if actual_length != manifest.payload.byte_length {
            return Err(PackageError::LengthMismatch {
                path: payload_path,
                expected: manifest.payload.byte_length,
                actual: actual_length,
            });
        }
        let actual_hash = sha256_hex(&payload);
        if !actual_hash.eq_ignore_ascii_case(&manifest.payload.sha256) {
            return Err(PackageError::HashMismatch {
                path: payload_path,
                expected: manifest.payload.sha256.clone(),
                actual: actual_hash,
            });
        }
        Ok(Self {
            manifest,
            manifest_path: manifest_path.into(),
            payload_path,
            payload,
        })
    }

    pub fn manifest(&self) -> &FlashPackageManifest {
        &self.manifest
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn payload_path(&self) -> &Path {
        &self.payload_path
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn helper_for(&self, route: &FlashRoute) -> Option<&HelperRequirement> {
        self.manifest.helper_for(route)
    }

    pub fn describe(&self) -> String {
        let targets = self
            .manifest
            .targets
            .iter()
            .map(|target| format!("{} {}", target.family, target.revision))
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "package {}\
             \n  name: {}\
             \n  version: {}\
             \n  publisher: {}\
             \n  helpers: {}\
             \n  payload: {} ({} bytes, sha256 {})\
             \n  targets: {}\
             \n  routes: {}\
             \n  write ranges: {}\
             \n  preserved ranges: {}\
             \n  regions: {}\
             \n  channel capabilities: {}\
             \n  state impact: {}\
             \n  license: {}\
             \n  source: {} @ {}\
             \n  recovery: {}",
            self.manifest.package_id,
            self.manifest.display_name,
            self.manifest.version,
            self.manifest.publisher,
            self.manifest
                .helpers
                .iter()
                .map(|helper| format!("{} {}", helper.program, helper.version))
                .collect::<Vec<_>>()
                .join(", "),
            self.payload_path.display(),
            self.manifest.payload.byte_length,
            self.manifest.payload.sha256,
            targets,
            self.manifest
                .targets
                .iter()
                .map(|target| target.route.label())
                .collect::<Vec<_>>()
                .join(", "),
            ranges_description(&self.manifest.write_ranges),
            ranges_description(&self.manifest.preserved_ranges),
            self.manifest.regions.join(", "),
            self.manifest.channel_capabilities.join(", "),
            self.manifest.state_impact,
            self.manifest.license,
            self.manifest.source_url,
            self.manifest.source_revision,
            self.manifest.recovery.before_write,
        )
    }
}

fn ranges_description(ranges: &[FlashRange]) -> String {
    ranges
        .iter()
        .map(|range| match range.end() {
            Some(end) => format!("{:#x}..{:#x}", range.start, end),
            None => format!("{:#x}..overflow", range.start),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn validate_manifest(manifest: &FlashPackageManifest) -> Result<(), PackageError> {
    if manifest.schema != PACKAGE_SCHEMA {
        return Err(PackageError::UnsupportedSchema(manifest.schema));
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
    if manifest.payload.path.trim().is_empty()
        || manifest.payload.byte_length == 0
        || manifest.payload.write_bytes == 0
        || !is_sha256(&manifest.payload.sha256)
    {
        return Err(PackageError::InvalidField("payload".to_string()));
    }
    if manifest.targets.is_empty()
        || manifest.helpers.is_empty()
        || manifest.write_ranges.is_empty()
        || manifest.regions.is_empty()
        || manifest.channel_capabilities.is_empty()
        || manifest.expected_application.version.trim().is_empty()
    {
        return Err(PackageError::InvalidField(
            "targets, helpers, write_ranges, regions, channel capabilities, and expected application must not be empty"
                .to_string(),
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
    for target in &manifest.targets {
        if target.revision.trim().is_empty()
            || target.bootloader.trim().is_empty()
            || target.flash_size == 0
        {
            return Err(PackageError::InvalidField("target".to_string()));
        }
        if manifest
            .write_ranges
            .iter()
            .chain(manifest.preserved_ranges.iter())
            .any(|range| range.end().is_none() || range.end().unwrap() > target.flash_size)
        {
            return Err(PackageError::InvalidField(format!(
                "range outside {} flash",
                target.family
            )));
        }
        let format_matches_route = matches!(
            (&target.route, &manifest.payload.format),
            (FlashRoute::AdafruitDfu, PayloadFormat::NrfDfuZip)
                | (FlashRoute::EspRom, PayloadFormat::EspflashElf)
        );
        if !format_matches_route {
            return Err(PackageError::InvalidField(format!(
                "payload format does not match {} route",
                target.route
            )));
        }
        let Some(helper) = manifest.helper_for(&target.route) else {
            return Err(PackageError::InvalidField(format!(
                "missing helper for {} route",
                target.route
            )));
        };
        if helper.program != target.route.helper()
            || helper.version.trim().is_empty()
            || helper.license.trim().is_empty()
            || helper.source_url.trim().is_empty()
            || helper.notice.trim().is_empty()
        {
            return Err(PackageError::InvalidField(format!(
                "invalid helper metadata for {}",
                target.route
            )));
        }
    }
    if has_overlap(&manifest.write_ranges)
        || has_overlap(&manifest.preserved_ranges)
        || manifest.write_ranges.iter().any(|write| {
            manifest
                .preserved_ranges
                .iter()
                .any(|preserved| write.overlaps(preserved))
        })
    {
        return Err(PackageError::ProtectedRangeOverlap);
    }
    Ok(())
}

fn has_overlap(ranges: &[FlashRange]) -> bool {
    ranges.iter().enumerate().any(|(index, range)| {
        ranges
            .iter()
            .skip(index + 1)
            .any(|other| range.overlaps(other))
    })
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum PackageError {
    #[error("cannot read package file: {0}")]
    Io(#[from] io::Error),
    #[error("invalid package TOML: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("unsupported package schema {0}; expected {PACKAGE_SCHEMA}")]
    UnsupportedSchema(u32),
    #[error("invalid package field: {0}")]
    InvalidField(String),
    #[error("package write or preserved ranges overlap")]
    ProtectedRangeOverlap,
    #[error(
        "payload length mismatch for {}: manifest says {expected}, bytes contain {actual}",
        path.display()
    )]
    LengthMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error(
        "payload hash mismatch for {}: manifest says {expected}, bytes hash to {actual}",
        path.display()
    )]
    HashMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(payload: &[u8]) -> FlashPackageManifest {
        FlashPackageManifest {
            schema: PACKAGE_SCHEMA,
            package_id: "test.package".into(),
            display_name: "Test package".into(),
            version: "1".into(),
            publisher: "Test publisher".into(),
            helpers: vec![HelperRequirement {
                route: FlashRoute::EspRom,
                program: "espflash".into(),
                version: "4.5.0".into(),
                license: "MIT OR Apache-2.0".into(),
                source_url: "https://example.invalid/espflash".into(),
                notice: "Test helper notice".into(),
            }],
            payload: PackagePayload {
                path: "payload.bin".into(),
                format: PayloadFormat::EspflashElf,
                byte_length: payload.len() as u64,
                sha256: sha256_hex(payload),
                write_bytes: payload.len() as u64,
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
            notices: "Test notices".into(),
            source_revision: "test".into(),
            source_url: "https://example.invalid/source".into(),
            origin_url: "https://example.invalid/package".into(),
            publisher_signature: None,
            recovery: RecoveryInstructions {
                before_write: "Keep the cable connected.".into(),
                after_failure: "Enter the ROM loader again.".into(),
            },
        }
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(
            sha256_hex(b"linkboy"),
            "5e27c306d7ec7d9f0527bbd3a7591e1578d82849c23cdd0a373a7f69b89e4e95"
        );
    }

    #[test]
    fn a_single_changed_byte_is_rejected() {
        let original = b"payload".to_vec();
        let mut changed = original.clone();
        changed[0] ^= 1;
        let error =
            FlashPackage::from_parts(manifest(&original), "manifest.toml", "payload.bin", changed)
                .expect_err("changing one payload byte must invalidate the package");
        assert!(matches!(error, PackageError::HashMismatch { .. }));
    }

    #[test]
    fn protected_ranges_are_rejected_before_payload_use() {
        let mut value = manifest(b"payload");
        value.preserved_ranges[0].start = 0x3eff00;
        let error =
            FlashPackage::from_parts(value, "manifest.toml", "payload.bin", b"payload".to_vec())
                .expect_err("a write crossing a preserved range must be refused");
        assert!(matches!(error, PackageError::ProtectedRangeOverlap));
    }

    #[test]
    fn unknown_manifest_keys_are_rejected() {
        let text = r#"
schema = 1
package_id = "x"
display_name = "x"
version = "1"
publisher = "x"
unexpected = true
"#;
        assert!(toml::from_str::<FlashPackageManifest>(text).is_err());
    }
}
