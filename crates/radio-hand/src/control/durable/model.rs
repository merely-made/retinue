//! WN1's bounded, board-independent durable configuration transaction journal.
//!
//! This module decides which record firmware must persist and when it may apply
//! a risky configuration. It deliberately does not know a board's flash page
//! geometry, reset mechanism, clock, or network stack. Firmware supplies those
//! operations through [`AbSlotStore`] and must persist an armed provisional
//! record before applying its candidate configuration.

use core::fmt;

use crate::store::Slot;
use heapless::Vec;
use sha2::{Digest, Sha256};

use super::super::{
    COMMIT_TOKEN_LEN, ConfigGeneration, ControllerId, ControllerRole, MAX_RESULT, NodeId,
    Operation, PublicConfigurationV1, Refusal, Request, Response, ResponseBody, TransactionId,
    VerifiedController, validate_retinue_public_identity,
};

mod codec;
mod commissioning;
mod owner;
mod portable_first_write;
mod recovery;
mod semantic;
#[cfg(test)]
mod sequence_tests;
#[cfg(test)]
mod tests;
mod transaction;
mod transition;
pub use codec::{decode_durable, encode_durable, load, next_record};
pub use commissioning::{
    FirstWriteBoot, FirstWriteError, FirstWriteLoadError, OWNER_CLAIM_LEN, OwnerClaim,
    OwnerClaimError, arbitrate_first_write, encode_first_write_state, load_first_write_state,
    next_first_write_record, validate_first_write_state, validate_resumable_first_write_state,
};
pub use owner::{OwnerGrant, VerifiedCounterError};
pub use portable_first_write::{
    AbandonOutcome, AbandonResponse, CLAIM_PROOF_LEN, CLAIM_REQUEST_LEN, ClaimChallenge,
    ClaimProofError, ClaimRequest, ClaimResponse, FIRST_OWNER_VERSION, FirstOwnerRequest,
    FirstOwnerResponse, FirstOwnerWireError, FirstWriteActions, FirstWriteEligibility,
    FirstWriteIo, FirstWritePreparationError, FirstWriteScratch, FirstWriteScratchError,
    FirstWriteStatus, FirstWriteStorageError, FirstWriteStore, INSPECT_RESPONSE_LEN, PairEvidence,
    ResumeOutcome, ResumeResponse, StageOutcome, abandon_first_write, claim_proof_transcript,
    first_write_status, inspect_first_write, resume_first_write, stage_first_write,
};
pub use recovery::{
    BoardRecoveryFacts, RecoveryClause, RecoveryPathFacts, RecoveryPolicy, RecoveryPolicyError,
};
pub use semantic::{SemanticTag, SemanticTagKey};
pub use transition::Transition;

/// Explicit ceiling for the versioned state body inside one `store` A/B record.
/// It includes one known-good configuration, one provisional candidate, and one
/// cached terminal result. Board integration must reserve a slot large enough
/// for this body plus [`crate::store::HEADER_LEN`].
pub const MAX_DURABLE_BODY: usize = 1536;
pub const MAX_OWNER_GRANTS: usize = 4;
pub const MAX_PUBLIC_CONFIG: usize = super::super::PUBLIC_CONFIGURATION_V1_LEN;
pub const MAX_SEALED_CREDENTIALS: usize = 96;
pub const SEMANTIC_TAG_LEN: usize = 16;
pub const CHANGE_ID_LEN: usize = 16;

const MAGIC: [u8; 4] = *b"RHD1";
const VERSION: u8 = 3;
pub const MUTATION_SEQUENCE_WINDOW: u64 = 4096;

/// Stable identifier that ties the distinct stage, apply, commit, and revert
/// requests of one configuration change together. It is not an outer-command
/// transaction ID, so lifecycle requests remain independently replay-bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChangeId(pub [u8; CHANGE_ID_LEN]);

/// Minimal board adapter. The concrete flash partition, erase size, and write
/// alignment remain firmware facts. A caller uses [`next_record`] to prepare a
/// record, then calls erase/program/readback in its board-specific sequence.
pub trait AbSlotStore {
    type Error;

