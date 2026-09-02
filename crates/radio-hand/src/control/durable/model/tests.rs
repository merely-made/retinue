use super::*;
use crate::control::{
    ManagementCarrier, ManagementCarrierSet, PublicConfigurationV1, RETINUE_PUBLIC_IDENTITY_LEN,
    ReticulumTransportPolicy,
};
use crate::region::Region;
use crate::store::Slot;
use core::fmt::Write;
use heapless::String;
use retinue::identity::PrivateIdentity;
const PAGE: usize = 4096;

pub(super) fn controller(value: u8) -> VerifiedController {
    VerifiedController::from_verified_key(
        OwnerGrant::from_public_identity(public_identity(value), ControllerRole::Owner)
            .controller(),
    )
}

pub(super) fn public_identity(value: u8) -> [u8; 64] {
    PrivateIdentity::from_secret_bytes(&[value; 64])
        .public()
        .to_public_bytes()
}

pub(super) fn config(public: &[u8], sealed: &[u8]) -> DurableConfig {
    DurableConfig {
        public: PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(
                902_000_000 + u32::from(public.first().copied().unwrap_or(0)) * 100_000,
            ),
            ReticulumTransportPolicy::new(true, true, 8).unwrap(),
            ManagementCarrierSet::from_mask(0b1001).unwrap(),
        )
        .unwrap(),
        sealed_credentials: Vec::try_from(sealed).unwrap(),
    }
}

pub(super) fn policy() -> RecoveryPolicy {
    RecoveryPolicy::new(
        RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        RecoveryClause::disabled(),
    )
    .unwrap()
}

fn config_without_recovery(public: &[u8]) -> DurableConfig {
    DurableConfig {
        public: PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(
                902_000_000 + u32::from(public.first().copied().unwrap_or(0)) * 100_000,
            ),
            ReticulumTransportPolicy::new(true, true, 8).unwrap(),
            ManagementCarrierSet::from_mask(0b1000).unwrap(),
        )
        .unwrap(),
        sealed_credentials: Vec::new(),
    }
}

pub(super) fn facts() -> BoardRecoveryFacts {
    BoardRecoveryFacts::new(
        Vec::from_slice(&[
            RecoveryPathFacts::new(ManagementCarrier::Usb, true, false, false).unwrap(),
            RecoveryPathFacts::new(ManagementCarrier::Reticulum, false, true, true).unwrap(),
            RecoveryPathFacts::new(ManagementCarrier::Ip, false, true, true).unwrap(),
            RecoveryPathFacts::new(ManagementCarrier::Ble, true, false, false).unwrap(),
        ])
        .unwrap(),
    )
    .unwrap()
}

pub(super) fn state() -> DurableState {
    DurableState::new(
        NodeId([0x10; 16]),
        Vec::from_slice(&[OwnerGrant::from_public_identity(
            public_identity(0x30),
            ControllerRole::Owner,
        )])
        .unwrap(),
        ConfigGeneration(7),
        config(b"old", b"sealed-old"),
        policy(),
        &facts(),
    )
    .unwrap()
}

pub(super) fn request(transaction: u8) -> Request {
    Request {
        transaction: TransactionId([transaction; 16]),
        transaction_sequence: u64::from(transaction),
        expected_generation: ConfigGeneration(7),
        operation: Operation::ProvisionalApply,
        arguments: Vec::try_from(b"wifi-credential-plaintext".as_slice()).unwrap(),
    }
}

pub(super) fn semantic_tag_key(value: u8) -> SemanticTagKey {
    SemanticTagKey::from_bytes([value; 32])
}

pub(super) fn change(value: u8) -> ChangeId {
    ChangeId([value; CHANGE_ID_LEN])
}

fn commit_request(transaction: u8) -> Request {
    Request {
        transaction: TransactionId([transaction; 16]),
        transaction_sequence: u64::from(transaction),
        expected_generation: ConfigGeneration(7),
        operation: Operation::Commit,
        arguments: Vec::new(),
    }
}

fn persist(a: &mut [u8; PAGE], b: &mut [u8; PAGE], state: &DurableState) {
    let mut body = [0; MAX_DURABLE_BODY];
    let mut page = [0xFF; PAGE];
    let write = next_record(a, b, state, &mut body, &mut page).unwrap();
    match write.slot {
        Slot::A => *a = page,
        Slot::B => *b = page,
    }
}

fn boot(a: &[u8; PAGE], b: &[u8; PAGE]) -> DurableState {
    load(a, b).expect("an intact record survives the cut")
}

