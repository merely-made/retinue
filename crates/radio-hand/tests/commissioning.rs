use ed25519_dalek::SigningKey;
use heapless::Vec;
use radio_hand::control::{
    BoardRecoveryFacts, ConfigGeneration, ControllerRole, ManagementCarrier, ManagementCarrierSet,
    NodeId, OwnerClaim, OwnerClaimError, PublicConfigurationError, PublicConfigurationV1,
    RecoveryClause, RecoveryPathFacts, RecoveryPolicy, RecoveryPolicyError,
    ReticulumTransportPolicy,
};
use radio_hand::region::Region;

fn owner_identity(seed: u8) -> [u8; 64] {
    let mut identity = [seed; 64];
    identity[32..].copy_from_slice(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .as_bytes(),
    );
    identity
}

fn configuration(mask: u8) -> PublicConfigurationV1 {
    PublicConfigurationV1::new(
        Region::Us915,
        selvage::PhyProfile::meshtastic_long_fast(906_875_000),
        ReticulumTransportPolicy::new(false, false, 0).unwrap(),
        ManagementCarrierSet::from_mask(mask).unwrap(),
    )
    .unwrap()
}

fn usb_recovery() -> RecoveryPolicy {
    RecoveryPolicy::new(
        RecoveryClause::new(
            ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8).unwrap(),
            1,
        )
        .unwrap(),
        RecoveryClause::disabled(),
    )
    .unwrap()
}

fn facts(usb_supports_physical_presence: bool) -> BoardRecoveryFacts {
    BoardRecoveryFacts::new(
        Vec::from_slice(&[RecoveryPathFacts::new(
            ManagementCarrier::Usb,
            usb_supports_physical_presence,
            false,
            false,
        )
        .unwrap()])
        .unwrap(),
    )
    .unwrap()
}

fn valid_claim() -> OwnerClaim {
    OwnerClaim::new(
        &owner_identity(0x31),
        configuration(1 << ManagementCarrier::Usb as u8),
        usb_recovery(),
    )
    .unwrap()
}

#[test]
fn valid_claim_creates_exactly_one_owner_at_the_canonical_initial_state() {
    let claim = valid_claim();
    let state = radio_hand::control::DurableState::from_owner_claim(
        NodeId([0x10; 16]),
        claim.clone(),
        &facts(true),
    )
    .unwrap();

    assert_eq!(state.node(), NodeId([0x10; 16]));
    assert_eq!(state.owner_grants().len(), 1);
    assert_eq!(state.owner_grants()[0].role(), ControllerRole::Owner);
    assert_eq!(
        state.owner_grants()[0].retinue_public_identity(),
        claim.owner_public_identity()
    );
    assert_eq!(state.owner_grants()[0].accepted_outer_counter(), 0);
    assert_eq!(state.owner_grants()[0].accepted_mutation_sequence(), 0);
    assert_eq!(state.known_good().generation, ConfigGeneration(0));
    assert_eq!(state.generation_watermark(), ConfigGeneration(0));
    assert_eq!(
        state.known_good().configuration.public,
        claim.public_configuration()
    );
    assert!(
        state
            .known_good()
            .configuration
            .sealed_credentials
            .is_empty()
    );
    assert_eq!(state.recovery_policy(), claim.recovery_policy());
    assert!(state.provisional().is_none());
    assert!(state.receipt().is_none());
}

#[test]
fn malformed_or_truncated_public_identity_is_rejected() {
    let mut malformed = [0; 64];
    // This compressed Edwards-Y encoding does not decompress as an Ed25519 key.
    malformed[32..].fill(2);
    assert_eq!(
        OwnerClaim::new(&malformed, configuration(1), usb_recovery(),),
        Err(OwnerClaimError::InvalidPublicIdentity)
    );
    assert_eq!(
        OwnerClaim::new(
            &owner_identity(0x32)[..63],
            configuration(1),
            usb_recovery(),
        ),
        Err(OwnerClaimError::PublicIdentityLength)
    );
}

#[test]
fn owner_role_is_implicit_and_unsafe_recovery_configuration_is_rejected() {
    assert_eq!(
        OwnerClaim::new(
            &owner_identity(0x33),
            configuration(1 << ManagementCarrier::Ip as u8),
            usb_recovery(),
        ),
        Err(OwnerClaimError::UnsatisfiedRecoveryPolicy)
    );
}

#[test]
fn board_recovery_facts_are_required_before_state_construction() {
    let claim = valid_claim();
    assert_eq!(
        radio_hand::control::DurableState::from_owner_claim(
            NodeId([0x20; 16]),
            claim,
            &facts(false),
        ),
        Err(OwnerClaimError::UnsafeBoardRecoveryFacts)
    );
}

#[test]
fn invalid_public_configuration_or_recovery_policy_is_refused_by_its_existing_validator() {
    let mut encoded = configuration(1).encode();
    encoded[2] = 0;
    assert_eq!(
        PublicConfigurationV1::decode(&encoded),
        Err(PublicConfigurationError::EmptyManagementCarriers)
    );
    assert_eq!(
        RecoveryPolicy::new(RecoveryClause::disabled(), RecoveryClause::disabled()),
        Err(RecoveryPolicyError::Empty)
    );
}

#[test]
fn construction_is_deterministic_and_has_no_transaction_artifacts() {
    let first = radio_hand::control::DurableState::from_owner_claim(
        NodeId([0x44; 16]),
        valid_claim(),
        &facts(true),
    )
    .unwrap();
    let second = radio_hand::control::DurableState::from_owner_claim(
        NodeId([0x44; 16]),
        valid_claim(),
        &facts(true),
    )
    .unwrap();
    assert_eq!(first, second);
    assert!(first.provisional().is_none());
    assert!(first.receipt().is_none());
}
