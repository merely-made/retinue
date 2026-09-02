use heapless::Vec;
use radio_hand::control::{
    BoardRecoveryFacts, ConfigGeneration, ControllerRole, DurableConfig, DurableLoadError,
    DurableState, MAX_DURABLE_BODY, ManagementCarrier, ManagementCarrierSet, NodeId, OwnerGrant,
    PublicConfigurationV1, RecoveryClause, RecoveryPathFacts, RecoveryPolicy,
    ReticulumTransportPolicy, decode_durable, encode_durable, load, next_record,
};
use radio_hand::region::Region;
use retinue::identity::PrivateIdentity;

const PAGE: usize = 4096;

fn configuration() -> DurableConfig {
    DurableConfig {
        public: PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap(),
        sealed_credentials: Vec::try_from(b"sealed-not-plaintext".as_slice()).unwrap(),
    }
}

fn policy() -> RecoveryPolicy {
    RecoveryPolicy::new(
        RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        RecoveryClause::disabled(),
    )
    .unwrap()
}

fn facts() -> BoardRecoveryFacts {
    BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap()
}

fn state() -> DurableState {
    DurableState::new(
        NodeId([0x10; 16]),
        Vec::from_slice(&[OwnerGrant::from_public_identity(
            PrivateIdentity::from_secret_bytes(&[0x30; 64])
                .public()
                .to_public_bytes(),
            ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(7),
        configuration(),
        policy(),
        &facts(),
    )
    .unwrap()
}

#[test]
fn public_journal_format_round_trips_and_uses_existing_ab_selection() {
    let state = state();
    let mut body = [0; MAX_DURABLE_BODY];
    let body_len = encode_durable(&state, &mut body).unwrap();
    assert_eq!(decode_durable(&body[..body_len]).unwrap(), state);

    let a = [0xFF; PAGE];
    let b = [0xFF; PAGE];
    let mut page = [0xFF; PAGE];
    let write = next_record(&a, &b, &state, &mut body, &mut page).unwrap();
    assert_eq!(write.sequence, 0);
    assert_eq!(load(&page, &b).unwrap(), state);
}

#[test]
fn rhd_v2_is_refused_instead_of_reinterpreting_weak_recovery_claims() {
    let state = state();
    let mut body = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state, &mut body).unwrap();
    assert_eq!(body[4], 3);
    body[4] = 2;
    assert_eq!(
        decode_durable(&body[..len]),
        Err(radio_hand::control::DurableError::UnsupportedVersion(2))
    );
}

#[test]
fn public_configuration_and_sealed_credentials_remain_separate() {
    let state = state();
    let public = state.known_good().configuration.public;
    let sealed = &state.known_good().configuration.sealed_credentials;
    assert_eq!(public.encode().len(), 21);
    assert!(
        public
            .encode()
            .windows(sealed.len())
            .all(|window| window != sealed.as_slice())
    );

    let mut body = [0; MAX_DURABLE_BODY];
    let body_len = encode_durable(&state, &mut body).unwrap();
    let recovered = decode_durable(&body[..body_len]).unwrap();
    assert_eq!(recovered.known_good().configuration.public, public);
    assert_eq!(
        recovered.known_good().configuration.sealed_credentials,
        *sealed
    );
}

#[test]
fn only_a_proven_blank_pair_is_uninitialized() {
    assert_eq!(
        load(&[0xFF; PAGE], &[0xFF; PAGE]),
        Err(DurableLoadError::Blank)
    );
    let mut corrupt = [0xFF; PAGE];
    corrupt[0] = 0;
    assert_eq!(
        load(&corrupt, &[0xFF; PAGE]),
        Err(DurableLoadError::Corrupt)
    );
}
