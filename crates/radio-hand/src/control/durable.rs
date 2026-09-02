//! WN1's bounded, board-independent durable configuration transaction journal.
//!
//! The public facade is deliberately small. `model` owns state transitions and
//! `codec` owns bytes over the existing CRC-protected A/B records.

mod model;

pub use model::{
    AbSlotStore, BoardRecoveryFacts, CHANGE_ID_LEN, CachedReceipt, ChangeId, DurableConfig,
    DurableError, DurableLoadError, DurableState, JournalWrite, KnownGood, MAX_DURABLE_BODY,
    MAX_OWNER_GRANTS, MAX_PUBLIC_CONFIG, MAX_SEALED_CREDENTIALS, MUTATION_SEQUENCE_WINDOW,
    OwnerGrant, Provisional, Recovery, RecoveryClause, RecoveryPathFacts, RecoveryPolicy,
    RecoveryPolicyError, SEMANTIC_TAG_LEN, SemanticTag, SemanticTagKey, Transition,
    VerifiedCounterError, decode_durable, encode_durable, load, next_record,
};
pub use model::{
    AbandonOutcome, AbandonResponse, CLAIM_PROOF_LEN, CLAIM_REQUEST_LEN, ClaimChallenge,
    ClaimProofError, ClaimRequest, ClaimResponse, FIRST_OWNER_VERSION, FirstOwnerRequest,
    FirstOwnerResponse, FirstOwnerWireError, FirstWriteActions, FirstWriteEligibility,
    FirstWriteIo, FirstWritePreparationError, FirstWriteScratch, FirstWriteScratchError,
    FirstWriteStatus, FirstWriteStorageError, FirstWriteStore, INSPECT_RESPONSE_LEN, PairEvidence,
    ResumeOutcome, ResumeResponse, StageOutcome, abandon_first_write, claim_proof_transcript,
    first_write_status, inspect_first_write, resume_first_write, stage_first_write,
};
pub use model::{
    FirstWriteBoot, FirstWriteError, FirstWriteLoadError, OWNER_CLAIM_LEN, OwnerClaim,
    OwnerClaimError, arbitrate_first_write, encode_first_write_state, load_first_write_state,
    next_first_write_record, validate_first_write_state, validate_resumable_first_write_state,
};
