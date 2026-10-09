//! Portable first-owner wire contract and power-cut-safe first-write storage.

use ed25519_dalek::{Signer, SigningKey};
use heapless::Vec;
use seneschal::control::{
    BoardRecoveryFacts, CLAIM_PROOF_LEN, ClaimRequest, DurableState, ManagementCarrier,
    ManagementCarrierSet, NodeId, OwnerClaim, PublicConfigurationV1, RecoveryClause,
    RecoveryPathFacts, RecoveryPolicy, claim_proof_transcript,
};
use seneschal::region::Region;

mod storage;
mod store;
mod wire;

const PAGE: usize = 4096;
const NODE: NodeId = NodeId([0x10; 16]);

fn identity(seed: u8) -> [u8; 64] {
    let mut identity = [seed; 64];
    identity[32..].copy_from_slice(
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .as_bytes(),
    );
    identity
}

fn configuration() -> PublicConfigurationV1 {
    PublicConfigurationV1::new(
        Region::Us915,
        selvage::PhyProfile::meshtastic_long_fast(906_875_000),
        seneschal::control::ReticulumTransportPolicy::new(false, false, 0).unwrap(),
        ManagementCarrierSet::from_mask(1 << ManagementCarrier::Usb as u8).unwrap(),
    )
    .unwrap()
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

fn claim() -> OwnerClaim {
    OwnerClaim::new(&identity(0x31), configuration(), policy()).unwrap()
}

fn state() -> DurableState {
    DurableState::from_owner_claim(NODE, claim(), &facts()).unwrap()
}

fn signed_request(node: NodeId, nonce: [u8; 32], claim: OwnerClaim) -> ClaimRequest {
    let mut transcript = [0; CLAIM_PROOF_LEN];
    claim_proof_transcript(node, nonce, &claim, &mut transcript);
    let signature = SigningKey::from_bytes(&[0x31; 32])
        .sign(&transcript)
        .to_bytes();
    ClaimRequest::new(node, nonce, claim, signature)
}
