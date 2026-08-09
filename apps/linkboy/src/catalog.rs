//! Public package-index metadata for firmware packages.
//!
//! The catalog is deliberately a separate authority from a package manifest. A manifest says
//! what Linkboy can safely install; this index says whether that package has public installer
//! and recovery evidence, and whether it is ready to be offered for purchase.

use std::fs;
use std::io;
use std::path::{Component, Path};

use serde::{Deserialize, Serialize};

use crate::package::{FlashPackage, PackageError};

pub const PACKAGE_INDEX_SCHEMA: &str = "retinue.package-index/v1";

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
    pub manifest: String,
    pub state: CatalogState,
    pub instructions_url: String,
    pub recovery_url: String,
    pub installer_receipts: Vec<String>,
    pub recovery_receipts: Vec<String>,
    pub purchase_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageIndex {
    pub schema: String,
    pub publisher: String,
    pub version: String,
    pub packages: Vec<CatalogPackage>,
    pub publisher_signature: Option<String>,
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
            if signature.trim().is_empty() {
                return Err(CatalogError::Invalid(
                    "publisher_signature cannot be empty".to_string(),
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
            if package.manifest().publisher != self.publisher {
                return Err(CatalogError::Invalid(format!(
                    "package {:?} has publisher {:?}, expected {:?}",
                    entry.package_id,
                    package.manifest().publisher,
                    self.publisher
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
                "- {} state={} installer_receipts={} recovery_receipts={}\n",
                package.package_id,
                package.state.label(),
                package.installer_receipts.len(),
                package.recovery_receipts.len()
            ));
        }
        output
    }
}

fn validate_catalog_package(package: &CatalogPackage) -> Result<(), CatalogError> {
    for (name, value) in [
        ("package_id", package.package_id.as_str()),
        ("manifest", package.manifest.as_str()),
    ] {
        if value.trim().is_empty() {
            return Err(CatalogError::Invalid(format!(
                "package {name} cannot be empty"
            )));
        }
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
            if !package.installer_receipts.is_empty() || !package.recovery_receipts.is_empty() {
                return Err(CatalogError::Invalid(format!(
                    "partial package {:?} cannot claim receipts",
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
    if package.installer_receipts.is_empty() || package.recovery_receipts.is_empty() {
        return Err(CatalogError::Invalid(format!(
            "package {:?} needs installer and recovery receipts before promotion",
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
    #[error("invalid package index: {0}")]
    Invalid(String),
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
            manifest: "test.toml".into(),
            state,
            instructions_url: "https://example.com/install".into(),
            recovery_url: "https://example.com/recover".into(),
            installer_receipts: Vec::new(),
            recovery_receipts: Vec::new(),
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
        assert!(index(value.clone()).validate().is_err());
        value.purchase_url = Some("https://example.com/buy".into());
        assert!(index(value).validate().is_ok());
    }
}
