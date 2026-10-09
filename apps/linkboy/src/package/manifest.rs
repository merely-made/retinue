use serde::{Deserialize, Serialize};

use super::{
    BoardFamily, FirmwarePartKind, FlashRange, FlashRoute, NODE_TIMEBASE_PRESERVED_RANGE,
    PERSISTENT_STATE_SCHEMA, PayloadFormat, ProcessorKind, StateImpact,
};

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

    pub(super) fn declared_parts(&self) -> Vec<PackagePart> {
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
