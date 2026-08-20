//! Public package-index metadata for firmware packages.
//!
//! The catalog is deliberately a separate authority from a package manifest. A manifest says
//! what Linkboy can safely install; this index says whether that package has public installer
//! and recovery evidence, and whether it is ready to be offered for purchase.

use std::fs;
use std::io;
use std::path::{Component, Path};

use minisign_verify::{PublicKey, Signature};
use serde::{Deserialize, Serialize};

use crate::package::{FlashPackage, PackageError};

pub const PACKAGE_INDEX_SCHEMA: &str = "retinue.package-index/v1";
pub const CATALOG_TRUST_SCHEMA: &str = "retinue.catalog-trust/v1";
const CATALOG_SIGNING_DOMAIN: &str = "retinue.package-index/canonical-v1\n";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogSignatureFormat {
    Minisign,
}

/// Detached authority over the canonical package-index value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSignature {
    pub format: CatalogSignatureFormat,
    pub key_id: String,
    pub signature: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrustedCatalogKey {
    pub publisher: String,
    pub key_id: String,
    pub public_key: String,
}

/// Owner-selected trust roots. This file is local policy, never supplied by a catalog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogTrust {
    pub schema: String,
    pub keys: Vec<TrustedCatalogKey>,
}

impl CatalogTrust {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let text = fs::read_to_string(path)?;
        let trust: Self = toml::from_str(&text)?;
        trust.validate()?;
        Ok(trust)
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema != CATALOG_TRUST_SCHEMA {
            return Err(CatalogError::Invalid(format!(
                "unsupported catalog-trust schema {:?}",
                self.schema
            )));
        }
        if self.keys.is_empty() {
            return Err(CatalogError::Invalid(
                "catalog trust needs at least one key".into(),
            ));
        }
        for (index, key) in self.keys.iter().enumerate() {
            if key.publisher.trim().is_empty()
                || key.key_id.trim().is_empty()
                || key.public_key.trim().is_empty()
            {
                return Err(CatalogError::Invalid(format!(
                    "catalog trust key {index} has an empty field"
                )));
            }
            if PublicKey::from_base64(&key.public_key).is_err() {
                return Err(CatalogError::Invalid(format!(
                    "catalog trust key {:?} is not a Minisign public key",
                    key.key_id
                )));
            }
            if self.keys[..index]
                .iter()
                .any(|known| known.publisher == key.publisher && known.key_id == key.key_id)
            {
                return Err(CatalogError::Invalid(format!(
                    "catalog trust repeats publisher {:?} key {:?}",
                    key.publisher, key.key_id
                )));
            }
        }
        Ok(())
    }

    fn key(&self, publisher: &str, key_id: &str) -> Option<&TrustedCatalogKey> {
        self.keys
            .iter()
            .find(|key| key.publisher == publisher && key.key_id == key_id)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogState {
    Partial,
    ProvenRecipe,
    Sellable,
}

impl CatalogState {
    fn label(&self) -> &'static str {
        match self {
            Self::Partial => "partial",
            Self::ProvenRecipe => "proven-recipe",
            Self::Sellable => "sellable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPackage {
    pub package_id: String,
    /// Monotonic publisher sequence used to reject rollback to an older release.
    pub release_sequence: u64,
    pub manifest: String,
    /// The package publisher, distinct from the catalog maintainer. A Merely Made catalog can
    /// therefore admit a foreign firmware recipe without claiming that Merely Made published it.
    pub firmware_publisher: String,
    pub state: CatalogState,
    pub instructions_url: String,
    pub recovery_url: String,
    pub installer_receipts: Vec<String>,
    pub recovery_receipts: Vec<String>,
    /// Host platforms with retained physical installer and recovery evidence.
    #[serde(default)]
    pub receipt_hosts: Vec<String>,
    pub purchase_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIndex {
    pub schema: String,
    pub publisher: String,
    pub version: String,
    pub packages: Vec<CatalogPackage>,
    pub publisher_signature: Option<CatalogSignature>,
}

/// An index whose signature was checked against owner-selected local trust.
///
/// Network fetch and staging APIs take this type, so a parsed but unauthenticated index
/// cannot cross that boundary by accident.
#[derive(Clone, Debug)]
pub struct AuthenticatedPackageIndex {
    index: PackageIndex,
    key_id: String,
}

impl AuthenticatedPackageIndex {
    pub fn index(&self) -> &PackageIndex {
        &self.index
    }

    pub fn key_id(&self) -> &str {
        &self.key_id
    }

    pub fn load_package(
        &self,
        index_path: impl AsRef<Path>,
        package_id: &str,
    ) -> Result<FlashPackage, CatalogError> {
        self.index.load_package(index_path, package_id)
    }
}

impl PackageIndex {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, CatalogError> {
        let text = fs::read_to_string(path)?;
        Ok(toml::from_str(&text)?)
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema != PACKAGE_INDEX_SCHEMA {
            return Err(CatalogError::Invalid(format!(
                "unsupported package-index schema {:?}",
                self.schema
            )));
        }
        for (name, value) in [
            ("publisher", self.publisher.as_str()),
            ("version", self.version.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(CatalogError::Invalid(format!("{name} cannot be empty")));
            }
        }
        if self.packages.is_empty() {
            return Err(CatalogError::Invalid(
                "packages cannot be empty".to_string(),
            ));
        }
        if let Some(signature) = &self.publisher_signature {
            if signature.key_id.trim().is_empty() || signature.signature.trim().is_empty() {
                return Err(CatalogError::Invalid(
                    "publisher_signature fields cannot be empty".to_string(),
                ));
            }
        }

        let mut package_ids = Vec::with_capacity(self.packages.len());
        for package in &self.packages {
            validate_catalog_package(package)?;
            if package_ids.iter().all(|id| id != &package.package_id) {
                package_ids.push(package.package_id.clone());
            } else {
                return Err(CatalogError::Invalid(format!(
                    "duplicate package_id {:?}",
                    package.package_id
                )));
            }
        }
        Ok(())
    }

    /// Canonical bytes signed by the catalog publisher.
    ///
    /// The signature field is excluded and a domain line prevents the same signature from
    /// authorizing another artifact type. TOML is serialized from the validated typed value,
    /// so insignificant source formatting is not part of the authority.
    pub fn signing_bytes(&self) -> Result<Vec<u8>, CatalogError> {
        let mut unsigned = self.clone();
        unsigned.publisher_signature = None;
        let canonical = toml::to_string(&unsigned)?;
        let mut bytes = Vec::with_capacity(CATALOG_SIGNING_DOMAIN.len() + canonical.len());
        bytes.extend_from_slice(CATALOG_SIGNING_DOMAIN.as_bytes());
        bytes.extend_from_slice(canonical.as_bytes());
        Ok(bytes)
    }

    /// Require a detached signature rooted in owner-selected local policy.
    pub fn authenticate(
        self,
        trust: &CatalogTrust,
    ) -> Result<AuthenticatedPackageIndex, CatalogError> {
        self.validate()?;
        trust.validate()?;
        let signature = self
            .publisher_signature
            .as_ref()
            .ok_or(CatalogError::MissingSignature)?;
        let trusted = trust
            .key(&self.publisher, &signature.key_id)
            .ok_or_else(|| CatalogError::UntrustedPublisher {
                publisher: self.publisher.clone(),
                key_id: signature.key_id.clone(),
            })?;
        let bytes = self.signing_bytes()?;
        verify_minisign(&bytes, &trusted.public_key, &signature.signature)?;
        Ok(AuthenticatedPackageIndex {
            key_id: signature.key_id.clone(),
            index: self,
        })
    }

    pub fn load_authenticated(
        path: impl AsRef<Path>,
        trust: &CatalogTrust,
    ) -> Result<AuthenticatedPackageIndex, CatalogError> {
        Self::load(path)?.authenticate(trust)
    }

    /// Load and verify every manifest named by this index.
    pub fn verify_packages(&self, index_path: impl AsRef<Path>) -> Result<(), CatalogError> {
        self.validate()?;
        let index_path = index_path.as_ref();
        let parent = index_path.parent().unwrap_or_else(|| Path::new("."));
        for entry in &self.packages {
            let manifest_path = parent.join(&entry.manifest);
            let package =
                FlashPackage::load(&manifest_path).map_err(|source| CatalogError::Package {
                    package_id: entry.package_id.clone(),
                    source,
                })?;
            if package.manifest().package_id != entry.package_id {
                return Err(CatalogError::Invalid(format!(
                    "catalog package {:?} names manifest package {:?}",
                    entry.package_id,
                    package.manifest().package_id
                )));
            }
            if package.manifest().publisher != entry.firmware_publisher {
                return Err(CatalogError::Invalid(format!(
                    "catalog package {:?} names firmware publisher {:?}, but its manifest says {:?}",
                    entry.package_id,
                    entry.firmware_publisher,
                    package.manifest().publisher
                )));
            }
        }
        Ok(())
    }

    pub fn package(&self, package_id: &str) -> Option<&CatalogPackage> {
        self.packages
            .iter()
            .find(|package| package.package_id == package_id)
    }

    /// Load one package after validating the whole index and every manifest it names.
    pub fn load_package(
        &self,
        index_path: impl AsRef<Path>,
        package_id: &str,
    ) -> Result<FlashPackage, CatalogError> {
        let index_path = index_path.as_ref();
        self.verify_packages(index_path)?;
        let entry = self.package(package_id).ok_or_else(|| {
            CatalogError::Invalid(format!("package {package_id:?} is not in the index"))
        })?;
        let parent = index_path.parent().unwrap_or_else(|| Path::new("."));
        let manifest_path = parent.join(&entry.manifest);
        FlashPackage::load(&manifest_path).map_err(|source| CatalogError::Package {
            package_id: entry.package_id.clone(),
            source,
        })
    }

    pub fn describe(&self) -> String {
        let mut output = format!(
            "package index {} publisher={} version={} packages={}\n",
            self.schema,
            self.publisher,
            self.version,
            self.packages.len()
        );
        for package in &self.packages {
            output.push_str(&format!(
                "- {} publisher={} state={} installer_receipts={} recovery_receipts={} receipt_hosts={}\n",
                package.package_id,
                package.firmware_publisher,
                package.state.label(),
                package.installer_receipts.len(),
                package.recovery_receipts.len(),
                package.receipt_hosts.len(),
            ));
        }
        output
    }
}

fn verify_minisign(bytes: &[u8], public_key: &str, signature: &str) -> Result<(), CatalogError> {
    let public_key = PublicKey::from_base64(public_key)
        .map_err(|error| CatalogError::Signature(error.to_string()))?;
    let signature =
        Signature::decode(signature).map_err(|error| CatalogError::Signature(error.to_string()))?;
    public_key
        .verify(bytes, &signature, false)
        .map_err(|error| CatalogError::Signature(error.to_string()))
}

fn validate_catalog_package(package: &CatalogPackage) -> Result<(), CatalogError> {
    for (name, value) in [
        ("package_id", package.package_id.as_str()),
        ("manifest", package.manifest.as_str()),
        ("firmware_publisher", package.firmware_publisher.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(CatalogError::Invalid(format!(
                "package {name} cannot be empty"
            )));
        }
    }
    if package.release_sequence == 0 {
        return Err(CatalogError::Invalid(format!(
            "package {:?} release_sequence must be positive",
            package.package_id
        )));
    }
    if Path::new(&package.manifest).is_absolute()
        || Path::new(&package.manifest)
            .components()
            .any(|component| component == Component::ParentDir)
    {
        return Err(CatalogError::Invalid(format!(
            "package {:?} manifest must stay below the index directory",
            package.package_id
        )));
    }
    for (name, value) in [
        ("instructions_url", package.instructions_url.as_str()),
        ("recovery_url", package.recovery_url.as_str()),
    ] {
        validate_url(name, value)?;
    }

    match package.state {
        CatalogState::Partial => {
            if !package.installer_receipts.is_empty()
                || !package.recovery_receipts.is_empty()
                || !package.receipt_hosts.is_empty()
            {
                return Err(CatalogError::Invalid(format!(
                    "partial package {:?} cannot claim receipt evidence",
                    package.package_id
                )));
            }
            if package.purchase_url.is_some() {
                return Err(CatalogError::Invalid(format!(
                    "partial package {:?} cannot have a purchase_url",
                    package.package_id
                )));
            }
        }
        CatalogState::ProvenRecipe => {
            validate_receipts(package)?;
            if package.purchase_url.is_some() {
                return Err(CatalogError::Invalid(format!(
                    "proven-recipe package {:?} cannot have a purchase_url",
                    package.package_id
                )));
            }
        }
        CatalogState::Sellable => {
            validate_receipts(package)?;
            let purchase_url = package.purchase_url.as_deref().ok_or_else(|| {
                CatalogError::Invalid(format!(
                    "sellable package {:?} needs a purchase_url",
                    package.package_id
                ))
            })?;
            validate_url("purchase_url", purchase_url)?;
        }
    }
    Ok(())
}

fn validate_receipts(package: &CatalogPackage) -> Result<(), CatalogError> {
    if package.installer_receipts.is_empty()
        || package.recovery_receipts.is_empty()
        || package.receipt_hosts.is_empty()
    {
        return Err(CatalogError::Invalid(format!(
            "package {:?} needs installer, recovery, and host receipt evidence before promotion",
            package.package_id
        )));
    }
    for receipt in package
        .installer_receipts
        .iter()
        .chain(package.recovery_receipts.iter())
    {
        validate_url("receipt", receipt)?;
    }
    let mut receipt_hosts = Vec::with_capacity(package.receipt_hosts.len());
    for host in &package.receipt_hosts {
        if ![
            "windows-x86_64",
            "macos-x86_64",
            "macos-aarch64",
            "linux-x86_64",
            "linux-aarch64",
        ]
        .contains(&host.as_str())
        {
            return Err(CatalogError::Invalid(format!(
                "package {:?} has unsupported receipt host {:?}",
                package.package_id, host
            )));
        }
        if receipt_hosts.iter().any(|known| *known == host) {
            return Err(CatalogError::Invalid(format!(
                "package {:?} repeats receipt host {:?}",
                package.package_id, host
            )));
        }
        receipt_hosts.push(host);
    }
    Ok(())
}

fn validate_url(name: &str, value: &str) -> Result<(), CatalogError> {
    if !value.starts_with("https://") || value.len() <= "https://".len() {
        return Err(CatalogError::Invalid(format!(
            "{name} must be a nonempty https URL"
        )));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Parse(#[from] toml::de::Error),
    #[error(transparent)]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid package index: {0}")]
    Invalid(String),
    #[error("package index has no publisher signature")]
    MissingSignature,
    #[error("catalog publisher {publisher:?} key {key_id:?} is not locally trusted")]
    UntrustedPublisher { publisher: String, key_id: String },
    #[error("catalog signature failed: {0}")]
    Signature(String),
    #[error("package {package_id:?} is invalid: {source}")]
    Package {
        package_id: String,
        #[source]
        source: PackageError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(state: CatalogState) -> CatalogPackage {
        CatalogPackage {
            package_id: "retinue.test".into(),
            release_sequence: 1,
            manifest: "test.toml".into(),
            firmware_publisher: "Merely Made".into(),
            state,
            instructions_url: "https://example.com/install".into(),
            recovery_url: "https://example.com/recover".into(),
            installer_receipts: Vec::new(),
            recovery_receipts: Vec::new(),
            receipt_hosts: Vec::new(),
            purchase_url: None,
        }
    }

    fn index(package: CatalogPackage) -> PackageIndex {
        PackageIndex {
            schema: PACKAGE_INDEX_SCHEMA.into(),
            publisher: "Merely Made".into(),
            version: "1".into(),
            packages: vec![package],
            publisher_signature: None,
        }
    }

    const TEST_PUBLIC_KEY: &str = "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
    const TEST_SIGNATURE: &str = "untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1556193335\tfile:test
y/rUw2y8/hOUYjZU71eHp/Wo1KZ40fGy2VJEDl34XMJM+TX48Ss/17u3IvIfbVR1FkZZSNCisQbuQY+bHwhEBg==";

    #[test]
    fn partial_catalog_is_valid_without_receipts() {
        assert!(index(package(CatalogState::Partial)).validate().is_ok());
    }

    #[test]
    fn proven_recipe_requires_both_receipt_kinds() {
        let mut value = package(CatalogState::ProvenRecipe);
        value
            .installer_receipts
            .push("https://example.com/install-receipt".into());
        assert!(index(value).validate().is_err());
    }

    #[test]
    fn sellable_catalog_requires_purchase_link_and_receipts() {
        let mut value = package(CatalogState::Sellable);
        value
            .installer_receipts
            .push("https://example.com/install-receipt".into());
        value
            .recovery_receipts
            .push("https://example.com/recovery-receipt".into());
        value.receipt_hosts.push("windows-x86_64".into());
        assert!(index(value.clone()).validate().is_err());
        value.purchase_url = Some("https://example.com/buy".into());
        assert!(index(value).validate().is_ok());
    }

    #[test]
    fn retained_public_index_promotes_only_receipted_packages() {
        let index: PackageIndex =
            toml::from_str(include_str!("../../../firmware/packages/index.toml"))
                .expect("parse retained public package index");
        index
            .validate()
            .expect("validate retained public package index");

        let package = |package_id| {
            index
                .packages
                .iter()
                .find(|package| package.package_id == package_id)
                .expect("named package")
        };
        assert_eq!(
            package("retinue.heltec-v4").state,
            CatalogState::ProvenRecipe
        );
        assert_eq!(
            package("retinue.heltec-v4").receipt_hosts,
            [
                "windows-x86_64",
                "macos-x86_64",
                "macos-aarch64",
                "linux-x86_64",
            ]
        );
        assert_eq!(package("retinue.t114").state, CatalogState::ProvenRecipe);
        assert_eq!(
            package("meshtastic.heltec-mesh-node-t114").state,
            CatalogState::Partial
        );
        assert!(
            package("meshtastic.heltec-mesh-node-t114")
                .installer_receipts
                .is_empty()
        );
        assert_eq!(
            package("prns.hopspot.heltec-v4").state,
            CatalogState::ProvenRecipe
        );
    }

    #[test]
    fn minisign_verifier_accepts_the_retained_vector_and_rejects_changed_bytes() {
        verify_minisign(b"test", TEST_PUBLIC_KEY, TEST_SIGNATURE).unwrap();
        assert!(verify_minisign(b"Test", TEST_PUBLIC_KEY, TEST_SIGNATURE).is_err());
    }

    #[test]
    fn signing_bytes_exclude_the_signature_and_are_domain_separated() {
        let mut index = index(package(CatalogState::Partial));
        let unsigned = index.signing_bytes().unwrap();
        index.publisher_signature = Some(CatalogSignature {
            format: CatalogSignatureFormat::Minisign,
            key_id: "test-key".into(),
            signature: TEST_SIGNATURE.into(),
        });
        assert_eq!(index.signing_bytes().unwrap(), unsigned);
        assert!(unsigned.starts_with(CATALOG_SIGNING_DOMAIN.as_bytes()));
    }

    #[test]
    fn parsed_catalog_is_not_authenticated_without_a_signature_or_local_trust() {
        let trust = CatalogTrust {
            schema: CATALOG_TRUST_SCHEMA.into(),
            keys: vec![TrustedCatalogKey {
                publisher: "Merely Made".into(),
                key_id: "test-key".into(),
                public_key: TEST_PUBLIC_KEY.into(),
            }],
        };
        assert!(matches!(
            index(package(CatalogState::Partial)).authenticate(&trust),
            Err(CatalogError::MissingSignature)
        ));

        let mut signed = index(package(CatalogState::Partial));
        signed.publisher_signature = Some(CatalogSignature {
            format: CatalogSignatureFormat::Minisign,
            key_id: "other-key".into(),
            signature: TEST_SIGNATURE.into(),
        });
        assert!(matches!(
            signed.authenticate(&trust),
            Err(CatalogError::UntrustedPublisher { .. })
        ));
    }
}
