//! Packages whose part bytes have been read and checked against the manifest.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::uf2_layout::validate_uf2_layout;
use super::validate::validate_manifest;
use super::{
    FlashPackageManifest, FlashRange, FlashRoute, HelperRequirement, PackageError, PackagePart,
};

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

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}