#[test]
fn durable_encoding_is_bounded_round_trips_and_never_keeps_request_bytes() {
    let mut state = state();
    let req = request(0x40);
    let semantic_tag_key = semantic_tag_key(0x70);
    let response = state
        .arm(
            state.node(),
            controller(0x30),
            &req,
            &semantic_tag_key,
            change(0x60),
            config(b"candidate", b"sealed-candidate"),
            10,
            20,
            [0xA5; COMMIT_TOKEN_LEN],
            Vec::try_from(b"safe-result".as_slice()).unwrap(),
        )
        .unwrap();
    assert!(response.is_changed());
    assert!(matches!(
        response.response().body,
        ResponseBody::Provisional { .. }
    ));

    let mut out = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state, &mut out).unwrap();
    assert!(len <= MAX_DURABLE_BODY);
    assert_eq!(decode_durable(&out[..len]).unwrap(), state);
    assert!(
        out[..len]
            .windows(b"wifi-credential-plaintext".len())
            .all(|window| window != b"wifi-credential-plaintext")
    );

    let mut debug = String::<4096>::new();
    write!(&mut debug, "{state:?}").unwrap();
    let mut key_debug = String::<64>::new();
    write!(&mut key_debug, "{semantic_tag_key:?}").unwrap();
    assert_eq!(key_debug.as_str(), "SemanticTagKey([redacted])");
    assert!(!debug.contains("sealed-candidate"));
    assert!(!debug.contains("wifi-credential-plaintext"));
    assert!(!debug.contains("165"));
    assert!(debug.contains("SemanticTag([redacted])"));
}

#[test]
fn cut_points_always_boot_to_last_known_good_until_commit_is_durable() {
    let mut a = [0xFF; PAGE];
    let mut b = [0xFF; PAGE];
    let mut state = state();
    let semantic_tag_key = semantic_tag_key(0x71);
    persist(&mut a, &mut b, &state);

    let req = request(0x41);
    let armed = state
        .arm(
            state.node(),
            controller(0x30),
            &req,
            &semantic_tag_key,
            change(0x61),
            config(b"candidate", b"sealed-candidate"),
            1,
            100,
            [0xA5; COMMIT_TOKEN_LEN],
            Vec::try_from(b"applied".as_slice()).unwrap(),
        )
        .unwrap();
    assert!(armed.is_changed());
    let candidate_generation = armed.response().effective_generation.unwrap();

    // Cut before the rollback record is written: the original record wins.
    assert_eq!(boot(&a, &b).known_good().generation, ConfigGeneration(7));

    // Cut after erase but before the provisional record is programmed.
    let mut arm_page = [0xFF; PAGE];
    let mut scratch = [0; MAX_DURABLE_BODY];
    let arm_write = next_record(&a, &b, &state, &mut scratch, &mut arm_page).unwrap();
    let mut erased_a = a;
    let mut erased_b = b;
    match arm_write.slot {
        Slot::A => erased_a = [0xFF; PAGE],
        Slot::B => erased_b = [0xFF; PAGE],
    }
    assert_eq!(
        boot(&erased_a, &erased_b).known_good().generation,
        ConfigGeneration(7)
    );

    // Cut after journal write or while applying the candidate: boot rolls back.
    match arm_write.slot {
        Slot::A => a = arm_page,
        Slot::B => b = arm_page,
    }
    // Every interrupted program prefix either leaves the old record selected
    // or leaves the full provisional record selected, which boot rolls back.
    for prefix in 0..arm_write.len {
        let mut cut_a = erased_a;
        let mut cut_b = erased_b;
        match arm_write.slot {
            Slot::A => cut_a[..prefix].copy_from_slice(&arm_page[..prefix]),
            Slot::B => cut_b[..prefix].copy_from_slice(&arm_page[..prefix]),
        }
        let mut cut = boot(&cut_a, &cut_b);
        let _ = cut.recover_after_reboot();
        assert_eq!(
            cut.known_good().generation,
            ConfigGeneration(7),
            "arm prefix {prefix}"
        );
    }
    let mut after_arm = boot(&a, &b);
    assert!(matches!(
        after_arm.recover_after_reboot(),
        Recovery::Rollback { .. }
    ));
    assert_eq!(after_arm.known_good().generation, ConfigGeneration(7));

    let commit = commit_request(0x51);
    let committed = {
        let mut value = state.clone();
        value
            .commit(
                value.node(),
                controller(0x30),
                &commit,
                &semantic_tag_key,
                change(0x61),
                candidate_generation,
                [0xA5; COMMIT_TOKEN_LEN],
                10,
            )
            .unwrap();
        value
    };
    let mut commit_page = [0xFF; PAGE];
    let commit_write = next_record(&a, &b, &committed, &mut scratch, &mut commit_page).unwrap();

    // Cut after erasing the commit target: the provisional record still wins and rolls back.
    let mut commit_erased_a = a;
    let mut commit_erased_b = b;
    match commit_write.slot {
        Slot::A => commit_erased_a = [0xFF; PAGE],
        Slot::B => commit_erased_b = [0xFF; PAGE],
    }
    assert!(matches!(
        boot(&commit_erased_a, &commit_erased_b).recover_after_reboot(),
        Recovery::Rollback { .. }
    ));
    for prefix in 0..commit_write.len {
        let mut cut_a = commit_erased_a;
        let mut cut_b = commit_erased_b;
        match commit_write.slot {
            Slot::A => cut_a[..prefix].copy_from_slice(&commit_page[..prefix]),
            Slot::B => cut_b[..prefix].copy_from_slice(&commit_page[..prefix]),
        }
        let mut cut = boot(&cut_a, &cut_b);
        let _ = cut.recover_after_reboot();
        let generation = cut.known_good().generation;
        assert!(
            generation == ConfigGeneration(7) || generation == candidate_generation,
            "commit prefix {prefix}"
        );
    }

    // The programmed commit wins; erasing the stale provisional page is only cleanup.
    match commit_write.slot {
        Slot::A => a = commit_page,
        Slot::B => b = commit_page,
    }
    assert_eq!(boot(&a, &b).known_good().generation, candidate_generation);
    match commit_write.slot.other() {
        Slot::A => a = [0xFF; PAGE],
        Slot::B => b = [0xFF; PAGE],
    }
    assert_eq!(boot(&a, &b).known_good().generation, candidate_generation);
}

