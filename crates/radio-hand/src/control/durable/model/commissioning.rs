//! Pure first-owner durable-state construction.
//!
//! This module has no carrier, storage, or physical-presence behavior. A later
//! claim-only admission path may validate an [`OwnerClaim`] and atomically persist
//! the returned state after it has established its own admission witness.

use heapless::Vec;

use crate::control::{
    PublicConfigurationError, RETINUE_PUBLIC_IDENTITY_LEN, validate_retinue_public_identity,
};

use super::{
    BoardRecoveryFacts, ConfigGeneration, ControllerRole, DurableConfig, DurableError,
    DurableLoadError, DurableState, JournalWrite, MAX_DURABLE_BODY, NodeId, OwnerGrant,
    PublicConfigurationV1, RecoveryPolicy, RecoveryPolicyError, Refusal, encode_durable, load,
    next_record,
};

/// Exact bytes in the canonical, signed public portion of a first-owner claim.
///
/// This is deliberately not an `RHD1` durable record.  It is the fixed public
/// claim carried by a small local carrier before the board derives and stages
/// the complete durable state.
pub const OWNER_CLAIM_LEN: usize =
    RETINUE_PUBLIC_IDENTITY_LEN + crate::control::PUBLIC_CONFIGURATION_V1_LEN + 4;

/// A bounded public request to establish exactly one initial owner.
///
/// It contains a Retinue public identity, the first portable public
/// configuration, and its recovery policy. Its owner role is implicit. It
/// deliberately has no node secret, private identity bytes, credentials,
/// carrier witness, or additional grants.
#[derive(Clone, PartialEq, Eq)]
pub struct OwnerClaim {
    owner_public_identity: [u8; RETINUE_PUBLIC_IDENTITY_LEN],
    public_configuration: PublicConfigurationV1,
    recovery_policy: RecoveryPolicy,
}

impl core::fmt::Debug for OwnerClaim {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("OwnerClaim")
            .field("owner_public_identity", &"[redacted]")
            .field("public_configuration", &self.public_configuration)
            .field("recovery_policy", &self.recovery_policy)
            .finish()
    }
}

/// Why a public initial-owner claim cannot become durable state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerClaimError {
    /// The submitted Retinue public identity is not exactly its canonical length.
    PublicIdentityLength,
    /// The submitted Retinue public identity cannot be parsed as a public identity.
    InvalidPublicIdentity,
    /// The requested portable public configuration is invalid or non-canonical.
    PublicConfiguration(PublicConfigurationError),
    /// The requested recovery policy has an invalid or non-canonical structure.
    RecoveryPolicy(RecoveryPolicyError),
    /// The requested recovery policy cannot be satisfied by the requested public configuration.
    UnsatisfiedRecoveryPolicy,
    /// Trusted board recovery facts cannot satisfy the requested recovery policy.
    UnsafeBoardRecoveryFacts,
    /// A validated claim violated a durable invariant during construction.
    DurableInvariant,
}

impl OwnerClaim {
    /// Validates a public, single-owner claim for a later carrier-specific admission path.
    pub fn new(
        owner_public_identity: &[u8],
        public_configuration: PublicConfigurationV1,
        recovery_policy: RecoveryPolicy,
    ) -> Result<Self, OwnerClaimError> {
        let owner_public_identity: [u8; RETINUE_PUBLIC_IDENTITY_LEN] = owner_public_identity
            .try_into()
            .map_err(|_| OwnerClaimError::PublicIdentityLength)?;
        validate_retinue_public_identity(&owner_public_identity)
            .map_err(|_| OwnerClaimError::InvalidPublicIdentity)?;
        public_configuration
            .validate()
            .map_err(OwnerClaimError::PublicConfiguration)?;
        recovery_policy
            .validate()
            .map_err(OwnerClaimError::RecoveryPolicy)?;
        let configuration = DurableConfig {
            public: public_configuration,
            sealed_credentials: Vec::new(),
        };
        if !recovery_policy.configuration_satisfies(&configuration) {
            return Err(OwnerClaimError::UnsatisfiedRecoveryPolicy);
        }
        Ok(Self {
            owner_public_identity,
            public_configuration,
            recovery_policy,
        })
    }

    /// The validated public owner identity. It contains no private key material.
    pub const fn owner_public_identity(&self) -> &[u8; RETINUE_PUBLIC_IDENTITY_LEN] {
        &self.owner_public_identity
    }

