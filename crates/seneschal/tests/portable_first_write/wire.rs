//! Exact request/response bytes, inspect action bits, and claim proof binding.

use ed25519_dalek::SigningKey;
use seneschal::control::{
    CLAIM_REQUEST_LEN, ClaimChallenge, ClaimProofError, ClaimRequest, FIRST_OWNER_VERSION,
    FirstOwnerRequest, FirstOwnerResponse, FirstWriteActions, FirstWriteStatus,
    INSPECT_RESPONSE_LEN, ManagementCarrierSet, NodeId, OwnerClaim, PairEvidence,
    PublicConfigurationV1, RecoveryClause, RecoveryPolicy,
};
use seneschal::region::Region;

use super::{NODE, claim, configuration, identity, policy, signed_request};

#[test]
fn exact_wire_round_trips_and_rejects_truncation_trailing_versions_and_opcodes() {
    let request = FirstOwnerRequest::Claim(signed_request(NODE, [0x43; 32], claim()));
    let mut bytes = [0; CLAIM_REQUEST_LEN];
    assert_eq!(request.encode(&mut bytes), Ok(CLAIM_REQUEST_LEN));
    assert_eq!(FirstOwnerRequest::decode(&bytes), Ok(request.clone()));
    assert!(FirstOwnerRequest::decode(&bytes[..bytes.len() - 1]).is_err());
    let mut trailing = [0; CLAIM_REQUEST_LEN + 1];
    trailing[..CLAIM_REQUEST_LEN].copy_from_slice(&bytes);
    assert!(FirstOwnerRequest::decode(&trailing).is_err());
    bytes[0] = FIRST_OWNER_VERSION + 1;
    assert!(FirstOwnerRequest::decode(&bytes).is_err());
    bytes[0] = FIRST_OWNER_VERSION;
    bytes[1] = 0x7f;
    assert!(FirstOwnerRequest::decode(&bytes).is_err());

    let response = FirstOwnerResponse::Inspect {
        status: FirstWriteStatus {
            control: PairEvidence::Blank,
            pending: PairEvidence::Blank,
        },
        node: NODE,
        nonce: [0x52; 32],
    };
    let mut response_bytes = [0; INSPECT_RESPONSE_LEN];
    assert_eq!(
        response.encode(&mut response_bytes),
        Ok(INSPECT_RESPONSE_LEN)
    );
    assert_eq!(FirstOwnerResponse::decode(&response_bytes), Ok(response));
    response_bytes[4] = 3;
    assert!(FirstOwnerResponse::decode(&response_bytes).is_err());
    let response = FirstOwnerResponse::Inspect {
        status: FirstWriteStatus {
            control: PairEvidence::Blank,
            pending: PairEvidence::Blank,
        },
        node: NODE,
        nonce: [0x52; 32],
    };
    let mut response_bytes = [0; INSPECT_RESPONSE_LEN];
    response.encode(&mut response_bytes).unwrap();
    assert!(FirstOwnerResponse::decode(&response_bytes[..INSPECT_RESPONSE_LEN - 1]).is_err());
    let mut trailing_response = [0; INSPECT_RESPONSE_LEN + 1];
    trailing_response[..INSPECT_RESPONSE_LEN].copy_from_slice(&response_bytes);
    assert!(FirstOwnerResponse::decode(&trailing_response).is_err());
    assert!(!format!("{response:?}").contains("82"));
}

#[test]
fn every_simple_request_and_response_variant_is_exact_and_rejects_bad_dispositions() {
    for request in [
        FirstOwnerRequest::Inspect,
        FirstOwnerRequest::Resume,
        FirstOwnerRequest::Abandon,
    ] {
        let mut bytes = [0; 2];
        assert_eq!(request.encode(&mut bytes), Ok(2));
        assert_eq!(FirstOwnerRequest::decode(&bytes), Ok(request));
        assert!(FirstOwnerRequest::decode(&[bytes[0], bytes[1], 0]).is_err());
    }
    for response in [
        FirstOwnerResponse::Claim(seneschal::control::ClaimResponse::Rejected),
        FirstOwnerResponse::Claim(seneschal::control::ClaimResponse::Staged),
        FirstOwnerResponse::Resume(seneschal::control::ResumeResponse::Rejected),
        FirstOwnerResponse::Resume(seneschal::control::ResumeResponse::Committed),
        FirstOwnerResponse::Resume(seneschal::control::ResumeResponse::CommittedCleanupPending),
        FirstOwnerResponse::Abandon(seneschal::control::AbandonResponse::Rejected),
        FirstOwnerResponse::Abandon(seneschal::control::AbandonResponse::Abandoned),
    ] {
        let mut bytes = [0; 3];
        assert_eq!(response.encode(&mut bytes), Ok(3));
        assert_eq!(FirstOwnerResponse::decode(&bytes), Ok(response));
        assert!(FirstOwnerResponse::decode(&bytes[..2]).is_err());
        assert!(FirstOwnerResponse::decode(&[bytes[0], bytes[1], bytes[2], 0]).is_err());
    }
    assert!(FirstOwnerResponse::decode(&[FIRST_OWNER_VERSION, 0x82, 0x7f]).is_err());
    assert!(FirstOwnerResponse::decode(&[FIRST_OWNER_VERSION, 0x83, 0x7f]).is_err());
    assert!(FirstOwnerResponse::decode(&[FIRST_OWNER_VERSION, 0x84, 0x7f]).is_err());
}