#[test]
fn replay_refusals_and_generation_watermark_are_durable() {
    let mut state = state();
    let req = request(0x42);
    let semantic_tag_key = semantic_tag_key(0x72);
    let candidate = config(b"candidate", b"sealed-candidate");
    let response = state
        .arm(
            state.node(),
            controller(0x30),
            &req,
            &semantic_tag_key,
            change(0x62),
            candidate.clone(),
            1,
            20,
            [0xB5; COMMIT_TOKEN_LEN],
            Vec::try_from(b"done".as_slice()).unwrap(),
        )
        .unwrap();
    assert!(response.is_changed());
    let first_generation = response.response().effective_generation.unwrap();
    assert_eq!(
        state.arm(
            state.node(),
            controller(0x30),
            &req,
            &semantic_tag_key,
            change(0x62),
            config(b"replayed", b""),
            2,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Ok(Transition::Replayed(response.into_response()))
    );
    let mut changed_arguments = req.clone();
    changed_arguments.arguments[0] ^= 1;
    assert_eq!(
        state.arm(
            state.node(),
            controller(0x30),
            &changed_arguments,
            &semantic_tag_key,
            change(0x62),
            candidate.clone(),
            2,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::TransactionConflict)
    );
    let mut longer_arguments = req.clone();
    longer_arguments.arguments.push(0).unwrap();
    assert_eq!(
        state.arm(
            state.node(),
            controller(0x30),
            &longer_arguments,
            &semantic_tag_key,
            change(0x62),
            candidate.clone(),
            2,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::TransactionConflict)
    );
    assert_eq!(
        state.commit(
            state.node(),
            controller(0x31),
            &commit_request(0x52),
            &semantic_tag_key,
            change(0x62),
            first_generation,
            [0xB5; COMMIT_TOKEN_LEN],
            3,
        ),
        Err(Refusal::Unauthorized)
    );
    assert!(matches!(state.expire(20), Recovery::Rollback { .. }));
    assert_eq!(state.generation_watermark(), first_generation);

    let next_request = request(0x43);
    let next = state
        .arm(
            state.node(),
            controller(0x30),
            &next_request,
            &semantic_tag_key,
            change(0x63),
            candidate,
            21,
            40,
            [0xC5; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert_eq!(
        next.response().effective_generation,
        Some(ConfigGeneration(first_generation.0 + 1))
    );
    assert_eq!(
        state.arm(
            NodeId([0x99; 16]),
            controller(0x30),
            &request(0x44),
            &semantic_tag_key,
            change(0x64),
            config(b"other", b"sealed"),
            21,
            40,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::WrongNode)
    );

    assert_eq!(
        DurableState::new(
            NodeId([0x10; 16]),
            Vec::new(),
            ConfigGeneration(0),
            config_without_recovery(b"unsafe"),
            policy(),
            &facts(),
        ),
        Err(Refusal::UnsafeRecoveryPath)
    );
}

#[test]
fn semantic_tag_key_must_survive_reboot_to_recognize_replays() {
    let mut state = state();
    let request = request(0x45);
    let key = semantic_tag_key(0x76);
    let response = state
        .arm(
            state.node(),
            controller(0x30),
            &request,
            &key,
            change(0x65),
            config(b"candidate", b"sealed-candidate"),
            1,
            20,
            [0xD5; COMMIT_TOKEN_LEN],
            Vec::try_from(b"done".as_slice()).unwrap(),
        )
        .unwrap();
    let mut a = [0xFF; PAGE];
    let mut b = [0xFF; PAGE];
    persist(&mut a, &mut b, &state);
    let mut reloaded = boot(&a, &b);

    assert_eq!(
        reloaded.arm(
            reloaded.node(),
            controller(0x30),
            &request,
            &key,
            change(0x65),
            config(b"replayed", b""),
            2,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Ok(Transition::Replayed(response.into_response()))
    );
    assert_eq!(
        reloaded.arm(
            reloaded.node(),
            controller(0x30),
            &request,
            &semantic_tag_key(0x77),
            change(0x65),
            config(b"replayed", b""),
            2,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::TransactionConflict)
    );
}

#[test]
fn decoded_operator_provisional_cannot_change_management_carriers() {
    let mut state = state();
    let mut candidate = config(b"candidate", b"sealed-old");
    candidate.public = PublicConfigurationV1::new(
        candidate.public.region(),
        candidate.public.requested_reticulum_phy(),
        candidate.public.reticulum_transport(),
        ManagementCarrierSet::from_mask(1).unwrap(),
    )
    .unwrap();
    state
        .arm(
            state.node(),
            controller(0x30),
            &request(0x46),
            &semantic_tag_key(0x76),
            change(0x66),
            candidate,
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    state.owner_grants[0].role = ControllerRole::Operator;
    let mut bytes = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state, &mut bytes).unwrap();
    assert_eq!(decode_durable(&bytes[..len]), Err(DurableError::Malformed));
}

#[test]
fn corrupt_slot_pair_cannot_be_reused_but_one_valid_slot_can_recover() {
    let state = state();
    let mut corrupt_a = [0xFF; PAGE];
    let mut corrupt_b = [0xFF; PAGE];
    corrupt_a[0] = 0;
    corrupt_b[7] = 0;
    let mut body = [0; MAX_DURABLE_BODY];
    let mut page = [0xFF; PAGE];
    assert_eq!(
        next_record(&corrupt_a, &corrupt_b, &state, &mut body, &mut page),
        Err(DurableError::NoValidSlot)
    );

    let mut valid_a = [0xFF; PAGE];
    let mut blank_b = [0xFF; PAGE];
    persist(&mut valid_a, &mut blank_b, &state);
    assert_eq!(
        next_record(&valid_a, &corrupt_b, &state, &mut body, &mut page)
            .unwrap()
            .slot,
        Slot::B
    );
}

#[test]
fn decoded_state_rejects_tampered_invariants() {
    let mut tampered = state();
    tampered.generation_watermark = ConfigGeneration(6);
    let mut body = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&tampered, &mut body).unwrap();
    assert_eq!(decode_durable(&body[..len]), Err(DurableError::Malformed));

    let mut duplicate = state();
    duplicate
        .owner_grants
        .push(duplicate.owner_grants[0])
        .unwrap();
    let len = encode_durable(&duplicate, &mut body).unwrap();
    assert_eq!(decode_durable(&body[..len]), Err(DurableError::Malformed));

    let mut no_owner = state();
    no_owner.owner_grants[0].role = ControllerRole::Operator;
    let len = encode_durable(&no_owner, &mut body).unwrap();
    assert_eq!(decode_durable(&body[..len]), Err(DurableError::Malformed));

    let mut mismatched_hash = state();
    mismatched_hash.owner_grants[0].controller.0[0] ^= 1;
    let len = encode_durable(&mismatched_hash, &mut body).unwrap();
    assert_eq!(decode_durable(&body[..len]), Err(DurableError::Malformed));

    let mut invalid_identity = state();
    let mut invalid_public = [0; RETINUE_PUBLIC_IDENTITY_LEN];
    // This compressed Edwards-Y encoding does not decompress as an Ed25519 key.
    invalid_public[32..].fill(2);
    invalid_identity.owner_grants[0] =
        OwnerGrant::from_public_identity(invalid_public, ControllerRole::Owner);
    let len = encode_durable(&invalid_identity, &mut body).unwrap();
    assert_eq!(decode_durable(&body[..len]), Err(DurableError::Malformed));
}

mod invariants;