    /// The validated initial portable public configuration.
    pub const fn public_configuration(&self) -> PublicConfigurationV1 {
        self.public_configuration
    }

    /// The validated initial recovery policy.
    pub const fn recovery_policy(&self) -> RecoveryPolicy {
        self.recovery_policy
    }

    /// Writes the one canonical public claim representation used by the
    /// first-owner proof transcript.  It contains the entire 64-byte Retinue
    /// public identity, public configuration, and recovery policy.
    pub fn encode_canonical(&self, out: &mut [u8; OWNER_CLAIM_LEN]) {
        out[..RETINUE_PUBLIC_IDENTITY_LEN].copy_from_slice(&self.owner_public_identity);
        let configuration = self.public_configuration.encode();
        let config_end = RETINUE_PUBLIC_IDENTITY_LEN + configuration.len();
        out[RETINUE_PUBLIC_IDENTITY_LEN..config_end].copy_from_slice(&configuration);
        let (physical, remote) = self.recovery_policy.encode_parts();
        out[config_end..].copy_from_slice(&[physical.0, physical.1, remote.0, remote.1]);
    }

    /// Decodes only the exact canonical public claim representation.  Extra
    /// bytes are never silently ignored at this trust boundary.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, OwnerClaimError> {
        if bytes.len() != OWNER_CLAIM_LEN {
            return Err(OwnerClaimError::PublicIdentityLength);
        }
        let identity = &bytes[..RETINUE_PUBLIC_IDENTITY_LEN];
        let config_end = RETINUE_PUBLIC_IDENTITY_LEN + crate::control::PUBLIC_CONFIGURATION_V1_LEN;
        let configuration =
            PublicConfigurationV1::decode(&bytes[RETINUE_PUBLIC_IDENTITY_LEN..config_end])
                .map_err(OwnerClaimError::PublicConfiguration)?;
        let policy = RecoveryPolicy::decode_canonical(&bytes[config_end..])
            .map_err(OwnerClaimError::RecoveryPolicy)?;
        Self::new(identity, configuration, policy)
    }
}

impl DurableState {
    /// Creates the first durable state for this exact opaque node identifier.
    ///
    /// The result has one owner grant with both counters at zero, generation and
    /// watermark zero, no sealed credentials, no provisional transaction, and no
    /// cached receipt. Trusted board facts make the result safe to persist. This
    /// method is pure: it neither admits a carrier claim nor writes an A/B slot.
    pub fn from_owner_claim(
        node: NodeId,
        claim: OwnerClaim,
        facts: &BoardRecoveryFacts,
    ) -> Result<Self, OwnerClaimError> {
        let grant =
            OwnerGrant::from_public_identity(claim.owner_public_identity, ControllerRole::Owner);
        Self::new(
            node,
            Vec::from_slice(&[grant]).map_err(|_| OwnerClaimError::DurableInvariant)?,
            ConfigGeneration(0),
            DurableConfig {
                public: claim.public_configuration,
                sealed_credentials: Vec::new(),
            },
            claim.recovery_policy,
            facts,
        )
        .map_err(|error| match error {
            Refusal::UnsafeRecoveryPath => OwnerClaimError::UnsafeBoardRecoveryFacts,
            _ => OwnerClaimError::DurableInvariant,
        })
    }
}

/// Why a durable state cannot be staged as a first owner write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWriteError {
    /// The existing durable encoder rejected the state or could not encode it.
    Durable(DurableError),
    /// The staged state belongs to a different opaque control node.
    WrongNode,
    /// Current trusted board facts cannot satisfy the staged recovery policy.
    UnsafeBoardRecoveryFacts,
    /// First-write staging permits exactly one controller grant.
    OwnerGrantCount,
    /// The sole first-write grant must be an owner.
    InitialOwnerRequired,
    /// A first-write grant must not carry an accepted outer counter.
    OuterCounterNotZero,
    /// A first-write grant must not carry an accepted mutation sequence.
    MutationSequenceNotZero,
    /// The known-good configuration must begin at generation zero.
    GenerationNotZero,
    /// The generation watermark must begin at zero.
    WatermarkNotZero,
    /// Initial commissioning carries no sealed credential material.
    SealedCredentialsPresent,
    /// Initial commissioning has no armed configuration transaction.
    ProvisionalPresent,
    /// Initial commissioning has no cached terminal receipt.
    CachedReceiptPresent,
}

