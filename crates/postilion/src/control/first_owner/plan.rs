//! The owner-selected public configuration and recovery policy.

use seneschal::control::{
    ManagementCarrier, ManagementCarrierSet, PublicConfigurationV1, RecoveryClause, RecoveryPolicy,
    RecoveryPolicyError, ReticulumTransportPolicy,
};
use seneschal::region::Region;
use tulle::PhyProfile;

/// Public configuration and recovery policy selected by the owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClaimPlan {
    public_configuration: PublicConfigurationV1,
    recovery_policy: RecoveryPolicy,
}

impl ClaimPlan {
    pub const fn new(
        public_configuration: PublicConfigurationV1,
        recovery_policy: RecoveryPolicy,
    ) -> Self {
        Self {
            public_configuration,
            recovery_policy,
        }
    }

    pub const fn public_configuration(self) -> PublicConfigurationV1 {
        self.public_configuration
    }

    pub const fn recovery_policy(self) -> RecoveryPolicy {
        self.recovery_policy
    }
}

/// Constructs the only truthful first-owner plan the current V4 USB carrier can make.
///
/// It enables USB management only, establishes one physical-presence USB recovery clause,
/// leaves authenticated remote recovery disabled, provisions no credentials, and makes this
/// first configuration a non-relay Reticulum transport.
pub fn v4_usb_claim_plan(region: Region, phy: PhyProfile) -> Result<ClaimPlan, V4UsbPlanError> {
    let carriers = ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8)
        .expect("the USB management bit is defined and non-empty");
    let transport = ReticulumTransportPolicy::new(false, false, 0)
        .expect("a non-relay, zero-hop policy is canonical");
    let public = PublicConfigurationV1::new(region, phy, transport, carriers)
        .map_err(V4UsbPlanError::Configuration)?;
    let physical = RecoveryClause::new(carriers, 1).expect("one USB survivor is canonical");
    let policy = RecoveryPolicy::new(physical, RecoveryClause::disabled())
        .map_err(V4UsbPlanError::Recovery)?;
    Ok(ClaimPlan::new(public, policy))
}

/// A V4 public claim-plan input was invalid before it touched a board.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum V4UsbPlanError {
    #[error("invalid V4 public configuration: {0:?}")]
    Configuration(seneschal::control::PublicConfigurationError),
    #[error("invalid V4 USB recovery policy: {0:?}")]
    Recovery(RecoveryPolicyError),
}
