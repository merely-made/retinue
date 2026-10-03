//! Durable recovery policy and firmware-owned board facts.

use heapless::Vec;

use crate::control::{
    MAX_RECOVERY_PATHS, ManagementCarrier, ManagementCarrierSet, PublicConfigurationV1,
};

use super::{DurableConfig, DurableError, DurableState, Refusal};

const KNOWN_MASK: u8 = (1 << (ManagementCarrier::Usb as u8))
    | (1 << (ManagementCarrier::Ble as u8))
    | (1 << (ManagementCarrier::Ip as u8))
    | (1 << (ManagementCarrier::Reticulum as u8));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryPolicyError {
    Empty,
    BadQuorum,
    NonCanonicalDisabled,
    UnknownCarrier,
    DuplicateCarrier,
    AuthenticationWithoutRemote,
}

/// One independently-enforced recovery clause. A disabled clause is exactly
/// `mask = 0, minimum_survivors = 0`; its fields cannot carry a latent policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryClause {
    mask: u8,
    minimum_survivors: u8,
}

impl RecoveryClause {
    pub const fn disabled() -> Self {
        Self {
            mask: 0,
            minimum_survivors: 0,
        }
    }

    pub const fn new(
        acceptable: ManagementCarrierSet,
        minimum_survivors: u8,
    ) -> Result<Self, RecoveryPolicyError> {
        let mask = acceptable.mask();
        if minimum_survivors == 0 || minimum_survivors > mask.count_ones() as u8 {
            return Err(RecoveryPolicyError::BadQuorum);
        }
        Ok(Self {
            mask,
            minimum_survivors,
        })
    }

    pub const fn is_active(self) -> bool {
        self.mask != 0
    }
    pub const fn minimum_survivors(self) -> u8 {
        self.minimum_survivors
    }
    pub const fn acceptable_mask(self) -> u8 {
        self.mask
    }

    pub(super) const fn from_parts(
        mask: u8,
        minimum_survivors: u8,
    ) -> Result<Self, RecoveryPolicyError> {
        if mask & !KNOWN_MASK != 0 {
            return Err(RecoveryPolicyError::UnknownCarrier);
        }
        if mask == 0 {
            return if minimum_survivors == 0 {
                Ok(Self::disabled())
            } else {
                Err(RecoveryPolicyError::NonCanonicalDisabled)
            };
        }
        if minimum_survivors == 0 || minimum_survivors > mask.count_ones() as u8 {
            return Err(RecoveryPolicyError::BadQuorum);
        }
        Ok(Self {
            mask,
            minimum_survivors,
        })
    }

    fn enabled_count(self, public: PublicConfigurationV1) -> u8 {
        (self.mask & public.enabled_management_carriers().mask()).count_ones() as u8
    }
}

/// Owner-selected recovery invariant. Both clauses, when active, must survive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPolicy {
    physical_presence: RecoveryClause,
    authenticated_remote: RecoveryClause,
}

impl RecoveryPolicy {
    pub const fn new(
        physical_presence: RecoveryClause,
        authenticated_remote: RecoveryClause,
    ) -> Result<Self, RecoveryPolicyError> {
        if !physical_presence.is_active() && !authenticated_remote.is_active() {
            return Err(RecoveryPolicyError::Empty);
        }
        Ok(Self {
            physical_presence,
            authenticated_remote,
        })
    }
    pub const fn physical_presence(self) -> RecoveryClause {
        self.physical_presence
    }
    pub const fn authenticated_remote(self) -> RecoveryClause {
        self.authenticated_remote
    }

