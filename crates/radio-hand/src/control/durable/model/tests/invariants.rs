use super::*;
use core::fmt::Write;
use heapless::{String, Vec};

#[test]
fn v3_worst_case_body_fits_and_grant_debug_redacts_identity() {
    let mut state = state();
    state.recovery_policy = RecoveryPolicy::new(
        RecoveryClause::new(ManagementCarrierSet::from_mask(0b0011).unwrap(), 1).unwrap(),
        RecoveryClause::new(ManagementCarrierSet::from_mask(0b1100).unwrap(), 1).unwrap(),
    )
    .unwrap();
    state.owner_grants = Vec::from_slice(&[
        OwnerGrant::from_public_identity(public_identity(0x7e), ControllerRole::Owner),
        OwnerGrant::from_public_identity(public_identity(0x31), ControllerRole::Operator),
        OwnerGrant::from_public_identity(public_identity(0x32), ControllerRole::Operator),
        OwnerGrant::from_public_identity(public_identity(0x33), ControllerRole::Observer),
    ])
    .unwrap();
    state.known_good.configuration =
        config(&[0x41; MAX_PUBLIC_CONFIG], &[0x42; MAX_SEALED_CREDENTIALS]);
    let armed_request = request(0x84);
    state
        .arm(
            state.node(),
            controller(0x7e),
            &armed_request,
            &semantic_tag_key(0x7a),
            change(0x84),
            config(&[0x43; MAX_PUBLIC_CONFIG], &[0x44; MAX_SEALED_CREDENTIALS]),
            1,
            20,
            [0xA5; COMMIT_TOKEN_LEN],
            Vec::from_slice(&[0x45; MAX_RESULT]).unwrap(),
        )
        .unwrap();
    let committed = commit_request(0x85);
    state
        .commit(
            state.node(),
            controller(0x7e),
            &committed,
            &semantic_tag_key(0x7a),
            change(0x84),
            ConfigGeneration(8),
            [0xA5; COMMIT_TOKEN_LEN],
            2,
        )
        .unwrap();
    let mut armed_again = request(0x86);
    armed_again.expected_generation = ConfigGeneration(8);
    state
        .arm(
            state.node(),
            controller(0x7e),
            &armed_again,
            &semantic_tag_key(0x7a),
            change(0x86),
            config(&[0x45; MAX_PUBLIC_CONFIG], &[0x46; MAX_SEALED_CREDENTIALS]),
            3,
            20,
            [0xA6; COMMIT_TOKEN_LEN],
            Vec::from_slice(&[0x47; MAX_RESULT]).unwrap(),
        )
        .unwrap();
    let mut body = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state, &mut body).unwrap();
    assert!(len <= MAX_DURABLE_BODY);
    assert_eq!(decode_durable(&body[..len]).unwrap(), state);
    let mut debug = String::<512>::new();
    write!(&mut debug, "{:?}", state.owner_grants[0]).unwrap();
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("126, 126, 126"));
}

#[test]
fn accepted_outer_counter_is_monotonic_and_durable() {
    let mut state = state();
    assert_eq!(
        state.advance_verified_outer_counter(controller(0x30), 7),
        Ok(())
    );
    assert_eq!(
        state.advance_verified_outer_counter(controller(0x30), 7),
        Err(VerifiedCounterError::NotMonotonic)
    );
    let mut body = [0; MAX_DURABLE_BODY];
    let len = encode_durable(&state, &mut body).unwrap();
    assert_eq!(
        decode_durable(&body[..len]).unwrap().owner_grants()[0].accepted_outer_counter(),
        7
    );
}