    fn read_slot(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error>;
    fn erase_slot(&mut self, slot: Slot) -> Result<(), Self::Error>;
    fn program_slot(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error>;
}

/// The non-secret and sealed portions of a board configuration.
///
/// `sealed_credentials` may contain ciphertext, never a plaintext credential.
/// It is intentionally redacted from `Debug`; callers must not put it in a
/// status result or an audit receipt either.
#[derive(Clone, PartialEq, Eq)]
pub struct DurableConfig {
    pub public: PublicConfigurationV1,
    pub sealed_credentials: Vec<u8, MAX_SEALED_CREDENTIALS>,
}
impl fmt::Debug for DurableConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DurableConfig")
            .field("public", &self.public)
            .field("sealed_credentials", &"[redacted]")
            .finish()
    }
}
#[derive(Clone, PartialEq, Eq)]
pub struct KnownGood {
    pub generation: ConfigGeneration,
    pub configuration: DurableConfig,
}
impl fmt::Debug for KnownGood {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KnownGood")
            .field("generation", &self.generation)
            .field("configuration", &self.configuration)
            .finish()
    }
}

#[derive(Clone, PartialEq, Eq)]
struct SemanticKey {
    transaction: TransactionId,
    transaction_sequence: u64,
    expected_generation: ConfigGeneration,
    operation: Operation,
    tag: SemanticTag,
}
impl SemanticKey {
    fn from_request(request: &Request, tag: SemanticTag) -> Self {
        Self {
            transaction: request.transaction,
            transaction_sequence: request.transaction_sequence,
            expected_generation: request.expected_generation,
            operation: request.operation,
            tag,
        }
    }

    fn matches(&self, request: &Request, tag: SemanticTag) -> bool {
        self.transaction == request.transaction
            && self.transaction_sequence == request.transaction_sequence
            && self.expected_generation == request.expected_generation
            && self.operation == request.operation
            && self.tag == tag
    }
}
impl fmt::Debug for SemanticKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SemanticKey")
            .field("transaction", &self.transaction)
            .field("transaction_sequence", &self.transaction_sequence)
            .field("expected_generation", &self.expected_generation)
            .field("operation", &self.operation)
            .field("tag", &self.tag)
            .finish()
    }
}

/// The only mutable transaction that can survive a reboot.
#[derive(Clone, PartialEq, Eq)]
pub struct Provisional {
    controller: ControllerId,
    change: ChangeId,
    semantic: SemanticKey,
    candidate_generation: ConfigGeneration,
    candidate: DurableConfig,
    deadline_ms: u64,
    commit_token: [u8; COMMIT_TOKEN_LEN],
    result: Vec<u8, MAX_RESULT>,
}
impl Provisional {
    /// The controller-chosen id shared by this candidate's apply, commit, and revert.
    pub const fn change(&self) -> ChangeId {
        self.change
    }
    /// The generation the board allocated for this candidate.
    pub const fn candidate_generation(&self) -> ConfigGeneration {
        self.candidate_generation
    }
    /// Board time after which the candidate rolls back without a commit.
    pub const fn deadline_ms(&self) -> u64 {
        self.deadline_ms
    }
}
impl fmt::Debug for Provisional {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Provisional")
            .field("controller", &self.controller)
            .field("change", &self.change)
            .field("semantic", &self.semantic)
            .field("candidate_generation", &self.candidate_generation)
            .field("candidate", &self.candidate)
            .field("deadline_ms", &self.deadline_ms)
            .field("commit_token", &"[redacted]")
            .field("result_len", &self.result.len())
            .finish()
    }
}

/// One terminal response retained for fresh-counter replay of the same semantic
/// transaction. It has no commit token and is not an audit log.
#[derive(Clone, PartialEq, Eq)]
pub struct CachedReceipt {
    controller: ControllerId,
    semantic: SemanticKey,
    body: ReceiptBody,
}
#[derive(Clone, PartialEq, Eq)]
enum ReceiptBody {
    Applied {
        known_good_generation: ConfigGeneration,
        result: Vec<u8, MAX_RESULT>,
    },
    Refused(Refusal),
}
impl fmt::Debug for CachedReceipt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CachedReceipt")
            .field("controller", &self.controller)
            .field("semantic", &self.semantic)
            .field("body", &"[metadata redacted]")
            .finish()
    }
}

