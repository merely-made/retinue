use ed25519_dalek::SigningKey;
use radio_hand::control::{
    PublicIdentityError, RETINUE_PUBLIC_IDENTITY_LEN, validate_retinue_public_identity,
};
use retinue::identity::Identity;

const RAW_CORPUS_CASES: usize = 256;
const CANONICAL_CORPUS_CASES: usize = 64;

fn generated_identity(case: u64) -> [u8; RETINUE_PUBLIC_IDENTITY_LEN] {
    let mut state = case ^ 0x9e37_79b9_7f4a_7c15;
    let mut identity = [0; RETINUE_PUBLIC_IDENTITY_LEN];
    for byte in &mut identity {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        *byte = (state >> 56) as u8;
    }
    identity
}

fn canonical_identity(case: u64) -> [u8; RETINUE_PUBLIC_IDENTITY_LEN] {
    let mut identity = generated_identity(case);
    let mut signing_seed = [0; 32];
    signing_seed.copy_from_slice(&generated_identity(case ^ u64::MAX)[..32]);
    identity[32..].copy_from_slice(
        SigningKey::from_bytes(&signing_seed)
            .verifying_key()
            .as_bytes(),
    );
    identity
}

#[test]
fn local_syntax_validator_matches_retinue_on_a_fixed_generated_corpus() {
    let mut raw_accepted = 0;
    let mut raw_rejected = 0;
    for case in 0..RAW_CORPUS_CASES {
        let identity = generated_identity(case as u64);
        let local = validate_retinue_public_identity(&identity).is_ok();
        let retinue = Identity::from_public_bytes(&identity).is_ok();
        assert_eq!(local, retinue, "arbitrary identity corpus case {case}");
        if retinue {
            raw_accepted += 1;
        } else {
            raw_rejected += 1;
        }
    }
    assert!(
        raw_accepted > 0,
        "generated corpus must include valid Ed25519 encodings"
    );
    assert!(
        raw_rejected > 0,
        "generated corpus must include invalid Ed25519 encodings"
    );

    for case in 0..CANONICAL_CORPUS_CASES {
        let identity = canonical_identity(case as u64);
        assert_eq!(
            validate_retinue_public_identity(&identity),
            Ok(()),
            "canonical identity corpus case {case}"
        );
        assert!(Identity::from_public_bytes(&identity).is_ok());
    }

    let mut malformed = [0; RETINUE_PUBLIC_IDENTITY_LEN];
    malformed[32..].fill(2);
    for identity in [malformed, [0xff; RETINUE_PUBLIC_IDENTITY_LEN]] {
        assert_eq!(
            validate_retinue_public_identity(&identity).is_ok(),
            Identity::from_public_bytes(&identity).is_ok()
        );
    }
}

#[test]
fn local_validator_keeps_the_slice_boundary_typed() {
    assert_eq!(
        validate_retinue_public_identity(&canonical_identity(0x41)[..63]),
        Err(PublicIdentityError::Length)
    );
}