    /// Decodes the four fixed recovery-policy bytes used by first-owner claim
    /// transport.  This keeps that carrier format strict without making its
    /// fields public or accepting a non-canonical disabled clause.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, RecoveryPolicyError> {
        let bytes: &[u8; 4] = bytes.try_into().map_err(|_| RecoveryPolicyError::Empty)?;
        Self::new(
            RecoveryClause::from_parts(bytes[0], bytes[1])?,
            RecoveryClause::from_parts(bytes[2], bytes[3])?,
        )
    }

    /// Revalidates the policy's canonical clause structure.
    pub fn validate(self) -> Result<(), RecoveryPolicyError> {
        Self::new(
            RecoveryClause::from_parts(
                self.physical_presence.mask,
                self.physical_presence.minimum_survivors,
            )?,
            RecoveryClause::from_parts(
                self.authenticated_remote.mask,
                self.authenticated_remote.minimum_survivors,
            )?,
        )?;
        Ok(())
    }

    pub(super) fn validate_structure(self) -> Result<(), DurableError> {
        self.validate().map_err(|_| DurableError::Malformed)
    }

    pub(super) fn configuration_satisfies(self, configuration: &DurableConfig) -> bool {
        (!self.physical_presence.is_active()
            || self.physical_presence.enabled_count(configuration.public)
                >= self.physical_presence.minimum_survivors)
            && (!self.authenticated_remote.is_active()
                || self
                    .authenticated_remote
                    .enabled_count(configuration.public)
                    >= self.authenticated_remote.minimum_survivors)
    }

    pub(super) fn facts_satisfy(
        self,
        configuration: &DurableConfig,
        facts: &BoardRecoveryFacts,
    ) -> bool {
        facts.count(
            configuration.public,
            self.physical_presence,
            RecoveryKind::Physical,
        ) >= self.physical_presence.minimum_survivors
            && facts.count(
                configuration.public,
                self.authenticated_remote,
                RecoveryKind::RemoteAuthenticated,
            ) >= self.authenticated_remote.minimum_survivors
    }

    pub(super) const fn encode_parts(self) -> ((u8, u8), (u8, u8)) {
        (
            (
                self.physical_presence.mask,
                self.physical_presence.minimum_survivors,
            ),
            (
                self.authenticated_remote.mask,
                self.authenticated_remote.minimum_survivors,
            ),
        )
    }
    pub(super) fn decode_parts(physical: (u8, u8), remote: (u8, u8)) -> Result<Self, DurableError> {
        Self::new(
            RecoveryClause::from_parts(physical.0, physical.1)
                .map_err(|_| DurableError::Malformed)?,
            RecoveryClause::from_parts(remote.0, remote.1).map_err(|_| DurableError::Malformed)?,
        )
        .map_err(|_| DurableError::Malformed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecoveryPathFacts {
    carrier: ManagementCarrier,
    supports_physical_presence: bool,
    supports_remote: bool,
    remote_is_authenticated: bool,
}

impl RecoveryPathFacts {
    pub const fn new(
        carrier: ManagementCarrier,
        supports_physical_presence: bool,
        supports_remote: bool,
        remote_is_authenticated: bool,
    ) -> Result<Self, RecoveryPolicyError> {
        if remote_is_authenticated && !supports_remote {
            return Err(RecoveryPolicyError::AuthenticationWithoutRemote);
        }
        Ok(Self {
            carrier,
            supports_physical_presence,
            supports_remote,
            remote_is_authenticated,
        })
    }
    pub const fn carrier(self) -> ManagementCarrier {
        self.carrier
    }
    pub const fn supports_physical_presence(self) -> bool {
        self.supports_physical_presence
    }
    pub const fn supports_remote(self) -> bool {
        self.supports_remote
    }
    pub const fn remote_is_authenticated(self) -> bool {
        self.remote_is_authenticated
    }
}

/// Trusted platform facts supplied by firmware's board profile, never by WN0/RHC0.
/// Downstream firmware constructs this from board capabilities before creating the
/// runtime; no carrier decoder or configuration candidate converts into it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoardRecoveryFacts {
    paths: Vec<RecoveryPathFacts, MAX_RECOVERY_PATHS>,
}

impl BoardRecoveryFacts {
    pub fn new(
        paths: Vec<RecoveryPathFacts, MAX_RECOVERY_PATHS>,
    ) -> Result<Self, RecoveryPolicyError> {
        for (index, path) in paths.iter().enumerate() {
            if paths[..index]
                .iter()
                .any(|prior| prior.carrier == path.carrier)
            {
                return Err(RecoveryPolicyError::DuplicateCarrier);
            }
            if path.remote_is_authenticated && !path.supports_remote {
                return Err(RecoveryPolicyError::AuthenticationWithoutRemote);
            }
        }
        Ok(Self { paths })
    }
    pub fn paths(&self) -> &[RecoveryPathFacts] {
        &self.paths
    }

    fn count(
        &self,
        public: PublicConfigurationV1,
        clause: RecoveryClause,
        kind: RecoveryKind,
    ) -> u8 {
        if !clause.is_active() {
            return 0;
        }
        self.paths
            .iter()
            .filter(|fact| {
                public.enabled_management_carriers().contains(fact.carrier)
                    && clause.mask & (1 << (fact.carrier as u8)) != 0
                    && match kind {
                        RecoveryKind::Physical => fact.supports_physical_presence,
                        RecoveryKind::RemoteAuthenticated => {
                            fact.supports_remote && fact.remote_is_authenticated
                        }
                    }
            })
            .count() as u8
    }
}

#[derive(Clone, Copy)]
enum RecoveryKind {
    Physical,
    RemoteAuthenticated,
}

pub(super) fn validate_policy_candidate(
    policy: RecoveryPolicy,
    candidate: &DurableConfig,
    facts: &BoardRecoveryFacts,
) -> Result<(), Refusal> {
    if policy.configuration_satisfies(candidate) && policy.facts_satisfy(candidate, facts) {
        Ok(())
    } else {
        Err(Refusal::UnsafeRecoveryPath)
    }
}

impl DurableState {
    /// Checks the durable owner policy against firmware's immutable board profile.
    /// This runs before boot applies either known-good or a rollback configuration.
    pub fn validate_recovery_facts(&self, facts: &BoardRecoveryFacts) -> Result<(), DurableError> {
        if !self
            .recovery_policy
            .facts_satisfy(&self.known_good.configuration, facts)
            || self
                .provisional
                .as_ref()
                .is_some_and(|value| !self.recovery_policy.facts_satisfy(&value.candidate, facts))
        {
            return Err(DurableError::Malformed);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{PublicConfigurationV1, ReticulumTransportPolicy};
    use crate::region::Region;

    fn public(mask: u8) -> PublicConfigurationV1 {
        PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(mask).unwrap(),
        )
        .unwrap()
    }
    fn config(mask: u8) -> DurableConfig {
        DurableConfig {
            public: public(mask),
            sealed_credentials: Vec::new(),
        }
    }

    #[test]
    fn clauses_are_canonical_and_both_active_clauses_are_independent() {
        assert_eq!(
            RecoveryClause::from_parts(0, 1),
            Err(RecoveryPolicyError::NonCanonicalDisabled)
        );
        assert_eq!(
            RecoveryClause::from_parts(1, 0),
            Err(RecoveryPolicyError::BadQuorum)
        );
        assert_eq!(
            RecoveryClause::from_parts(0x80, 1),
            Err(RecoveryPolicyError::UnknownCarrier)
        );
        let policy = RecoveryPolicy::new(
            RecoveryClause::new(ManagementCarrierSet::from_mask(0b0011).unwrap(), 1).unwrap(),
            RecoveryClause::new(ManagementCarrierSet::from_mask(0b1100).unwrap(), 1).unwrap(),
        )
        .unwrap();
        assert!(policy.configuration_satisfies(&config(0b0101)));
        assert!(!policy.configuration_satisfies(&config(0b0001)));
        assert!(!policy.configuration_satisfies(&config(0b0100)));
    }

    #[test]
    fn board_facts_reject_duplicates_and_unauthenticated_remote() {
        assert_eq!(
            RecoveryPathFacts::new(ManagementCarrier::Ip, false, false, true),
            Err(RecoveryPolicyError::AuthenticationWithoutRemote)
        );
        let usb = RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap();
        assert_eq!(
            BoardRecoveryFacts::new(Vec::from_slice(&[usb, usb]).unwrap()),
            Err(RecoveryPolicyError::DuplicateCarrier)
        );
    }

    #[test]
    fn remote_clause_needs_board_authenticated_remote_not_candidate_claims() {
        let policy = RecoveryPolicy::new(
            RecoveryClause::disabled(),
            RecoveryClause::new(ManagementCarrierSet::from_mask(0b0100).unwrap(), 1).unwrap(),
        )
        .unwrap();
        let candidate = config(0b0100);
        let bad = BoardRecoveryFacts::new(
            Vec::from_slice(&[
                RecoveryPathFacts::new(ManagementCarrier::Ip, false, true, false).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        let good = BoardRecoveryFacts::new(
            Vec::from_slice(&[
                RecoveryPathFacts::new(ManagementCarrier::Ip, false, true, true).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            validate_policy_candidate(policy, &candidate, &bad),
            Err(Refusal::UnsafeRecoveryPath)
        );
        assert_eq!(validate_policy_candidate(policy, &candidate, &good), Ok(()));
    }
}
