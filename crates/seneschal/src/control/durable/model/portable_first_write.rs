//! Carrier-neutral first-owner claim bytes and power-cut-safe first-write I/O.
//!
//! KISS, USB packetization, physical presence, and entropy acquisition belong
//! to a board carrier. This module accepts only already-framed exact bytes and
//! has no allocator or carrier dependency.

mod request;
mod response;
mod storage;

pub use request::{
    ClaimChallenge, ClaimProofError, ClaimRequest, FirstOwnerRequest, FirstOwnerWireError,
    claim_proof_transcript,
};
pub use response::{
    AbandonResponse, ClaimResponse, FirstOwnerResponse, FirstWriteActions, FirstWriteEligibility,
    FirstWriteStatus, PairEvidence, ResumeResponse,
};
pub use storage::{
    AbandonOutcome, FirstWriteIo, FirstWritePreparationError, FirstWriteScratch,
    FirstWriteScratchError, FirstWriteStorageError, FirstWriteStore, ResumeOutcome, StageOutcome,
    abandon_first_write, first_write_status, inspect_first_write, resume_first_write,
    stage_first_write,
};

use super::OWNER_CLAIM_LEN;

/// Version of the literal first-owner carrier contract.
pub const FIRST_OWNER_VERSION: u8 = 1;
const REQUEST_INSPECT: u8 = 1;
const REQUEST_CLAIM: u8 = 2;
const REQUEST_RESUME: u8 = 3;
const REQUEST_ABANDON: u8 = 4;
const RESPONSE_BIT: u8 = 0x80;
const NODE_ID_LEN: usize = 16;
const NONCE_LEN: usize = 32;
const SIGNATURE_LEN: usize = 64;
const CLAIM_PREFIX_LEN: usize = 2 + NODE_ID_LEN + NONCE_LEN + OWNER_CLAIM_LEN;
/// Exact bytes covered by a claim proof, including its domain separator.
pub const CLAIM_PROOF_LEN: usize = 28 + 1 + NODE_ID_LEN + NONCE_LEN + OWNER_CLAIM_LEN;
/// Exact literal-carrier length of a claim request.
pub const CLAIM_REQUEST_LEN: usize = CLAIM_PREFIX_LEN + SIGNATURE_LEN;
/// Exact literal-carrier length of an inspect response.
pub const INSPECT_RESPONSE_LEN: usize = 2 + 3 + NODE_ID_LEN + NONCE_LEN;
pub(super) const CLAIM_DOMAIN: &[u8; 28] = b"retinue:first-owner:claim:v1";
