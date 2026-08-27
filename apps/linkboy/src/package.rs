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

pub const PACKAGE_SCHEMA: u32 = 2;
/// Schema for the persistent native-node reservation record described by a package.
pub const PERSISTENT_STATE_SCHEMA: u32 = 1;
/// Concrete running-state token emitted after a board has verified its durable lease.
pub const NODE_TIMEBASE_GUARD: &str = "node-timebase-v1";
/// The T114 flash interval that a guarded native-node image must preserve.
pub const NODE_TIMEBASE_PRESERVED_RANGE: FlashRange = FlashRange {
    start: 0xe8000,
    length: 0x4000,
};
const ESP_FLASH_SECTOR_SIZE: u32 = 0x1000;

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
    Uf2MassStorage,
}

impl FlashRoute {
    pub fn helper(&self) -> &'static str {
        match self {
            Self::AdafruitDfu => "adafruit-nrfutil",
            Self::EspRom => "espflash",
            Self::Uf2MassStorage => "linkboy UF2 volume writer",
        }
    }

    /// UF2 is a file-copy protocol implemented here, not an unpinned shell helper.
    pub fn uses_builtin_writer(&self) -> bool {
        matches!(self, Self::Uf2MassStorage)
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::AdafruitDfu => "serial DFU (adafruit-nrfutil)",
            Self::EspRom => "ESP ROM loader (espflash)",
            Self::Uf2MassStorage => "UF2 mass-storage bootloader",
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
    Uf2,
    RawBinary,
}

/// The job a part performs in a sparse image. The manifest order is also the write order.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FirmwarePartKind {
    Bootloader,
    PartitionTable,
    Application,
}

impl fmt::Display for FirmwarePartKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Bootloader => "bootloader",
            Self::PartitionTable => "partition table",
            Self::Application => "application",
        })
    }
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
pub struct HelperArtifact {
    /// Linkboy's installed helper directory name, for example `windows-x86_64`.
    pub platform: String,
    /// Digest of the executable after extracting the upstream release archive.
    pub binary_sha256: String,
    /// Digest published for the retained upstream release archive.
    pub archive_sha256: String,
    pub archive_url: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HelperRequirement {
    pub route: FlashRoute,
    pub program: String,
    pub version: String,
    /// Legacy single-platform custody. New public packages use `artifacts`.
    #[serde(default)]
    pub binary_sha256: Option<String>,
    /// Official release artifacts admitted on each supported host platform.
    #[serde(default)]
    pub artifacts: Vec<HelperArtifact>,
    pub license: String,
    pub source_url: String,
    pub notice: String,
}

impl HelperRequirement {
    pub fn artifact_for_current_platform(&self) -> Option<&HelperArtifact> {
        let platform = helper_platform();
        self.artifacts
            .iter()
            .find(|artifact| artifact.platform == platform)
    }