/// Result of reading one pending first-write A/B pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstWriteLoadError {
    /// Both outer A/B slots are erased.
    Blank,
    /// One or both slots are nonblank but cannot hold a canonical first-write state.
    Corrupt(FirstWriteError),
}

/// Internal classification of the separate commissioning first-write A/B pair.
#[derive(Debug, Clone, PartialEq, Eq)]
enum FirstWritePair {
    /// Both slots are erased.
    Blank,
    /// A complete, canonical initial state is staged for a later first write.
    Pending(DurableState),
    /// The pair is nonblank but invalid. It is never treated as blank.
    Corrupt,
}

/// Internal classification of the ordinary durable control A/B pair for boot arbitration.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ControlPair {
    /// A normal durable control state is present.
    Valid(DurableState),
    /// Both control slots are erased.
    Blank,
    /// The control pair is nonblank but has no valid durable state.
    Corrupt,
}

/// Pure outcome for commissioning boot arbitration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FirstWriteBoot {
    /// An ordinary durable control state is present and must continue through
    /// the normal control-runtime boot path.
    ControlPresent(DurableState),
    /// Neither pair holds state, so there is no owner yet.
    BlankUncommissioned,
    /// A canonical initial state is staged and must be handled by later first-write logic.
    FirstWritePending(DurableState),
    /// The available durable evidence is unsafe or incomplete.
    Fault,
}

/// Validates that a state is exactly the canonical first-owner durable state.
///
/// Existing durable semantic and Retinue public-identity validators run directly,
/// without a second MAX_DURABLE_BODY scratch buffer. This accepts no
/// normalization or repair of malformed state.
pub fn validate_first_write_state(state: &DurableState) -> Result<(), FirstWriteError> {
    state
        .validate_semantics()
        .map_err(FirstWriteError::Durable)?;
    if state.owner_grants().len() != 1 {
        return Err(FirstWriteError::OwnerGrantCount);
    }
    let grant = &state.owner_grants()[0];
    if grant.role() != ControllerRole::Owner {
        return Err(FirstWriteError::InitialOwnerRequired);
    }
    if grant.accepted_outer_counter() != 0 {
        return Err(FirstWriteError::OuterCounterNotZero);
    }
    if grant.accepted_mutation_sequence() != 0 {
        return Err(FirstWriteError::MutationSequenceNotZero);
    }
    if state.known_good().generation != ConfigGeneration(0) {
        return Err(FirstWriteError::GenerationNotZero);
    }
    if state.generation_watermark() != ConfigGeneration(0) {
        return Err(FirstWriteError::WatermarkNotZero);
    }
    if !state
        .known_good()
        .configuration
        .sealed_credentials
        .is_empty()
    {
        return Err(FirstWriteError::SealedCredentialsPresent);
    }
    if state.provisional().is_some() {
        return Err(FirstWriteError::ProvisionalPresent);
    }
    if state.receipt().is_some() {
        return Err(FirstWriteError::CachedReceiptPresent);
    }
    Ok(())
}

