//! Strict firmware package metadata and payload verification.
//!
//! A package is a strict manifest plus the exact bytes named by that manifest. Planning and a
//! future executor receive this verified object, never an image path whose contents may have
//! changed since the decision was made.

mod kinds;
mod manifest;
#[cfg(test)]
mod tests;
mod uf2_layout;
mod validate;
mod verified;

use std::io;
use std::path::PathBuf;

pub use kinds::{
    BoardFamily, FirmwarePartKind, FlashRange, FlashRoute, PayloadFormat, ProcessorKind,
    StateImpact,
};
pub use manifest::{
    ExpectedApplication, FlashPackageManifest, HelperArtifact, HelperRequirement, PackagePart,
    PackagePayload, PackageTarget, PersistentStateCompatibility, PublisherSignature,
    PublisherSignatureFormat, RecoveryInstructions, helper_platform,
};
pub use verified::{FlashPackage, VerifiedPackagePart, sha256_hex};

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