    pub fn expected_binary_sha256(&self) -> Option<&str> {
        self.artifact_for_current_platform()
            .map(|artifact| artifact.binary_sha256.as_str())
            .or(self.binary_sha256.as_deref())
    }
}

pub fn helper_platform() -> String {
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
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

/// One immutable file in an ordered firmware package.
///
/// ESP sparse packages state a concrete flash offset per part. Container formats, such as the
/// nRF DFU ZIP and a self-contained ESP ELF, keep their address layout inside the container and
/// therefore have no outer offset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackagePart {
    pub kind: FirmwarePartKind,
    pub path: String,
    pub format: PayloadFormat,
    pub offset: Option<u32>,
    pub byte_length: u64,
    pub sha256: String,
    pub write_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PublisherSignatureFormat {
    Minisign,
}

/// Evidence retained from an upstream publisher. It is deliberately evidence, not a Linkboy
/// trust root: the signed Merely package index still decides which network package is admitted.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublisherSignature {
    pub format: PublisherSignatureFormat,
    pub key_id: String,
    pub signed_manifest_url: String,
    pub signed_manifest_sha256: String,
    pub signature: String,
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
    /// External firmware can require a human to exercise its own interface after the helper has
    /// verified every written part. This prevents a Retinue-only serial probe from claiming a
    /// foreign application is broken.
    #[serde(default)]
    pub manual_check: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecoveryInstructions {
    pub before_write: String,
    pub after_failure: String,
}

/// Compatibility evidence for a firmware image that can safely continue a durable native-node
/// announce sequence after a reset. An absent declaration means that the image makes no such
/// claim. The declaration is intentionally small and additive so schema-2 manifests remain
/// readable by older Linkboy builds.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistentStateCompatibility {
    pub schema: u32,
    pub native_node_guard: bool,
    pub preserved_range: FlashRange,
}

impl PersistentStateCompatibility {
    pub fn supports_native_node_guard(&self) -> bool {
        self.schema == PERSISTENT_STATE_SCHEMA
            && self.native_node_guard
            && self.preserved_range == NODE_TIMEBASE_PRESERVED_RANGE
    }
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
    /// The legacy one-file package form. Schema v2 keeps it for the current Retinue routes;
    /// new sparse packages use `parts` instead.
    #[serde(default)]
    pub payload: Option<PackagePayload>,
    /// Ordered immutable parts for a sparse package. Exactly one of `payload` and `parts` is
    /// present, so an old opaque container cannot quietly become an incomplete sparse write.
    #[serde(default)]
    pub parts: Vec<PackagePart>,
    pub targets: Vec<PackageTarget>,
    /// Explicit ranges for an opaque one-file container. Sparse packages derive their ranges
    /// from the verified part offsets and must not carry a second, contradictory range list.
    #[serde(default)]
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
    pub publisher_signature: Option<PublisherSignature>,
    pub recovery: RecoveryInstructions,
    /// Optional additive declaration. Foreign and legacy packages omit it and therefore never
    /// accidentally claim support for the guarded Retinue native-node state.
    #[serde(default)]
    pub persistent_state: Option<PersistentStateCompatibility>,
}

impl FlashPackageManifest {
    pub fn helper_for(&self, route: &FlashRoute) -> Option<&HelperRequirement> {
        self.helpers.iter().find(|helper| &helper.route == route)
    }

    pub fn write_ranges(&self) -> Vec<FlashRange> {
        if self.parts.is_empty() {
            self.write_ranges.clone()
        } else {
            self.parts
                .iter()
                .filter_map(|part| {
                    Some(FlashRange {
                        start: part.offset?,
                        length: u32::try_from(part.write_bytes).ok()?,
                    })
                })
                .collect()
        }
    }

    fn declared_parts(&self) -> Vec<PackagePart> {
        match &self.payload {
            Some(payload) => vec![PackagePart {
                kind: FirmwarePartKind::Application,
                path: payload.path.clone(),
                format: payload.format.clone(),
                offset: None,
                byte_length: payload.byte_length,
                sha256: payload.sha256.clone(),
                write_bytes: payload.write_bytes,
            }],
            None => self.parts.clone(),
        }
    }
}

/// A manifest whose payload has been read and verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlashPackage {
    manifest: FlashPackageManifest,
    manifest_path: PathBuf,
    parts: Vec<VerifiedPackagePart>,
}

/// A package part whose exact bytes have been checked against the manifest before planning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerifiedPackagePart {
    declaration: PackagePart,
    path: PathBuf,
    bytes: Vec<u8>,
}

