use super::tests::{change, config, controller, request, semantic_tag_key, state};
use super::{
    COMMIT_TOKEN_LEN, ControllerRole, MUTATION_SEQUENCE_WINDOW, Operation, Refusal, ResponseBody,
    Transition,
};
use crate::control::{ManagementCarrierSet, PublicConfigurationV1};
use heapless::Vec;

#[test]
fn mutation_sequences_survive_receipt_eviction_without_reopening_a_mutation() {
    let mut state = state();
    let key = semantic_tag_key(0x78);
    let first = request(0x80);
    state
        .arm(
            state.node(),
            controller(0x30),
            &first,
            &key,
            change(0x80),
            config(b"candidate", b"sealed"),
            1,
            10,
            [0x80; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        state.arm(
            state.node(),
            controller(0x30),
            &first,
            &key,
            change(0x80),
            config(b"replayed", b""),
            2,
            10,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Ok(Transition::Replayed(_))
    ));
    let mut changed = first.clone();
    changed.arguments.push(0).unwrap();
    assert_eq!(
        state.arm(
            state.node(),
            controller(0x30),
            &changed,
            &key,
            change(0x80),
            config(b"candidate", b"sealed"),
            2,
            10,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::TransactionConflict)
    );
    let _ = state.expire(10);
    let second = request(0x81);
    state
        .arm(
            state.node(),
            controller(0x30),
            &second,
            &key,
            change(0x81),
            config(b"candidate", b"sealed"),
            11,
            20,
            [0x81; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    let _ = state.expire(20);
    assert_eq!(
        state.arm(
            state.node(),
            controller(0x30),
            &first,
            &key,
            change(0x80),
            config(b"candidate", b"sealed"),
            21,
            30,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::TransactionExpired)
    );
    let mut too_far = request(0x82);
    too_far.transaction_sequence = 0x81_u64 + MUTATION_SEQUENCE_WINDOW + 1;
    assert_eq!(
        state.arm(
            state.node(),
            controller(0x30),
            &too_far,
            &key,
            change(0x82),
            config(b"candidate", b"sealed"),
            21,
            30,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Err(Refusal::TransactionTooFar)
    );
}

#[test]
fn safe_policy_refusals_advance_and_retain_their_sequence() {
    let mut state = state();
    let key = semantic_tag_key(0x79);
    let mut request = request(0x83);
    request.operation = Operation::Commit;
    let changed = state
        .arm(
            state.node(),
            controller(0x30),
            &request,
            &key,
            change(0x83),
            config(b"candidate", b"sealed"),
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(changed.is_changed());
    assert!(matches!(
        changed.response().body,
        ResponseBody::Refused {
            reason: Refusal::UnsupportedOperation,
            ..
        }
    ));
    assert!(matches!(
        state.arm(
            state.node(),
            controller(0x30),
            &request,
            &key,
            change(0x83),
            config(b"candidate", b"sealed"),
            2,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        ),
        Ok(Transition::Replayed(_))
    ));
}

#[test]
fn observer_grant_authenticates_but_cannot_mutate_configuration() {
    let mut state = state();
    state.owner_grants[0].role = ControllerRole::Observer;
    let request = request(0x86);
    let transition = state
        .arm(
            state.node(),
            controller(0x30),
            &request,
            &semantic_tag_key(0x7b),
            change(0x86),
            config(b"candidate", b"sealed"),
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(transition.is_changed());
    assert!(matches!(
        transition.response().body,
        ResponseBody::Refused {
            reason: Refusal::Unauthorized,
            ..
        }
    ));
    assert!(state.provisional().is_none());
}

#[test]
fn operator_is_limited_to_reticulum_phy_and_transport_not_carriers_or_secrets() {
    let key = semantic_tag_key(0x7c);
    let mut safe = state();
    safe.owner_grants[0].role = ControllerRole::Operator;
    let accepted = safe
        .arm(
            safe.node(),
            controller(0x30),
            &request(0x87),
            &key,
            change(0x87),
            config(b"new-phy", b"sealed-old"),
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        accepted.response().body,
        ResponseBody::Provisional { .. }
    ));

    let mut carrier_change = state();
    carrier_change.owner_grants[0].role = ControllerRole::Operator;
    let mut candidate = config(b"new-phy", b"sealed-old");
    candidate.public = PublicConfigurationV1::new(
        candidate.public.region(),
        candidate.public.requested_reticulum_phy(),
        candidate.public.reticulum_transport(),
        ManagementCarrierSet::from_mask(1).unwrap(),
    )
    .unwrap();
    let denied = carrier_change
        .arm(
            carrier_change.node(),
            controller(0x30),
            &request(0x88),
            &key,
            change(0x88),
            candidate,
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        denied.response().body,
        ResponseBody::Refused {
            reason: Refusal::Unauthorized,
            ..
        }
    ));

    let mut secret_change = state();
    secret_change.owner_grants[0].role = ControllerRole::Operator;
    let denied = secret_change
        .arm(
            secret_change.node(),
            controller(0x30),
            &request(0x89),
            &key,
            change(0x89),
            config(b"new-phy", b"different-sealed"),
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        denied.response().body,
        ResponseBody::Refused {
            reason: Refusal::Unauthorized,
            ..
        }
    ));
}

#[test]
fn updater_cannot_mutate_configuration() {
    let mut state = state();
    state.owner_grants[0].role = ControllerRole::Updater;
    let transition = state
        .arm(
            state.node(),
            controller(0x30),
            &request(0x8a),
            &semantic_tag_key(0x7d),
            change(0x8a),
            config(b"candidate", b"sealed-old"),
            1,
            20,
            [0; COMMIT_TOKEN_LEN],
            Vec::new(),
        )
        .unwrap();
    assert!(matches!(
        transition.response().body,
        ResponseBody::Refused {
            reason: Refusal::Unauthorized,
            ..
        }
    ));
}
