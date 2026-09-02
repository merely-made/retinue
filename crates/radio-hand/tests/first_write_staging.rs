use ed25519_dalek::SigningKey;
use heapless::Vec;
use radio_hand::control::{
    BoardRecoveryFacts, ConfigGeneration, DurableConfig, DurableState, FirstWriteBoot,
    FirstWriteError, FirstWriteLoadError, ManagementCarrier, ManagementCarrierSet, NodeId,
    OwnerClaim, OwnerGrant, PublicConfigurationV1, RecoveryClause, RecoveryPathFacts,
    RecoveryPolicy, ReticulumTransportPolicy, arbitrate_first_write, decode_durable,
    encode_first_write_state, load_first_write_state, next_first_write_record, next_record,
    validate_first_write_state,
};
use radio_hand::region::Region;

const PAGE: usize = 4096;

fn owner_identity(seed: u8) -> [u8; 64] {
    let mut identity = [seed; 64];
    identity[32..].copy_from_slice(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .as_bytes(),
    );
    identity
}

fn public_configuration() -> PublicConfigurationV1 {
    PublicConfigurationV1::new(
        Region::Us915,
        selvage::PhyProfile::meshtastic_long_fast(906_875_000),
        ReticulumTransportPolicy::new(false, false, 0).unwrap(),
        ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8).unwrap(),
    )
    .unwrap()
}

fn recovery_policy() -> RecoveryPolicy {
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
    DurableState::from_owner_claim(
        NodeId([0x10; 16]),
        OwnerClaim::new(
            &owner_identity(0x31),
            public_configuration(),
            recovery_policy(),
        )
        .unwrap(),
        &facts(),
    )
    .unwrap()
}

fn staged_page(state: &DurableState) -> [u8; PAGE] {
    let blank = [0xff; PAGE];
    let mut body = [0; radio_hand::control::MAX_DURABLE_BODY];
    let mut page = [0xff; PAGE];
    next_first_write_record(
        &blank,
        &blank,
        state,
        NodeId([0x10; 16]),
        &facts(),
        &mut body,
        &mut page,
    )
    .unwrap();
    page
}

fn control_page(state: &DurableState) -> [u8; PAGE] {
    let blank = [0xff; PAGE];
    let mut body = [0; radio_hand::control::MAX_DURABLE_BODY];
    let mut page = [0xff; PAGE];
    next_record(&blank, &blank, state, &mut body, &mut page).unwrap();
    page
}

fn arbitrate(
    control_a: &[u8],
    control_b: &[u8],
    pending_a: &[u8],
    pending_b: &[u8],
) -> FirstWriteBoot {
    arbitrate_first_write(
        control_a,
        control_b,
        pending_a,
        pending_b,
        NodeId([0x10; 16]),
        &facts(),
    )
}