/// Durable state: exactly one known-good configuration and at most one
/// provisional candidate. `generation_watermark` prevents a rolled-back
/// candidate generation from ever being allocated again.
#[derive(Clone, PartialEq, Eq)]
pub struct DurableState {
    node: NodeId,
    owner_grants: Vec<OwnerGrant, MAX_OWNER_GRANTS>,
    recovery_policy: RecoveryPolicy,
    known_good: KnownGood,
    generation_watermark: ConfigGeneration,
    provisional: Option<Provisional>,
    receipt: Option<CachedReceipt>,
}
impl fmt::Debug for DurableState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DurableState")
            .field("node", &self.node)
            .field("owner_grants", &self.owner_grants)
            .field("recovery_policy", &self.recovery_policy)
            .field("known_good", &self.known_good)
            .field("generation_watermark", &self.generation_watermark)
            .field("provisional", &self.provisional)
            .field("receipt", &self.receipt)
            .finish()
    }
}

/// What firmware must restore before opening ordinary network services.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)] // no_std: rollback must return its fixed configuration by value.
pub enum Recovery {
    None,
    Rollback { configuration: DurableConfig },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableError {
    BufferTooSmall,
    Malformed,
    NoValidSlot,
    UnsupportedVersion(u8),
    Capacity,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DurableLoadError {
    Blank,
    Corrupt,
    State(DurableError),
}

/// A fully encoded outer record ready for the board store's target slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JournalWrite {
    pub slot: Slot,
    pub sequence: u32,
    pub len: usize,
}

impl DurableState {
    pub fn new(
        node: NodeId,
        owner_grants: Vec<OwnerGrant, MAX_OWNER_GRANTS>,
        generation: ConfigGeneration,
        configuration: DurableConfig,
        recovery_policy: RecoveryPolicy,
        facts: &BoardRecoveryFacts,
    ) -> Result<Self, Refusal> {
        recovery::validate_policy_candidate(recovery_policy, &configuration, facts)?;
        let state = Self {
            node,
            owner_grants,
            recovery_policy,
            known_good: KnownGood {
                generation,
                configuration,
            },
            generation_watermark: generation,
            provisional: None,
            receipt: None,
        };
        state
            .validate_semantics()
            .map_err(|_| Refusal::InvalidArguments)?;
        Ok(state)
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn known_good(&self) -> &KnownGood {
        &self.known_good
    }

    pub const fn generation_watermark(&self) -> ConfigGeneration {
        self.generation_watermark
    }

    pub const fn provisional(&self) -> Option<&Provisional> {
        self.provisional.as_ref()
    }

    pub const fn receipt(&self) -> Option<&CachedReceipt> {
        self.receipt.as_ref()
    }

    pub fn owner_grants(&self) -> &[OwnerGrant] {
        &self.owner_grants
    }

    pub const fn recovery_policy(&self) -> RecoveryPolicy {
        self.recovery_policy
    }

    /// Records a counter already accepted by Retinue's verifier. Firmware must persist the
    /// resulting state with the control transition before acknowledging the command.
    pub fn advance_verified_outer_counter(
        &mut self,
        controller: VerifiedController,
        counter: u64,
    ) -> Result<(), VerifiedCounterError> {
        let grant = self
            .owner_grants
            .iter_mut()
            .find(|grant| grant.controller == controller.0)
            .ok_or(VerifiedCounterError::UnknownController)?;
        if counter <= grant.accepted_outer_counter {
            return Err(VerifiedCounterError::NotMonotonic);
        }
        grant.accepted_outer_counter = counter;
        Ok(())
    }

    /// Records an expired or rebooted provisional transaction as refused and
    /// returns the exact known-good configuration firmware must restore.
    pub fn rollback(&mut self) -> Recovery {
        let Some(provisional) = self.provisional.take() else {
            return Recovery::None;
        };
        self.receipt = Some(CachedReceipt {
            controller: provisional.controller,
            semantic: provisional.semantic,
            body: ReceiptBody::Refused(Refusal::TransactionExpired),
        });
        Recovery::Rollback {
            configuration: self.known_good.configuration.clone(),
        }
    }

    /// Applies deadline policy. Persist the resulting state before considering
    /// the rollback complete.
    pub fn expire(&mut self, now_ms: u64) -> Recovery {
        match self.provisional.as_ref() {
            Some(provisional) if now_ms >= provisional.deadline_ms => self.rollback(),
            _ => Recovery::None,
        }
    }

    /// Reboot semantics are stricter than time semantics: every armed record
    /// rolls back, even if its in-boot deadline has not elapsed. `deadline_ms`
    /// is never wall time and never authorizes a provisional record after boot.
    pub fn recover_after_reboot(&mut self) -> Recovery {
        self.rollback()
    }

    fn permits_configuration(&self, controller: ControllerId, candidate: &DurableConfig) -> bool {
        self.owner_grants.iter().any(|grant| {
            grant.controller == controller
                && match grant.role {
                    ControllerRole::Owner => true,
                    ControllerRole::Operator => {
                        candidate.public.region() == self.known_good.configuration.public.region()
                            && candidate.public.enabled_management_carriers()
                                == self
                                    .known_good
                                    .configuration
                                    .public
                                    .enabled_management_carriers()
                            && candidate.sealed_credentials
                                == self.known_good.configuration.sealed_credentials
                    }
                    ControllerRole::Observer | ControllerRole::Updater => false,
                }
        })
    }

    fn permits_provisional_commit(&self, controller: ControllerId) -> bool {
        self.owner_grants.iter().any(|grant| {
            grant.controller == controller
                && matches!(grant.role, ControllerRole::Operator | ControllerRole::Owner)
        })
    }

    /// Whether a verified controller may abandon the armed candidate. The same grants
    /// that may confirm a candidate may revert it.
    pub(crate) fn permits_provisional_revert(&self, controller: VerifiedController) -> bool {
        self.permits_provisional_commit(controller.0)
    }

    /// The invariant shared by construction, durable decoding, and commit.
    /// Treat every violation as corrupt durable state rather than repairing it.
    pub(super) fn validate_semantics(&self) -> Result<(), DurableError> {
        if self.recovery_policy.validate_structure().is_err()
            || !self
                .recovery_policy
                .configuration_satisfies(&self.known_good.configuration)
            || self.generation_watermark < self.known_good.generation
        {
            return Err(DurableError::Malformed);
        }
        let mut has_owner = false;
        for (index, grant) in self.owner_grants.iter().enumerate() {
            validate_retinue_public_identity(&grant.retinue_public_identity)
                .map_err(|_| DurableError::Malformed)?;
            let digest = Sha256::digest(grant.retinue_public_identity);
            if digest[..16] != grant.controller.0 {
                return Err(DurableError::Malformed);
            }
            if matches!(grant.role, ControllerRole::Owner) {
                has_owner = true;
            }
            if self.owner_grants[..index]
                .iter()
                .any(|prior| prior.controller == grant.controller)
            {
                return Err(DurableError::Malformed);
            }
        }
        if !has_owner {
            return Err(DurableError::Malformed);
        }
        if let Some(provisional) = &self.provisional
            && (!self
                .recovery_policy
                .configuration_satisfies(&provisional.candidate)
                || provisional.candidate_generation <= self.known_good.generation
                || provisional.candidate_generation != self.generation_watermark
                || provisional.semantic.operation != Operation::ProvisionalApply
                || !self.permits_configuration(provisional.controller, &provisional.candidate)
                || self
                    .owner_grants
                    .iter()
                    .find(|grant| grant.controller == provisional.controller)
                    .is_none_or(|grant| {
                        provisional.semantic.transaction_sequence > grant.accepted_mutation_sequence
                    }))
        {
            return Err(DurableError::Malformed);
        }
        if let Some(CachedReceipt {
            body:
                ReceiptBody::Applied {
                    known_good_generation,
                    ..
                },
            ..
        }) = &self.receipt
            && *known_good_generation != self.known_good.generation
        {
            return Err(DurableError::Malformed);
        }
        if let Some(receipt) = &self.receipt
            && self
                .owner_grants
                .iter()
                .find(|grant| grant.controller == receipt.controller)
                .is_none_or(|grant| {
                    receipt.semantic.transaction_sequence > grant.accepted_mutation_sequence
                })
        {
            return Err(DurableError::Malformed);
        }
        Ok(())
    }

    fn admit_mutation(
        &mut self,
        controller: ControllerId,
        request: &Request,
        semantic_tag: SemanticTag,
    ) -> Result<Option<Response>, Refusal> {
        if let Some(response) = self.replay(controller, request, semantic_tag)? {
            return Ok(Some(response));
        }
        let grant = self
            .owner_grants
            .iter_mut()
            .find(|grant| grant.controller == controller)
            .ok_or(Refusal::Unauthorized)?;
        if request.transaction_sequence <= grant.accepted_mutation_sequence {
            return Err(Refusal::TransactionExpired);
        }
        if request.transaction_sequence - grant.accepted_mutation_sequence
            > MUTATION_SEQUENCE_WINDOW
        {
            return Err(Refusal::TransactionTooFar);
        }
        grant.accepted_mutation_sequence = request.transaction_sequence;
        Ok(None)
    }

    fn cache_refusal(
        &mut self,
        controller: ControllerId,
        request: &Request,
        semantic_tag: SemanticTag,
        reason: Refusal,
    ) -> Transition {
        self.receipt = Some(CachedReceipt {
            controller,
            semantic: SemanticKey::from_request(request, semantic_tag),
            body: ReceiptBody::Refused(reason),
        });
        Transition::changed(self.refusal_response(request.transaction, reason))
    }

    fn replay(
        &self,
        controller: ControllerId,
        request: &Request,
        semantic_tag: SemanticTag,
    ) -> Result<Option<Response>, Refusal> {
        if let Some(provisional) = self.provisional.as_ref()
            && provisional.controller == controller
            && provisional.semantic.transaction_sequence == request.transaction_sequence
        {
            return if provisional.semantic.matches(request, semantic_tag) {
                Ok(Some(self.provisional_response()))
            } else {
                Err(Refusal::TransactionConflict)
            };
        }
        let Some(receipt) = self.receipt.as_ref() else {
            return Ok(None);
        };
        if receipt.controller != controller
            || receipt.semantic.transaction_sequence != request.transaction_sequence
        {
            return Ok(None);
        }
        if !receipt.semantic.matches(request, semantic_tag) {
            return Err(Refusal::TransactionConflict);
        }
        Ok(Some(match &receipt.body {
            ReceiptBody::Applied {
                known_good_generation,
                result,
            } => Response {
                node: self.node,
                transaction: request.transaction,
                known_good_generation: *known_good_generation,
                effective_generation: Some(*known_good_generation),
                body: ResponseBody::Applied(result.clone()),
            },
            ReceiptBody::Refused(reason) => self.refusal_response(request.transaction, *reason),
        }))
    }

    fn provisional_response(&self) -> Response {
        let provisional = self
            .provisional
            .as_ref()
            .expect("an armed transaction exists");
        Response {
            node: self.node,
            transaction: provisional.semantic.transaction,
            known_good_generation: self.known_good.generation,
            effective_generation: Some(provisional.candidate_generation),
            body: ResponseBody::Provisional {
                deadline_ms: provisional.deadline_ms,
                commit_token: provisional.commit_token,
                result: provisional.result.clone(),
            },
        }
    }

    fn refusal_response(&self, transaction: TransactionId, reason: Refusal) -> Response {
        Response {
            node: self.node,
            transaction,
            known_good_generation: self.known_good.generation,
            effective_generation: None,
            body: ResponseBody::Refused {
                reason,
                result: Vec::new(),
            },
        }
    }
}
