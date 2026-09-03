//! Radio-free, carrier-neutral wall-node control contract.
//!
//! The semantic request lives inside `retinue::command::Command`; the outer command retains
//! target, signer, counter, signature, and verification authority. This module is only the
//! bounded inner opcode grammar, replies, capability facts, and WN0 volatile admission seam.

mod admission;
mod arguments;
mod codec;
mod durable;
mod model;
mod position_disclosure;
mod public_configuration;
mod public_identity;
#[cfg(feature = "control-retinue")]
mod retinue_command;
mod runtime;
mod status;

pub use admission::{Admission, RequestAdmission};
pub use arguments::{
    ArgumentsError, COMMIT_ARGUMENTS_LEN, CommitArguments, PROVISIONAL_APPLY_ARGUMENTS_LEN,
    ProvisionalApplyArguments, REVERT_ARGUMENTS_LEN, RevertArguments,
};
pub use codec::{decode_request, decode_response, encode_request, encode_response};
pub use durable::{
    AbSlotStore, BoardRecoveryFacts, CHANGE_ID_LEN, CachedReceipt, ChangeId, DurableConfig,
    DurableError, DurableLoadError, DurableState, JournalWrite, KnownGood, MAX_DURABLE_BODY,
    MAX_OWNER_GRANTS, MAX_PUBLIC_CONFIG, MAX_SEALED_CREDENTIALS, MUTATION_SEQUENCE_WINDOW,
    OwnerGrant, Provisional, Recovery, RecoveryClause, RecoveryPathFacts, RecoveryPolicy,
    RecoveryPolicyError, SEMANTIC_TAG_LEN, SemanticTag, SemanticTagKey, Transition,
    VerifiedCounterError, decode_durable, encode_durable, load, next_record,
};
pub use durable::{
    AbandonOutcome, AbandonResponse, CLAIM_PROOF_LEN, CLAIM_REQUEST_LEN, ClaimChallenge,
    ClaimProofError, ClaimRequest, ClaimResponse, FIRST_OWNER_VERSION, FirstOwnerRequest,
    FirstOwnerResponse, FirstOwnerWireError, FirstWriteActions, FirstWriteEligibility,
    FirstWriteIo, FirstWritePreparationError, FirstWriteScratch, FirstWriteScratchError,
    FirstWriteStatus, FirstWriteStorageError, FirstWriteStore, INSPECT_RESPONSE_LEN, PairEvidence,
    ResumeOutcome, ResumeResponse, StageOutcome, abandon_first_write, claim_proof_transcript,
    first_write_status, inspect_first_write, resume_first_write, stage_first_write,
};
pub use durable::{
    FirstWriteBoot, FirstWriteError, FirstWriteLoadError, OWNER_CLAIM_LEN, OwnerClaim,
    OwnerClaimError, arbitrate_first_write, encode_first_write_state, load_first_write_state,
    next_first_write_record, validate_first_write_state, validate_resumable_first_write_state,
};
pub use model::{
    AdapterCapability, BoardClass, COMMAND_OPCODE, COMMIT_TOKEN_LEN, Capabilities,
    CarrierCapability, ConfigGeneration, ControllerId, ControllerRole, DecodeError, Disposition,
    EncodeError, GOLDEN_REQUEST, GOLDEN_RESPONSE, ID_LEN, ImageKind, ImageSlot, MAX_ADAPTERS,
    MAX_ARGUMENTS, MAX_CARRIERS, MAX_IMAGE_SLOTS, MAX_RADIOS, MAX_RECOVERY_PATHS, MAX_REQUEST_LEN,
    MAX_RESPONSE_LEN, MAX_RESULT, ManagementCarrier, NodeId, Operation, RadioCapability, RadioKind,
    RecoveryPath, Refusal, Request, ResidentAdapter, Response, ResponseBody, TransactionId,
    VERSION, VerifiedController,
};
pub use position_disclosure::{
    AbsentPolicy, BlindedPositionAcl, DisclosureTier, POSITION_ACL_ENTRY_LEN,
    POSITION_ACL_HASH_LEN, POSITION_ACL_HEADER_LEN, POSITION_ACL_SECRET_LEN, POSITION_ACL_TAG_LEN,
    POSITION_ACL_V1_VERSION, PositionAclEntry, PositionAclError, PositionAclV1, Resolved,
};
pub use public_configuration::{
    ManagementCarrierSet, PUBLIC_CONFIGURATION_V1_LEN, PUBLIC_CONFIGURATION_V1_VERSION,
    PublicConfigurationError, PublicConfigurationV1, ReticulumTransportPolicy,
};
pub use public_identity::{
    PublicIdentityError, RETINUE_PUBLIC_IDENTITY_LEN, validate_retinue_public_identity,
};
#[cfg(feature = "control-retinue")]
pub use retinue_command::{
    CONTROL_COMMAND_FRAME_TAG, CONTROL_RESPONSE_FRAME_TAG, ControlFrameError, ControlVerifier,
    InboundControl, InboundControlError, MAX_CONTROL_COMMAND_FRAME_LEN,
    MAX_CONTROL_RESPONSE_FRAME_LEN, MIN_CONTROL_COMMAND_FRAME_LEN, VerifierRestoreError,
    decode_command_frame, decode_response_frame, decode_verified_command, encode_command_frame,
    encode_response_frame, restore_control_verifier, restore_verifier,
};
pub use runtime::{
    BootState, ConfigApplier, ControlRuntime, DurableScratch, DurableScratchError, LiveOutcome,
    MAX_PROVISIONAL_LIFETIME_MS, MIN_DURABLE_SLOT_BYTES, MIN_PROVISIONAL_LIFETIME_MS,
    PreparedCommit, PreparedProvisional, QuietExit, QuietGuard, QuietWindow, RuntimeError,
};
pub use status::{
    CONTROL_STATUS_FRAME_LEN, CONTROL_STATUS_FRAME_TAG, CONTROL_STATUS_NONCE_LEN,
    CONTROL_STATUS_REQUEST_FRAME_LEN, CONTROL_STATUS_REQUEST_FRAME_TAG, CONTROL_STATUS_V1_LEN,
    CONTROL_STATUS_VERSION, ControlStatusAuthority, ControlStatusBootFact, ControlStatusError,
    ControlStatusEvidence, ControlStatusRequestV1, ControlStatusV1,
};