impl VerifiedPackagePart {
    pub fn declaration(&self) -> &PackagePart {
        &self.declaration
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

impl FlashPackage {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, PackageError> {
        let manifest_path = path.as_ref().to_path_buf();
        let text = fs::read_to_string(&manifest_path)?;
        let manifest: FlashPackageManifest = toml::from_str(&text)?;
        let parent = manifest_path.parent().unwrap_or_else(|| Path::new("."));
        let parts = manifest
            .declared_parts()
            .into_iter()
            .map(|part| {
                let path = parent.join(&part.path);
                fs::read(&path).map(|bytes| (path, bytes))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_verified_parts(manifest, manifest_path, parts)
    }

    /// Test and embedding seam for callers that already have the manifest and bytes. It never
    /// opens a serial port or consults the operating system beyond the supplied values.
    pub fn from_parts(
        manifest: FlashPackageManifest,
        manifest_path: impl Into<PathBuf>,
        payload_path: impl Into<PathBuf>,
        payload: Vec<u8>,
    ) -> Result<Self, PackageError> {
        if manifest.payload.is_none() {
            return Err(PackageError::PartCountMismatch {
                expected: manifest.parts.len(),
                actual: 1,
            });
        }
        Self::from_verified_parts(
            manifest,
            manifest_path,
            vec![(payload_path.into(), payload)],
        )
    }

    /// Test and embedding seam for multi-part packages. Input order is the manifest order and
    /// every part is verified before this returns an object that planning or execution can use.
    pub fn from_verified_parts(
        manifest: FlashPackageManifest,
        manifest_path: impl Into<PathBuf>,
        supplied_parts: Vec<(PathBuf, Vec<u8>)>,
    ) -> Result<Self, PackageError> {
        validate_manifest(&manifest)?;
        let declared_parts = manifest.declared_parts();
        if declared_parts.len() != supplied_parts.len() {
            return Err(PackageError::PartCountMismatch {
                expected: declared_parts.len(),
                actual: supplied_parts.len(),
            });
        }
        let parts = declared_parts
            .into_iter()
            .zip(supplied_parts)
            .map(|(declaration, (path, bytes))| {
                let actual_length = bytes.len() as u64;
                if actual_length != declaration.byte_length {
                    return Err(PackageError::LengthMismatch {
                        path,
                        expected: declaration.byte_length,
                        actual: actual_length,
                    });
                }
                let actual_hash = sha256_hex(&bytes);
                if !actual_hash.eq_ignore_ascii_case(&declaration.sha256) {
                    return Err(PackageError::HashMismatch {
                        path,
                        expected: declaration.sha256.clone(),
                        actual: actual_hash,
                    });
                }
                Ok(VerifiedPackagePart {
                    declaration,
                    path,
                    bytes,
                })
            })
            .collect::<Result<Vec<_>, PackageError>>()?;
        validate_uf2_layout(&manifest, &parts)?;
        Ok(Self {
            manifest,
            manifest_path: manifest_path.into(),
            parts,
        })
    }

    pub fn manifest(&self) -> &FlashPackageManifest {
        &self.manifest
    }

    pub fn manifest_path(&self) -> &Path {
        &self.manifest_path
    }

    pub fn parts(&self) -> &[VerifiedPackagePart] {
        &self.parts
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
             \n  parts: {}\
             \n  targets: {}\
             \n  routes: {}\
             \n  write ranges: {}\
             \n  preserved ranges: {}\
             \n  regions: {}\
             \n  channel capabilities: {}\
             \n  state impact: {}\
             \n  persistent state: {}\
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
            self.parts
                .iter()
                .map(|part| format!(
                    "{} {} at {} ({} bytes, sha256 {})",
                    part.declaration.kind,
                    part.path.display(),
                    part.declaration
                        .offset
                        .map(|offset| format!("{offset:#x}"))
                        .unwrap_or_else(|| "container layout".into()),
                    part.declaration.byte_length,
                    part.declaration.sha256,
                ))
                .collect::<Vec<_>>()
                .join(", "),
            targets,
            self.manifest
                .targets
                .iter()
                .map(|target| target.route.label())
                .collect::<Vec<_>>()
                .join(", "),
            ranges_description(&self.manifest.write_ranges()),
            ranges_description(&self.manifest.preserved_ranges),
            self.manifest.regions.join(", "),
            self.manifest.channel_capabilities.join(", "),
            self.manifest.state_impact,
            self.manifest
                .persistent_state
                .as_ref()
                .map(|state| {
                    format!(
                        "schema {} native-node-guard={} preserved {}",
                        state.schema,
                        state.native_node_guard,
                        ranges_description(std::slice::from_ref(&state.preserved_range))
                    )
                })
                .unwrap_or_else(|| "not declared".into()),
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

const UF2_BLOCK_SIZE: usize = 512;
const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_ID: u32 = 0x0000_2000;

/// UF2 carries target addresses inside its fixed-size blocks. Checking them here keeps a
/// one-file package's declared write ranges real rather than an optimistic side note beside
/// an opaque blob.
fn validate_uf2_layout(
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
    for (index, block) in bytes.chunks_exact(UF2_BLOCK_SIZE).enumerate() {
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
    #[error("package has {actual} verified parts, but its manifest requires {expected}")]
    PartCountMismatch { expected: usize, actual: usize },
    #[error(
        "package part length mismatch for {}: manifest says {expected}, bytes contain {actual}",
        path.display()
    )]
    LengthMismatch {
        path: PathBuf,
        expected: u64,
        actual: u64,
    },
    #[error(
        "package part hash mismatch for {}: manifest says {expected}, bytes hash to {actual}",
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
                binary_sha256: None,
                artifacts: Vec::new(),
                license: "MIT OR Apache-2.0".into(),
                source_url: "https://example.invalid/espflash".into(),
                notice: "Test helper notice".into(),
            }],
            payload: Some(PackagePayload {
                path: "payload.bin".into(),
                format: PayloadFormat::EspflashElf,
                byte_length: payload.len() as u64,
                sha256: sha256_hex(payload),
                write_bytes: payload.len() as u64,
            }),
            parts: Vec::new(),
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
                manual_check: None,
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
            persistent_state: None,
        }
    }

    fn sparse_manifest(parts: &[(&str, FirmwarePartKind, u32, &[u8])]) -> FlashPackageManifest {
        let mut value = manifest(b"legacy-container");
        value.payload = None;
        value.write_ranges.clear();
        value.preserved_ranges = vec![FlashRange {
            start: 0xd000,
            length: 0x1000,
        }];
        value.parts = parts
            .iter()
            .map(|(path, kind, offset, bytes)| PackagePart {
                kind: kind.clone(),
                path: (*path).into(),
                format: PayloadFormat::RawBinary,
                offset: Some(*offset),
                byte_length: bytes.len() as u64,
                sha256: sha256_hex(bytes),
                write_bytes: bytes.len() as u64,
            })
            .collect();
        value.publisher_signature = Some(PublisherSignature {
            format: PublisherSignatureFormat::Minisign,
            key_id: "1FB2CA18B2C25E1F".into(),
            signed_manifest_url: "https://example.invalid/hopspot/flash-manifest.json".into(),
            signed_manifest_sha256: "b".repeat(64),
            signature: "untrusted comment: retained upstream signature".into(),
        });
        value
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
    fn native_node_guard_must_match_a_real_preserved_range() {
        let mut value = manifest(b"payload");
        value.persistent_state = Some(PersistentStateCompatibility {
            schema: PERSISTENT_STATE_SCHEMA,
            native_node_guard: true,
            preserved_range: NODE_TIMEBASE_PRESERVED_RANGE,
        });
        let error =
            FlashPackage::from_parts(value, "manifest.toml", "payload.bin", b"payload".to_vec())
                .expect_err("a guard claim without a preserved range must be refused");
        assert!(
            matches!(error, PackageError::InvalidField(message) if message.contains("preserved_ranges"))
        );
    }

    #[test]
    fn sparse_parts_keep_ordered_hashes_offsets_and_publisher_evidence() {
        let bootloader = b"bootloader";
        let partition_table = b"partition-table";
        let application = b"application";
        let manifest = sparse_manifest(&[
            (
                "bootloader.bin",
                FirmwarePartKind::Bootloader,
                0,
                bootloader,
            ),
            (
                "partition-table.bin",
                FirmwarePartKind::PartitionTable,
                0x8000,
                partition_table,
            ),
            (
                "application.bin",
                FirmwarePartKind::Application,
                0x10000,
                application,
            ),
        ]);
        let package = FlashPackage::from_verified_parts(
            manifest,
            "manifest.toml",
            vec![
                (PathBuf::from("bootloader.bin"), bootloader.to_vec()),
                (
                    PathBuf::from("partition-table.bin"),
                    partition_table.to_vec(),
                ),
                (PathBuf::from("application.bin"), application.to_vec()),
            ],
        )
        .expect("ordered sparse parts should verify");
        assert_eq!(package.parts().len(), 3);
        assert_eq!(
            package
                .parts()
                .iter()
                .map(|part| part.declaration().offset)
                .collect::<Vec<_>>(),
            vec![Some(0), Some(0x8000), Some(0x10000)]
        );
        assert_eq!(package.manifest().write_ranges().len(), 3);
        assert!(package.manifest().publisher_signature.is_some());
    }

    #[test]
    fn changed_sparse_part_is_rejected_before_a_plan_exists() {
        let bootloader = b"bootloader";
        let partition_table = b"partition-table";
        let application = b"application";
        let manifest = sparse_manifest(&[
            (
                "bootloader.bin",
                FirmwarePartKind::Bootloader,
                0,
                bootloader,
            ),
            (
                "partition-table.bin",
                FirmwarePartKind::PartitionTable,
                0x8000,
                partition_table,
            ),
            (
                "application.bin",
                FirmwarePartKind::Application,
                0x10000,
                application,
            ),
        ]);
        let error = FlashPackage::from_verified_parts(
            manifest,
            "manifest.toml",
            vec![
                (PathBuf::from("bootloader.bin"), bootloader.to_vec()),
                (PathBuf::from("partition-table.bin"), b"changed".to_vec()),
                (PathBuf::from("application.bin"), application.to_vec()),
            ],
        )
        .expect_err("a changed sparse artifact must invalidate the complete package");
        assert!(matches!(error, PackageError::LengthMismatch { .. }));
    }

    #[test]
    fn manual_external_package_may_leave_retinue_capabilities_unspecified() {
        let bootloader = b"bootloader";
        let partition_table = b"partition-table";
        let application = b"application";
        let mut manifest = sparse_manifest(&[
            (
                "bootloader.bin",
                FirmwarePartKind::Bootloader,
                0,
                bootloader,
            ),
            (
                "partition-table.bin",
                FirmwarePartKind::PartitionTable,
                0x8000,
                partition_table,
            ),
            (
                "application.bin",
                FirmwarePartKind::Application,
                0x10000,
                application,
            ),
        ]);
        manifest.regions.clear();
        manifest.channel_capabilities.clear();
        manifest.expected_application.manual_check =
            Some("Exercise the upstream interface.".into());
        assert!(
            FlashPackage::from_verified_parts(
                manifest,
                "manifest",
                vec![
                    ("bootloader.bin".into(), bootloader.to_vec()),
                    ("partition-table.bin".into(), partition_table.to_vec()),
                    ("application.bin".into(), application.to_vec()),
                ],
            )
            .is_ok()
        );
    }

    #[test]
    fn sparse_part_cannot_enter_a_preserved_provisioning_slot() {
        let bootloader = b"bootloader";
        let partition_table = b"partition-table";
        let application = b"application";
        let manifest = sparse_manifest(&[
            (
                "bootloader.bin",
                FirmwarePartKind::Bootloader,
                0,
                bootloader,
            ),
            (
                "partition-table.bin",
                FirmwarePartKind::PartitionTable,
                0xd000,
                partition_table,
            ),
            (
                "application.bin",
                FirmwarePartKind::Application,
                0x10000,
                application,
            ),
        ]);
        let error = FlashPackage::from_verified_parts(
            manifest,
            "manifest.toml",
            vec![
                (PathBuf::from("bootloader.bin"), bootloader.to_vec()),
                (
                    PathBuf::from("partition-table.bin"),
                    partition_table.to_vec(),
                ),
                (PathBuf::from("application.bin"), application.to_vec()),
            ],
        )
        .expect_err("a sparse write cannot touch provisioning");
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

    #[test]
    fn nrf52840_uf2_without_the_matching_family_id_is_rejected() {
        let mut bytes =
            crate::uf2::encode_application(b"application", 0x26000, crate::uf2::NRF52840_FAMILY_ID)
                .unwrap();
        bytes[28..32].copy_from_slice(&0_u32.to_le_bytes());

        let mut value = manifest(&bytes);
        value.helpers[0].route = FlashRoute::Uf2MassStorage;
        value.helpers[0].program = FlashRoute::Uf2MassStorage.helper().into();
        value.payload = Some(PackagePayload {
            path: "payload.uf2".into(),
            format: PayloadFormat::Uf2,
            byte_length: bytes.len() as u64,
            sha256: sha256_hex(&bytes),
            write_bytes: crate::uf2::PAYLOAD_SIZE as u64,
        });
        value.targets = vec![PackageTarget {
            family: BoardFamily::T114,
            revision: "2.x".into(),
            processor: ProcessorKind::Nrf52840,
            flash_size: 1024 * 1024,
            bootloader: "adafruit-uf2-0.9.0".into(),
            route: FlashRoute::Uf2MassStorage,
        }];
        value.write_ranges = vec![FlashRange {
            start: 0x26000,
            length: crate::uf2::PAYLOAD_SIZE as u32,
        }];
        value.preserved_ranges = vec![FlashRange {
            start: 0x26100,
            length: 1,
        }];
        value.expected_application.board = BoardFamily::T114;

        let error = FlashPackage::from_parts(value, "manifest", "payload.uf2", bytes)
            .expect_err("the nRF52840 family guard must be part of package admission");
        assert!(matches!(
            error,
            PackageError::InvalidField(detail) if detail.contains("nRF52840 family id")
        ));
    }
}