#[test]
fn inspect_action_bits_are_exact_for_every_recovery_shape() {
    let blank = FirstWriteStatus {
        control: PairEvidence::Blank,
        pending: PairEvidence::Blank,
    }
    .actions();
    assert!(blank.permits_claim() && blank.permits_ordinary_service());
    assert!(!blank.permits_resume() && !blank.permits_abandon());
    let pending = FirstWriteStatus {
        control: PairEvidence::Blank,
        pending: PairEvidence::Valid,
    }
    .actions();
    assert!(pending.permits_resume() && pending.permits_abandon());
    let corrupt_pending = FirstWriteStatus {
        control: PairEvidence::Blank,
        pending: PairEvidence::Corrupt,
    }
    .actions();
    assert_eq!(corrupt_pending.bits(), FirstWriteActions::ABANDON);
    let repair = FirstWriteStatus {
        control: PairEvidence::Corrupt,
        pending: PairEvidence::Valid,
    }
    .actions();
    assert_eq!(repair.bits(), FirstWriteActions::RESUME);
}

#[test]
fn claim_proof_binds_every_authority_bearing_byte_and_is_one_shot() {
    let nonce = [0x81; 32];
    let request = signed_request(NODE, nonce, claim());
    assert_eq!(
        ClaimChallenge::from_fresh_entropy(nonce).verify(&request, NODE),
        Ok(claim())
    );
    assert_eq!(
        ClaimChallenge::from_fresh_entropy(nonce).verify(&request, NodeId([0x11; 16])),
        Err(ClaimProofError::WrongNode)
    );
    assert_eq!(
        ClaimChallenge::from_fresh_entropy([0x82; 32]).verify(&request, NODE),
        Err(ClaimProofError::WrongNonce)
    );

    let changed_config = OwnerClaim::new(
        &identity(0x31),
        PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(907_875_000),
            seneschal::control::ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap(),
        policy(),
    )
    .unwrap();
    let changed_recovery = OwnerClaim::new(
        &identity(0x31),
        configuration(),
        RecoveryPolicy::new(
            RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
            RecoveryClause::new(ManagementCarrierSet::from_mask(1).unwrap(), 1).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let mut changed_x25519 = identity(0x31);
    changed_x25519[0] ^= 1;
    let mut changed_ed25519 = identity(0x31);
    changed_ed25519[32..].copy_from_slice(
        SigningKey::from_bytes(&[0x32; 32])
            .verifying_key()
            .as_bytes(),
    );
    for changed in [
        changed_config,
        changed_recovery,
        OwnerClaim::new(&changed_x25519, configuration(), policy()).unwrap(),
        OwnerClaim::new(&changed_ed25519, configuration(), policy()).unwrap(),
    ] {
        let tampered = ClaimRequest::new(NODE, nonce, changed, *request.signature());
        assert_eq!(
            ClaimChallenge::from_fresh_entropy(nonce).verify(&tampered, NODE),
            Err(ClaimProofError::InvalidSignature)
        );
    }
    let mut malformed_identity = identity(0x31);
    malformed_identity[32..].fill(2);
    assert!(OwnerClaim::new(&malformed_identity, configuration(), policy()).is_err());
    let bad_signature = ClaimRequest::new(NODE, nonce, claim(), [0; 64]);
    assert_eq!(
        ClaimChallenge::from_fresh_entropy(nonce).verify(&bad_signature, NODE),
        Err(ClaimProofError::InvalidSignature)
    );
}