/// Rechecks a canonical staged state against current boot-specific facts before
/// it can be treated as resumable first-write work.
pub fn validate_resumable_first_write_state(
    state: &DurableState,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<(), FirstWriteError> {
    validate_first_write_state(state)?;
    if state.node() != expected_node {
        return Err(FirstWriteError::WrongNode);
    }
    state
        .validate_recovery_facts(facts)
        .map_err(|_| FirstWriteError::UnsafeBoardRecoveryFacts)
}

/// Encodes a validated first-write state with the existing canonical RHD1 codec.
pub fn encode_first_write_state(
    state: &DurableState,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
    out: &mut [u8],
) -> Result<usize, FirstWriteError> {
    validate_resumable_first_write_state(state, expected_node, facts)?;
    encode_durable(state, out).map_err(FirstWriteError::Durable)
}

/// Prepares the next generic A/B record for a validated first-write state.
///
/// This does not erase, program, or read back a board slot.
pub fn next_first_write_record(
    a: &[u8],
    b: &[u8],
    state: &DurableState,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
    body_scratch: &mut [u8; MAX_DURABLE_BODY],
    page_out: &mut [u8],
) -> Result<JournalWrite, FirstWriteError> {
    validate_resumable_first_write_state(state, expected_node, facts)?;
    next_record(a, b, state, body_scratch, page_out).map_err(FirstWriteError::Durable)
}

/// Loads a pending A/B pair only when it is canonical and resumable for this boot.
pub fn load_first_write_state(
    a: &[u8],
    b: &[u8],
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<DurableState, FirstWriteLoadError> {
    match load(a, b) {
        Ok(state) => validate_resumable_first_write_state(&state, expected_node, facts)
            .map(|()| state)
            .map_err(FirstWriteLoadError::Corrupt),
        Err(DurableLoadError::Blank) => Err(FirstWriteLoadError::Blank),
        Err(DurableLoadError::Corrupt) => Err(FirstWriteLoadError::Corrupt(
            FirstWriteError::Durable(DurableError::Malformed),
        )),
        Err(DurableLoadError::State(error)) => Err(FirstWriteLoadError::Corrupt(
            FirstWriteError::Durable(error),
        )),
    }
}

/// Classifies a pending first-write pair for this boot without repairing it.
fn classify_first_write_pair(
    a: &[u8],
    b: &[u8],
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> FirstWritePair {
    match load_first_write_state(a, b, expected_node, facts) {
        Ok(state) => FirstWritePair::Pending(state),
        Err(FirstWriteLoadError::Blank) => FirstWritePair::Blank,
        Err(FirstWriteLoadError::Corrupt(_)) => FirstWritePair::Corrupt,
    }
}

/// Classifies the ordinary control pair for pure first-write boot arbitration.
fn classify_control_pair(a: &[u8], b: &[u8]) -> ControlPair {
    match load(a, b) {
        Ok(state) => ControlPair::Valid(state),
        Err(DurableLoadError::Blank) => ControlPair::Blank,
        Err(DurableLoadError::Corrupt | DurableLoadError::State(_)) => ControlPair::Corrupt,
    }
}

/// Resolves raw control and pending A/B evidence without any storage or carrier action.
///
/// Pending state is classified with the expected node and current trusted board
/// recovery facts before it can become a [`FirstWriteBoot::FirstWritePending`] result.
pub fn arbitrate_first_write(
    control_a: &[u8],
    control_b: &[u8],
    pending_a: &[u8],
    pending_b: &[u8],
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> FirstWriteBoot {
    arbitrate_classified_first_write(
        classify_control_pair(control_a, control_b),
        classify_first_write_pair(pending_a, pending_b, expected_node, facts),
    )
}

fn arbitrate_classified_first_write(
    control: ControlPair,
    pending: FirstWritePair,
) -> FirstWriteBoot {
    match control {
        ControlPair::Valid(state) => FirstWriteBoot::ControlPresent(state),
        ControlPair::Blank => match pending {
            FirstWritePair::Blank => FirstWriteBoot::BlankUncommissioned,
            FirstWritePair::Pending(state) => FirstWriteBoot::FirstWritePending(state),
            FirstWritePair::Corrupt => FirstWriteBoot::Fault,
        },
        ControlPair::Corrupt => match pending {
            FirstWritePair::Pending(state) => FirstWriteBoot::FirstWritePending(state),
            FirstWritePair::Blank | FirstWritePair::Corrupt => FirstWriteBoot::Fault,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{
        ManagementCarrier, ManagementCarrierSet, PublicConfigurationV1, RecoveryClause,
        RecoveryPathFacts, ReticulumTransportPolicy,
    };
    use crate::region::Region;
    use ed25519_dalek::SigningKey;

    fn facts() -> BoardRecoveryFacts {
        BoardRecoveryFacts::new(
            Vec::from_slice(&[
                RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
            ])
            .unwrap(),
        )
        .unwrap()
    }

    fn initial_state() -> DurableState {
        let mut identity = [0x51; 64];
        identity[32..].copy_from_slice(
            SigningKey::from_bytes(&[0x51; 32])
                .verifying_key()
                .as_bytes(),
        );
        let public = PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8).unwrap(),
        )
        .unwrap();
        let policy = RecoveryPolicy::new(
            RecoveryClause::new(
                ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8).unwrap(),
                1,
            )
            .unwrap(),
            RecoveryClause::disabled(),
        )
        .unwrap();
        DurableState::from_owner_claim(
            NodeId([0x51; 16]),
            OwnerClaim::new(&identity, public, policy).unwrap(),
            &facts(),
        )
        .unwrap()
    }

    #[test]
    fn staged_state_rejects_a_tampered_replay_counter() {
        let mut state = initial_state();
        state.owner_grants[0].accepted_outer_counter = 1;
        assert_eq!(
            validate_first_write_state(&state),
            Err(FirstWriteError::OuterCounterNotZero)
        );
    }
}