fn later_control_state() -> DurableState {
    DurableState::new(
        NodeId([0x10; 16]),
        Vec::from_slice(&[OwnerGrant::from_public_identity(
            owner_identity(0x31),
            radio_hand::control::ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(1),
        DurableConfig {
            public: public_configuration(),
            sealed_credentials: Vec::new(),
        },
        recovery_policy(),
        &facts(),
    )
    .unwrap()
}

#[test]
fn canonical_initial_state_round_trips_through_staging_and_rhd1() {
    let state = initial_state();
    let mut body = [0; radio_hand::control::MAX_DURABLE_BODY];
    let len = encode_first_write_state(&state, NodeId([0x10; 16]), &facts(), &mut body).unwrap();
    assert_eq!(decode_durable(&body[..len]).unwrap(), state);

    let pending = staged_page(&state);
    let blank = [0xff; PAGE];
    assert_eq!(
        load_first_write_state(&pending, &blank, NodeId([0x10; 16]), &facts()),
        Ok(state)
    );
}

#[test]
fn only_two_erased_pending_slots_are_blank_and_torn_data_is_corrupt() {
    let blank = [0xff; PAGE];
    assert_eq!(
        arbitrate(&blank, &blank, &blank, &blank),
        FirstWriteBoot::BlankUncommissioned
    );
    assert_eq!(
        load_first_write_state(&blank, &blank, NodeId([0x10; 16]), &facts()),
        Err(FirstWriteLoadError::Blank)
    );

    let mut torn = blank;
    torn[0] = 0;
    assert_eq!(
        arbitrate(&blank, &blank, &torn, &blank),
        FirstWriteBoot::Fault
    );
    assert!(matches!(
        load_first_write_state(&torn, &blank, NodeId([0x10; 16]), &facts()),
        Err(FirstWriteLoadError::Corrupt(_))
    ));
}

#[test]
fn valid_control_wins_over_stale_pending_state() {
    let control = control_page(&later_control_state());
    let pending = staged_page(&initial_state());
    assert_eq!(
        arbitrate(&control, &[0xff; PAGE], &pending, &[0xff; PAGE]),
        FirstWriteBoot::ControlPresent(later_control_state())
    );
    let mut corrupt_pending = [0xff; PAGE];
    corrupt_pending[0] = 0;
    assert_eq!(
        arbitrate(&control, &[0xff; PAGE], &corrupt_pending, &[0xff; PAGE],),
        FirstWriteBoot::ControlPresent(later_control_state())
    );
}

#[test]
fn pending_initial_state_recovers_blank_or_corrupt_control_pair() {
    let state = initial_state();
    let pending = staged_page(&state);
    let blank = [0xff; PAGE];
    assert_eq!(
        arbitrate(&blank, &blank, &pending, &blank),
        FirstWriteBoot::FirstWritePending(state.clone())
    );

    let mut corrupt = blank;
    corrupt[0] = 0;
    assert_eq!(
        arbitrate(&corrupt, &blank, &pending, &blank),
        FirstWriteBoot::FirstWritePending(state)
    );
}

#[test]
fn corrupt_control_without_valid_pending_faults() {
    let blank = [0xff; PAGE];
    let mut corrupt = blank;
    corrupt[0] = 0;
    assert_eq!(
        arbitrate(&corrupt, &blank, &blank, &blank),
        FirstWriteBoot::Fault
    );
    assert_eq!(
        arbitrate(&blank, &blank, &corrupt, &blank),
        FirstWriteBoot::Fault
    );
}

#[test]
fn pending_state_must_match_this_node_and_current_recovery_facts() {
    let state = initial_state();
    let pending = staged_page(&state);
    let blank = [0xff; PAGE];
    assert_eq!(
        load_first_write_state(&pending, &blank, NodeId([0x11; 16]), &facts()),
        Err(FirstWriteLoadError::Corrupt(FirstWriteError::WrongNode))
    );
    let unavailable_usb = BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, false, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        load_first_write_state(&pending, &blank, NodeId([0x10; 16]), &unavailable_usb),
        Err(FirstWriteLoadError::Corrupt(
            FirstWriteError::UnsafeBoardRecoveryFacts
        ))
    );
    assert_eq!(
        arbitrate_first_write(
            &blank,
            &blank,
            &pending,
            &blank,
            NodeId([0x11; 16]),
            &facts(),
        ),
        FirstWriteBoot::Fault
    );
    assert_eq!(
        arbitrate_first_write(
            &blank,
            &blank,
            &pending,
            &blank,
            NodeId([0x10; 16]),
            &unavailable_usb,
        ),
        FirstWriteBoot::Fault
    );
}

#[test]
fn write_helpers_reject_foreign_or_unrecoverable_state() {
    let state = initial_state();
    let blank = [0xff; PAGE];
    let mut body = [0; radio_hand::control::MAX_DURABLE_BODY];
    let mut page = [0xff; PAGE];
    let unavailable_usb = BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, false, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        encode_first_write_state(&state, NodeId([0x11; 16]), &facts(), &mut body),
        Err(FirstWriteError::WrongNode)
    );
    assert_eq!(
        next_first_write_record(
            &blank,
            &blank,
            &state,
            NodeId([0x11; 16]),
            &facts(),
            &mut body,
            &mut page,
        ),
        Err(FirstWriteError::WrongNode)
    );
    assert_eq!(
        encode_first_write_state(&state, NodeId([0x10; 16]), &unavailable_usb, &mut body),
        Err(FirstWriteError::UnsafeBoardRecoveryFacts)
    );
    assert_eq!(
        next_first_write_record(
            &blank,
            &blank,
            &state,
            NodeId([0x10; 16]),
            &unavailable_usb,
            &mut body,
            &mut page,
        ),
        Err(FirstWriteError::UnsafeBoardRecoveryFacts)
    );
}

#[test]
fn non_initial_generation_cannot_be_staged() {
    let state = later_control_state();
    assert_eq!(
        validate_first_write_state(&state),
        Err(FirstWriteError::GenerationNotZero)
    );
}
