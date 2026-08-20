//! Authenticated staging and activation state, before any OTA bearer exists.
//!
//! Bluetooth, Wi-Fi, LoRa, USB, and file copy are delivery mechanisms. None of
//! them decides whether bytes are authorized, newer, complete, safe to boot, or
//! confirmed. This module owns those decisions without pretending both current
//! targets have the same bootloader capability.

use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::catalog::{AuthenticatedPackageIndex, CatalogError};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationMode {
    /// An inactive application slot can be selected for one trial boot, then
    /// confirmed or rolled back without an external programmer.
    DualSlotRollback,
    /// The current bootloader can recover an application, but cannot promise an
    /// atomic trial boot and autonomous rollback.
    ExternalRecoveryOnly,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseIdentity {
    pub package_id: String,
    pub version: String,
    pub expected_application: String,
    pub release_sequence: u64,
    /// SHA-256 over the ordered, length-delimited verified package parts.
    pub payload_set_sha256: String,
}

/// Derive a stageable release only after catalog authority and every package
/// payload digest have both been verified.
pub fn authenticated_release(
    catalog: &AuthenticatedPackageIndex,
    index_path: impl AsRef<Path>,
    package_id: &str,
) -> Result<ReleaseIdentity, UpdateError> {
    let entry = catalog
        .index()
        .package(package_id)
        .ok_or_else(|| UpdateError::UnknownPackage(package_id.into()))?;
    let package = catalog.load_package(index_path, package_id)?;
    let mut digest = Sha256::new();
    for part in package.parts() {
        let bytes = part.bytes();
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    Ok(ReleaseIdentity {
        package_id: package.manifest().package_id.clone(),
        version: package.manifest().version.clone(),
        expected_application: package.manifest().expected_application.version.clone(),
        release_sequence: entry.release_sequence,
        payload_set_sha256: format!("{:x}", digest.finalize()),
    })
}

/// Persistable authority state. A bearer may fill `staged`; only a rollback-capable
/// activator may move it into `running` without outside recovery.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateJournal {
    pub mode: ActivationMode,
    confirmed: ReleaseIdentity,
    running: ReleaseIdentity,
    staged: Option<ReleaseIdentity>,
    rollback: Option<ReleaseIdentity>,
}

impl UpdateJournal {
    pub fn new(mode: ActivationMode, confirmed: ReleaseIdentity) -> Self {
        Self {
            mode,
            running: confirmed.clone(),
            confirmed,
            staged: None,
            rollback: None,
        }
    }

    pub fn confirmed(&self) -> &ReleaseIdentity {
        &self.confirmed
    }

    pub fn running(&self) -> &ReleaseIdentity {
        &self.running
    }

    pub fn staged(&self) -> Option<&ReleaseIdentity> {
        self.staged.as_ref()
    }

    pub fn awaiting_confirmation(&self) -> bool {
        self.rollback.is_some()
    }

    /// Admit verified bytes to the inactive staging area.
    pub fn stage(&mut self, candidate: ReleaseIdentity) -> Result<(), UpdateError> {
        if self.awaiting_confirmation() {
            return Err(UpdateError::TrialBootPending);
        }
        if candidate.package_id != self.confirmed.package_id {
            return Err(UpdateError::WrongPackage {
                installed: self.confirmed.package_id.clone(),
                candidate: candidate.package_id,
            });
        }
        let floor = self
            .staged
            .as_ref()
            .map(|staged| staged.release_sequence)
            .unwrap_or(self.confirmed.release_sequence);
        if candidate.release_sequence <= floor {
            return Err(UpdateError::RollbackRefused {
                installed_or_staged: floor,
                candidate: candidate.release_sequence,
            });
        }
        self.staged = Some(candidate);
        Ok(())
    }

    /// Select the staged image for a trial boot.
    pub fn activate_trial(&mut self) -> Result<&ReleaseIdentity, UpdateError> {
        if self.mode != ActivationMode::DualSlotRollback {
            return Err(UpdateError::ExternalRecoveryRequired);
        }
        if self.awaiting_confirmation() {
            return Err(UpdateError::TrialBootPending);
        }
        let candidate = self.staged.take().ok_or(UpdateError::NothingStaged)?;
        self.rollback = Some(self.confirmed.clone());
        self.running = candidate;
        Ok(&self.running)
    }

    /// Confirm only the exact application identity selected for the trial boot.
    pub fn confirm(&mut self, observed_application: &str) -> Result<(), UpdateError> {
        if !self.awaiting_confirmation() {
            return Err(UpdateError::NoTrialBoot);
        }
        if observed_application != self.running.expected_application {
            return Err(UpdateError::WrongApplication {
                expected: self.running.expected_application.clone(),
                observed: observed_application.into(),
            });
        }
        self.confirmed = self.running.clone();
        self.rollback = None;
        Ok(())
    }

    /// Roll an unconfirmed trial back to the last confirmed image.
    pub fn rollback_unconfirmed(&mut self) -> Result<&ReleaseIdentity, UpdateError> {
        let previous = self.rollback.take().ok_or(UpdateError::NoTrialBoot)?;
        self.running = previous.clone();
        self.confirmed = previous;
        Ok(&self.running)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("package {0:?} is not in the authenticated catalog")]
    UnknownPackage(String),
    #[error("update for {candidate:?} cannot replace installed package {installed:?}")]
    WrongPackage {
        installed: String,
        candidate: String,
    },
    #[error(
        "release sequence {candidate} is not newer than installed or staged sequence {installed_or_staged}"
    )]
    RollbackRefused {
        installed_or_staged: u64,
        candidate: u64,
    },
    #[error("an unconfirmed trial boot is already active")]
    TrialBootPending,
    #[error("nothing is staged")]
    NothingStaged,
    #[error("this target requires an external recovery path for activation")]
    ExternalRecoveryRequired,
    #[error("there is no unconfirmed trial boot")]
    NoTrialBoot,
    #[error("trial boot returned application {observed:?}, expected {expected:?}")]
    WrongApplication { expected: String, observed: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(sequence: u64) -> ReleaseIdentity {
        ReleaseIdentity {
            package_id: "retinue.test".into(),
            version: format!("v{sequence}"),
            expected_application: format!("app-v{sequence}"),
            release_sequence: sequence,
            payload_set_sha256: format!("{sequence:064x}"),
        }
    }

    #[test]
    fn dual_slot_trial_confirms_only_the_returned_application() {
        let mut journal = UpdateJournal::new(ActivationMode::DualSlotRollback, release(4));
        journal.stage(release(5)).unwrap();
        assert_eq!(journal.activate_trial().unwrap().release_sequence, 5);
        assert!(matches!(
            journal.confirm("some-other-app"),
            Err(UpdateError::WrongApplication { .. })
        ));
        journal.confirm("app-v5").unwrap();
        assert_eq!(journal.confirmed().release_sequence, 5);
        assert!(!journal.awaiting_confirmation());
    }

    #[test]
    fn failed_trial_returns_to_the_last_confirmed_release() {
        let mut journal = UpdateJournal::new(ActivationMode::DualSlotRollback, release(8));
        journal.stage(release(9)).unwrap();
        journal.activate_trial().unwrap();
        assert_eq!(journal.rollback_unconfirmed().unwrap().release_sequence, 8);
        assert_eq!(journal.running(), journal.confirmed());
    }

    #[test]
    fn rollback_and_cross_package_staging_are_refused() {
        let mut journal = UpdateJournal::new(ActivationMode::DualSlotRollback, release(10));
        assert!(matches!(
            journal.stage(release(9)),
            Err(UpdateError::RollbackRefused { .. })
        ));
        let mut foreign = release(11);
        foreign.package_id = "somebody.else".into();
        assert!(matches!(
            journal.stage(foreign),
            Err(UpdateError::WrongPackage { .. })
        ));
    }

    #[test]
    fn current_t114_shape_can_stage_but_cannot_claim_safe_activation() {
        let mut journal = UpdateJournal::new(ActivationMode::ExternalRecoveryOnly, release(51));
        journal.stage(release(52)).unwrap();
        assert!(matches!(
            journal.activate_trial(),
            Err(UpdateError::ExternalRecoveryRequired)
        ));
        assert_eq!(journal.running().release_sequence, 51);
        assert_eq!(journal.staged().unwrap().release_sequence, 52);
    }
}
