//! Carrier-neutral first-owner claim bytes and power-cut-safe first-write I/O.
//!
//! KISS, USB packetization, physical presence, and entropy acquisition belong
//! to a board carrier.  This module accepts only already-framed exact bytes and
//! has no allocator or carrier dependency.

use ed25519_dalek::{Signature, VerifyingKey};

use crate::store::{self, Slot};

use super::{
    BoardRecoveryFacts, DurableError, DurableLoadError, DurableState, FirstWriteError,
    FirstWriteLoadError, MAX_DURABLE_BODY, NodeId, OWNER_CLAIM_LEN, OwnerClaim, load,
    load_first_write_state, next_first_write_record, next_record,
};

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
const CLAIM_DOMAIN: &[u8; 28] = b"retinue:first-owner:claim:v1";

/// A freshly generated board challenge.  It deliberately cannot be cloned or
/// copied: the carrier sends [`Self::nonce`] while retaining this value, then
/// [`Self::verify`] consumes it exactly once.
pub struct ClaimChallenge {
    nonce: [u8; NONCE_LEN],
}

impl core::fmt::Debug for ClaimChallenge {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ClaimChallenge")
            .field("nonce", &"[redacted]")
            .finish()
    }
}

impl ClaimChallenge {
    /// Wraps one fresh 32-byte value supplied by a board true-entropy source.
    /// The source, freshness policy, and session expiry are carrier facts.
    pub const fn from_fresh_entropy(nonce: [u8; NONCE_LEN]) -> Self {
        Self { nonce }
    }

    /// The nonce to place in an inspect response.  Calling this does not
    /// consume the challenge; verification below does.
    pub const fn nonce(&self) -> [u8; NONCE_LEN] {
        self.nonce
    }

    /// Verifies and consumes this one challenge.  A caller must retain the
    /// resulting session outcome rather than reusing this value.
    pub fn verify(
        self,
        request: &ClaimRequest,
        expected_node: NodeId,
    ) -> Result<OwnerClaim, ClaimProofError> {
        if request.node != expected_node {
            return Err(ClaimProofError::WrongNode);
        }
        if request.nonce != self.nonce {
            return Err(ClaimProofError::WrongNonce);
        }
        let verifying_bytes: [u8; 32] = request.claim.owner_public_identity()[32..]
            .try_into()
            .expect("OwnerClaim validation guarantees the Ed25519 half");
        let key = VerifyingKey::from_bytes(&verifying_bytes)
            .map_err(|_| ClaimProofError::InvalidPublicKey)?;
        let signature = Signature::from_bytes(&request.signature);
        let mut transcript = [0; CLAIM_PROOF_LEN];
        claim_proof_transcript(request.node, request.nonce, &request.claim, &mut transcript);
        key.verify_strict(&transcript, &signature)
            .map_err(|_| ClaimProofError::InvalidSignature)?;
        Ok(request.claim.clone())
    }
}

/// Why a parsed Claim request cannot prove possession of its public identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimProofError {
    WrongNode,
    WrongNonce,
    InvalidPublicKey,
    InvalidSignature,
}

/// Produces the exact domain-separated proof transcript.  The byte sequence
/// binds the protocol version, opaque node id, challenge nonce, and the whole
/// canonical owner claim, including X25519 and Ed25519 identity halves.
pub fn claim_proof_transcript(
    node: NodeId,
    nonce: [u8; NONCE_LEN],
    claim: &OwnerClaim,
    out: &mut [u8; CLAIM_PROOF_LEN],
) {
    out[..CLAIM_DOMAIN.len()].copy_from_slice(CLAIM_DOMAIN);
    let mut cursor = CLAIM_DOMAIN.len();
    out[cursor] = FIRST_OWNER_VERSION;
    cursor += 1;
    out[cursor..cursor + NODE_ID_LEN].copy_from_slice(&node.0);
    cursor += NODE_ID_LEN;
    out[cursor..cursor + NONCE_LEN].copy_from_slice(&nonce);
    cursor += NONCE_LEN;
    let mut encoded_claim = [0; OWNER_CLAIM_LEN];
    claim.encode_canonical(&mut encoded_claim);
    out[cursor..].copy_from_slice(&encoded_claim);
}

/// A parsed owner-claim request.  Roles and board recovery facts are absent:
/// the first role is always Owner and board facts are local authority.
#[derive(Clone, PartialEq, Eq)]
pub struct ClaimRequest {
    node: NodeId,
    nonce: [u8; NONCE_LEN],
    claim: OwnerClaim,
    signature: [u8; SIGNATURE_LEN],
}

impl core::fmt::Debug for ClaimRequest {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("ClaimRequest")
            .field("node", &self.node)
            .field("nonce", &"[redacted]")
            .field("claim", &self.claim)
            .field("signature", &"[redacted]")
            .finish()
    }
}

impl ClaimRequest {
    /// Creates a parsed request from exactly the pieces a carrier received.
    /// The signature remains untrusted until a consumed [`ClaimChallenge`]
    /// verifies it.
    pub const fn new(
        node: NodeId,
        nonce: [u8; NONCE_LEN],
        claim: OwnerClaim,
        signature: [u8; SIGNATURE_LEN],
    ) -> Self {
        Self {
            node,
            nonce,
            claim,
            signature,
        }
    }

    pub const fn node(&self) -> NodeId {
        self.node
    }

    pub const fn nonce(&self) -> [u8; NONCE_LEN] {
        self.nonce
    }

    pub const fn claim(&self) -> &OwnerClaim {
        &self.claim
    }

    pub const fn signature(&self) -> &[u8; SIGNATURE_LEN] {
        &self.signature
    }
}

/// Literal carrier request names.  KISS framing is deliberately outside this
/// exact payload parser.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FirstOwnerRequest {
    Inspect,
    Claim(ClaimRequest),
    Resume,
    Abandon,
}

/// Strict request parsing failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstOwnerWireError {
    Length,
    UnsupportedVersion(u8),
    UnknownOpcode(u8),
    InvalidClaim,
    InvalidEvidence,
    InvalidDisposition,
}

impl FirstOwnerRequest {
    /// Parses one exact, unframed request.  Trailing bytes are an error.
    pub fn decode(bytes: &[u8]) -> Result<Self, FirstOwnerWireError> {
        if bytes.len() < 2 {
            return Err(FirstOwnerWireError::Length);
        }
        if bytes[0] != FIRST_OWNER_VERSION {
            return Err(FirstOwnerWireError::UnsupportedVersion(bytes[0]));
        }
        match bytes[1] {
            REQUEST_INSPECT if bytes.len() == 2 => Ok(Self::Inspect),
            REQUEST_RESUME if bytes.len() == 2 => Ok(Self::Resume),
            REQUEST_ABANDON if bytes.len() == 2 => Ok(Self::Abandon),
            REQUEST_CLAIM if bytes.len() == CLAIM_REQUEST_LEN => {
                let node = NodeId(bytes[2..2 + NODE_ID_LEN].try_into().expect("fixed slice"));
                let nonce = bytes[2 + NODE_ID_LEN..2 + NODE_ID_LEN + NONCE_LEN]
                    .try_into()
                    .expect("fixed slice");
                let claim_start = 2 + NODE_ID_LEN + NONCE_LEN;
                let claim = OwnerClaim::decode_canonical(
                    &bytes[claim_start..claim_start + OWNER_CLAIM_LEN],
                )
                .map_err(|_| FirstOwnerWireError::InvalidClaim)?;
                let signature = bytes[claim_start + OWNER_CLAIM_LEN..]
                    .try_into()
                    .expect("fixed slice");
                Ok(Self::Claim(ClaimRequest::new(
                    node, nonce, claim, signature,
                )))
            }
            REQUEST_INSPECT | REQUEST_CLAIM | REQUEST_RESUME | REQUEST_ABANDON => {
                Err(FirstOwnerWireError::Length)
            }
            opcode => Err(FirstOwnerWireError::UnknownOpcode(opcode)),
        }
    }

    /// Encodes one exact unframed request into the caller's fixed buffer.
    pub fn encode(&self, out: &mut [u8]) -> Result<usize, FirstOwnerWireError> {
        let len = match self {
            Self::Inspect | Self::Resume | Self::Abandon => 2,
            Self::Claim(_) => CLAIM_REQUEST_LEN,
        };
        if out.len() != len {
            return Err(FirstOwnerWireError::Length);
        }
        out[0] = FIRST_OWNER_VERSION;
        match self {
            Self::Inspect => out[1] = REQUEST_INSPECT,
            Self::Resume => out[1] = REQUEST_RESUME,
            Self::Abandon => out[1] = REQUEST_ABANDON,
            Self::Claim(request) => {
                out[1] = REQUEST_CLAIM;
                out[2..2 + NODE_ID_LEN].copy_from_slice(&request.node.0);
                out[2 + NODE_ID_LEN..2 + NODE_ID_LEN + NONCE_LEN].copy_from_slice(&request.nonce);
                let claim_start = 2 + NODE_ID_LEN + NONCE_LEN;
                let mut claim = [0; OWNER_CLAIM_LEN];
                request.claim.encode_canonical(&mut claim);
                out[claim_start..claim_start + OWNER_CLAIM_LEN].copy_from_slice(&claim);
                out[claim_start + OWNER_CLAIM_LEN..].copy_from_slice(&request.signature);
            }
        }
        Ok(len)
    }
}

/// Raw A/B evidence, independent of the action a board elects to permit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PairEvidence {
    Blank = 0,
    Valid = 1,
    Corrupt = 2,
}

/// Detailed first-write inspection result.  Its methods expose eligibility
/// without treating a corrupt nonblank pair as erased flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstWriteStatus {
    pub control: PairEvidence,
    pub pending: PairEvidence,
}

impl FirstWriteStatus {
    /// Normal modem/RNode service remains eligible only on a blank board with
    /// no staged claim.
    pub const fn ordinary_service_eligible(self) -> bool {
        matches!(self.control, PairEvidence::Blank) && matches!(self.pending, PairEvidence::Blank)
    }

    pub const fn claim_eligible(self) -> bool {
        self.ordinary_service_eligible()
    }

    pub const fn resume_eligible(self) -> bool {
        matches!(self.pending, PairEvidence::Valid)
            && matches!(self.control, PairEvidence::Blank | PairEvidence::Corrupt)
    }

    pub const fn abandon_eligible(self) -> bool {
        matches!(self.control, PairEvidence::Blank) && !matches!(self.pending, PairEvidence::Blank)
    }

    /// Every independently eligible action. Inspect carries this exact bitset
    /// because a valid pending record permits both resume and abandon, while a
    /// corrupt pending record permits only physical abandon.
    pub const fn actions(self) -> FirstWriteActions {
        let mut bits = 0;
        if self.claim_eligible() {
            bits |= FirstWriteActions::CLAIM;
        }
        if self.resume_eligible() {
            bits |= FirstWriteActions::RESUME;
        }
        if self.abandon_eligible() {
            bits |= FirstWriteActions::ABANDON;
        }
        if self.ordinary_service_eligible() {
            bits |= FirstWriteActions::ORDINARY_SERVICE;
        }
        FirstWriteActions(bits)
    }
}

/// Exact action-eligibility bitset sent in an Inspect response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FirstWriteActions(u8);

impl FirstWriteActions {
    pub const CLAIM: u8 = 1;
    pub const RESUME: u8 = 1 << 1;
    pub const ABANDON: u8 = 1 << 2;
    pub const ORDINARY_SERVICE: u8 = 1 << 3;
    const KNOWN: u8 = Self::CLAIM | Self::RESUME | Self::ABANDON | Self::ORDINARY_SERVICE;

    pub const fn bits(self) -> u8 {
        self.0
    }
    pub const fn permits_claim(self) -> bool {
        self.0 & Self::CLAIM != 0
    }
    pub const fn permits_resume(self) -> bool {
        self.0 & Self::RESUME != 0
    }
    pub const fn permits_abandon(self) -> bool {
        self.0 & Self::ABANDON != 0
    }
    pub const fn permits_ordinary_service(self) -> bool {
        self.0 & Self::ORDINARY_SERVICE != 0
    }
    const fn is_canonical(self) -> bool {
        self.0 & !Self::KNOWN == 0
    }
}

/// Exactly what an operation is permitted to do after inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWriteEligibility {
    Uncommissioned,
    Resume,
    ControlPresent,
    Fault,
}

impl FirstWriteStatus {
    pub const fn eligibility(self) -> FirstWriteEligibility {
        if matches!(self.control, PairEvidence::Valid) {
            FirstWriteEligibility::ControlPresent
        } else if self.resume_eligible() {
            FirstWriteEligibility::Resume
        } else if self.ordinary_service_eligible() {
            FirstWriteEligibility::Uncommissioned
        } else {
            FirstWriteEligibility::Fault
        }
    }
}

/// A response to literal first-owner carrier payloads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum FirstOwnerResponse {
    Inspect {
        status: FirstWriteStatus,
        /// Exact opaque target to bind in the requested claim proof.
        node: NodeId,
        nonce: [u8; NONCE_LEN],
    },
    Claim(ClaimResponse),
    Resume(ResumeResponse),
    Abandon(AbandonResponse),
}

impl core::fmt::Debug for FirstOwnerResponse {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Inspect { status, node, .. } => formatter
                .debug_struct("InspectResponse")
                .field("status", status)
                .field("node", node)
                .field("nonce", &"[redacted]")
                .finish(),
            Self::Claim(response) => formatter
                .debug_tuple("ClaimResponse")
                .field(response)
                .finish(),
            Self::Resume(response) => formatter
                .debug_tuple("ResumeResponse")
                .field(response)
                .finish(),
            Self::Abandon(response) => formatter
                .debug_tuple("AbandonResponse")
                .field(response)
                .finish(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ClaimResponse {
    Rejected = 0,
    Staged = 1,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ResumeResponse {
    Rejected = 0,
    Committed = 1,
    CommittedCleanupPending = 2,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AbandonResponse {
    Rejected = 0,
    Abandoned = 1,
}

impl FirstOwnerResponse {
    /// Parses one exact unframed response.  A host must not infer a response
    /// from a request-shaped frame.
    pub fn decode(bytes: &[u8]) -> Result<Self, FirstOwnerWireError> {
        if bytes.len() < 2 {
            return Err(FirstOwnerWireError::Length);
        }
        if bytes[0] != FIRST_OWNER_VERSION {
            return Err(FirstOwnerWireError::UnsupportedVersion(bytes[0]));
        }
        match bytes[1] {
            opcode
                if opcode == RESPONSE_BIT | REQUEST_INSPECT
                    && bytes.len() == INSPECT_RESPONSE_LEN =>
            {
                let status = FirstWriteStatus {
                    control: evidence(bytes[2])?,
                    pending: evidence(bytes[3])?,
                };
                let actions = FirstWriteActions(bytes[4]);
                if !actions.is_canonical() || actions != status.actions() {
                    return Err(FirstOwnerWireError::InvalidEvidence);
                }
                Ok(Self::Inspect {
                    status,
                    node: NodeId(bytes[5..5 + NODE_ID_LEN].try_into().expect("fixed slice")),
                    nonce: bytes[5 + NODE_ID_LEN..].try_into().expect("fixed slice"),
                })
            }
            opcode if opcode == RESPONSE_BIT | REQUEST_CLAIM && bytes.len() == 3 => {
                Ok(Self::Claim(claim_response(bytes[2])?))
            }
            opcode if opcode == RESPONSE_BIT | REQUEST_RESUME && bytes.len() == 3 => {
                Ok(Self::Resume(resume_response(bytes[2])?))
            }
            opcode if opcode == RESPONSE_BIT | REQUEST_ABANDON && bytes.len() == 3 => {
                Ok(Self::Abandon(abandon_response(bytes[2])?))
            }
            opcode if opcode & RESPONSE_BIT != 0 => Err(FirstOwnerWireError::Length),
            opcode => Err(FirstOwnerWireError::UnknownOpcode(opcode)),
        }
    }

    /// Encodes one exact unframed response.
    pub fn encode(self, out: &mut [u8]) -> Result<usize, FirstOwnerWireError> {
        let len = if matches!(self, Self::Inspect { .. }) {
            INSPECT_RESPONSE_LEN
        } else {
            3
        };
        if out.len() != len {
            return Err(FirstOwnerWireError::Length);
        }
        out[0] = FIRST_OWNER_VERSION;
        match self {
            Self::Inspect {
                status,
                node,
                nonce,
            } => {
                out[1] = RESPONSE_BIT | REQUEST_INSPECT;
                out[2] = status.control as u8;
                out[3] = status.pending as u8;
                out[4] = status.actions().bits();
                out[5..5 + NODE_ID_LEN].copy_from_slice(&node.0);
                out[5 + NODE_ID_LEN..].copy_from_slice(&nonce);
            }
            Self::Claim(response) => {
                out[1] = RESPONSE_BIT | REQUEST_CLAIM;
                out[2] = response as u8;
            }
            Self::Resume(response) => {
                out[1] = RESPONSE_BIT | REQUEST_RESUME;
                out[2] = response as u8;
            }
            Self::Abandon(response) => {
                out[1] = RESPONSE_BIT | REQUEST_ABANDON;
                out[2] = response as u8;
            }
        }
        Ok(len)
    }
}

fn evidence(value: u8) -> Result<PairEvidence, FirstOwnerWireError> {
    match value {
        0 => Ok(PairEvidence::Blank),
        1 => Ok(PairEvidence::Valid),
        2 => Ok(PairEvidence::Corrupt),
        _ => Err(FirstOwnerWireError::InvalidEvidence),
    }
}
fn claim_response(value: u8) -> Result<ClaimResponse, FirstOwnerWireError> {
    match value {
        0 => Ok(ClaimResponse::Rejected),
        1 => Ok(ClaimResponse::Staged),
        _ => Err(FirstOwnerWireError::InvalidDisposition),
    }
}
fn resume_response(value: u8) -> Result<ResumeResponse, FirstOwnerWireError> {
    match value {
        0 => Ok(ResumeResponse::Rejected),
        1 => Ok(ResumeResponse::Committed),
        2 => Ok(ResumeResponse::CommittedCleanupPending),
        _ => Err(FirstOwnerWireError::InvalidDisposition),
    }
}
fn abandon_response(value: u8) -> Result<AbandonResponse, FirstOwnerWireError> {
    match value {
        0 => Ok(AbandonResponse::Rejected),
        1 => Ok(AbandonResponse::Abandoned),
        _ => Err(FirstOwnerWireError::InvalidDisposition),
    }
}

/// Separate pending and ordinary-control A/B storage.  Board adapters decide
/// partitions and flash alignment; this portable contract decides their safe
/// ordering.
pub trait FirstWriteStore {
    type Error;
    fn read_control(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error>;
    fn erase_control(&mut self, slot: Slot) -> Result<(), Self::Error>;
    fn program_control(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error>;
    fn read_pending(&mut self, slot: Slot, out: &mut [u8]) -> Result<(), Self::Error>;
    fn erase_pending(&mut self, slot: Slot) -> Result<(), Self::Error>;
    fn program_pending(&mut self, slot: Slot, record: &[u8]) -> Result<(), Self::Error>;
}

/// Fixed caller-owned scratch.  It keeps all first-write operations usable by
/// a core-only board image without an allocator.
pub struct FirstWriteScratch<'a> {
    control_a: &'a mut [u8],
    control_b: &'a mut [u8],
    pending_a: &'a mut [u8],
    pending_b: &'a mut [u8],
    record_body: &'a mut [u8; MAX_DURABLE_BODY],
    record_page: &'a mut [u8],
    readback: &'a mut [u8],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWriteScratchError {
    UnequalSlotLengths,
}

impl<'a> FirstWriteScratch<'a> {
    /// Makes scratch only when every store read buffer has the same exact slot
    /// length. The record page may be larger, but never smaller, so a board
    /// adapter can safely copy any of its A/B slots into these buffers.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        control_a: &'a mut [u8],
        control_b: &'a mut [u8],
        pending_a: &'a mut [u8],
        pending_b: &'a mut [u8],
        record_body: &'a mut [u8; MAX_DURABLE_BODY],
        record_page: &'a mut [u8],
        readback: &'a mut [u8],
    ) -> Result<Self, FirstWriteScratchError> {
        let slot_len = control_a.len();
        if control_b.len() != slot_len
            || pending_a.len() != slot_len
            || pending_b.len() != slot_len
            || readback.len() != slot_len
            || record_page.len() < slot_len
        {
            return Err(FirstWriteScratchError::UnequalSlotLengths);
        }
        Ok(Self {
            control_a,
            control_b,
            pending_a,
            pending_b,
            record_body,
            record_page,
            readback,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWriteIo {
    ReadControl(Slot),
    ReadPending(Slot),
    EraseControl(Slot),
    ProgramControl(Slot),
    ErasePending(Slot),
    ProgramPending(Slot),
    VerifyControl(Slot),
    VerifyPending(Slot),
}

#[derive(Debug, PartialEq, Eq)]
pub enum FirstWriteStorageError<E> {
    Store { operation: FirstWriteIo, error: E },
    Ineligible(FirstWriteStatus),
    Preparation(FirstWritePreparationError),
    ReadbackMismatch { pending: bool, slot: Slot },
    InvalidPending(FirstWriteError),
}

/// A local encoding failure before any erase/program operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirstWritePreparationError {
    Durable(DurableError),
    RecordBuffer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageOutcome {
    Staged,
}
#[derive(Debug, PartialEq, Eq)]
pub enum ResumeOutcome<E> {
    AlreadyControlPresent,
    Committed,
    CommittedWithCleanupFailure(FirstWriteStorageError<E>),
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbandonOutcome {
    NothingStaged,
    Abandoned,
}

/// Reads and classifies both storage pairs without changing boot arbitration.
pub fn inspect_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<FirstWriteStatus, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    Ok(first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    ))
}

/// Classifies raw A/B bytes with full blank/valid/corrupt evidence.
pub fn first_write_status(
    control_a: &[u8],
    control_b: &[u8],
    pending_a: &[u8],
    pending_b: &[u8],
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> FirstWriteStatus {
    let control = match load(control_a, control_b) {
        Ok(_) => PairEvidence::Valid,
        Err(DurableLoadError::Blank) => PairEvidence::Blank,
        Err(_) => PairEvidence::Corrupt,
    };
    let pending = match load_first_write_state(pending_a, pending_b, expected_node, facts) {
        Ok(_) => PairEvidence::Valid,
        Err(FirstWriteLoadError::Blank) => PairEvidence::Blank,
        Err(_) => PairEvidence::Corrupt,
    };
    FirstWriteStatus { control, pending }
}

/// Stages only the pending pair, and only on an entirely blank board.
pub fn stage_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    state: &DurableState,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<StageOutcome, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    let status = first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    );
    if !status.claim_eligible() {
        return Err(FirstWriteStorageError::Ineligible(status));
    }
    let write = next_first_write_record(
        scratch.pending_a,
        scratch.pending_b,
        state,
        expected_node,
        facts,
        scratch.record_body,
        scratch.record_page,
    )
    .map_err(FirstWriteStorageError::InvalidPending)?;
    store
        .erase_pending(write.slot)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ErasePending(write.slot),
            error,
        })?;
    store
        .program_pending(write.slot, &scratch.record_page[..write.len])
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ProgramPending(write.slot),
            error,
        })?;
    store
        .read_pending(write.slot, scratch.readback)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::VerifyPending(write.slot),
            error,
        })?;
    let (a, b) = select_written(
        write.slot,
        scratch.readback,
        scratch.pending_a,
        scratch.pending_b,
    );
    match load_first_write_state(a, b, expected_node, facts) {
        Ok(readback) if readback == *state => Ok(StageOutcome::Staged),
        Ok(_) | Err(_) => Err(FirstWriteStorageError::ReadbackMismatch {
            pending: true,
            slot: write.slot,
        }),
    }
}

/// Commits an exact valid pending state to ordinary control before any pending
/// cleanup.  A cleanup fault is returned as a durable-commit outcome, not as a
/// claim that the control write failed.
pub fn resume_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<ResumeOutcome<S::Error>, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    let status = first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    );
    if matches!(status.control, PairEvidence::Valid) {
        return Ok(ResumeOutcome::AlreadyControlPresent);
    }
    if !status.resume_eligible() {
        return Err(FirstWriteStorageError::Ineligible(status));
    }
    let pending =
        load_first_write_state(scratch.pending_a, scratch.pending_b, expected_node, facts)
            .map_err(|error| match error {
                FirstWriteLoadError::Corrupt(reason) => {
                    FirstWriteStorageError::InvalidPending(reason)
                }
                FirstWriteLoadError::Blank => FirstWriteStorageError::Ineligible(status),
            })?;
    let write = if matches!(status.control, PairEvidence::Corrupt) {
        let body_len = super::encode_durable(&pending, scratch.record_body).map_err(|error| {
            FirstWriteStorageError::Preparation(FirstWritePreparationError::Durable(error))
        })?;
        // The outer record may be CRC-valid even though its durable body is
        // malformed. Preserve its sequence ordering so this repair wins A/B
        // selection. At MAX, overwrite the selected malformed slot at MAX:
        // equal-sequence selection deterministically keeps that same slot.
        let selection = store::select(scratch.control_a, scratch.control_b);
        let (slot, sequence) = match selection.active {
            Some((slot, record)) if record.sequence == u32::MAX => (slot, u32::MAX),
            _ => (selection.next, selection.next_sequence),
        };
        let len = store::encode(
            sequence,
            &scratch.record_body[..body_len],
            scratch.record_page,
        )
        .map_err(|_| {
            FirstWriteStorageError::Preparation(FirstWritePreparationError::RecordBuffer)
        })?;
        super::JournalWrite {
            slot,
            sequence,
            len,
        }
    } else {
        next_record(
            scratch.control_a,
            scratch.control_b,
            &pending,
            scratch.record_body,
            scratch.record_page,
        )
        .map_err(|error| {
            FirstWriteStorageError::Preparation(FirstWritePreparationError::Durable(error))
        })?
    };
    store
        .erase_control(write.slot)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::EraseControl(write.slot),
            error,
        })?;
    store
        .program_control(write.slot, &scratch.record_page[..write.len])
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ProgramControl(write.slot),
            error,
        })?;
    store
        .read_control(write.slot, scratch.readback)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::VerifyControl(write.slot),
            error,
        })?;
    let (a, b) = select_written(
        write.slot,
        scratch.readback,
        scratch.control_a,
        scratch.control_b,
    );
    if !matches!(load(a, b), Ok(readback) if readback == pending) {
        return Err(FirstWriteStorageError::ReadbackMismatch {
            pending: false,
            slot: write.slot,
        });
    }
    for slot in [Slot::A, Slot::B] {
        if let Err(error) = erase_pending_and_verify(store, slot, scratch.readback) {
            return Ok(ResumeOutcome::CommittedWithCleanupFailure(error));
        }
    }
    Ok(ResumeOutcome::Committed)
}

/// Erases staged work only while ordinary control is proven blank.  It never
/// removes stale pending data after a valid control record exists.
pub fn abandon_first_write<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
    expected_node: NodeId,
    facts: &BoardRecoveryFacts,
) -> Result<AbandonOutcome, FirstWriteStorageError<S::Error>> {
    read_all(store, scratch)?;
    let status = first_write_status(
        scratch.control_a,
        scratch.control_b,
        scratch.pending_a,
        scratch.pending_b,
        expected_node,
        facts,
    );
    if status.ordinary_service_eligible() {
        return Ok(AbandonOutcome::NothingStaged);
    }
    if !status.abandon_eligible() {
        return Err(FirstWriteStorageError::Ineligible(status));
    }
    for slot in [Slot::A, Slot::B] {
        erase_pending_and_verify(store, slot, scratch.readback)?;
    }
    Ok(AbandonOutcome::Abandoned)
}

fn read_all<S: FirstWriteStore>(
    store: &mut S,
    scratch: &mut FirstWriteScratch<'_>,
) -> Result<(), FirstWriteStorageError<S::Error>> {
    store
        .read_control(Slot::A, scratch.control_a)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadControl(Slot::A),
            error,
        })?;
    store
        .read_control(Slot::B, scratch.control_b)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadControl(Slot::B),
            error,
        })?;
    store
        .read_pending(Slot::A, scratch.pending_a)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadPending(Slot::A),
            error,
        })?;
    store
        .read_pending(Slot::B, scratch.pending_b)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ReadPending(Slot::B),
            error,
        })?;
    Ok(())
}

fn select_written<'a>(
    slot: Slot,
    written: &'a [u8],
    a: &'a [u8],
    b: &'a [u8],
) -> (&'a [u8], &'a [u8]) {
    match slot {
        Slot::A => (written, b),
        Slot::B => (a, written),
    }
}

fn erase_pending_and_verify<S: FirstWriteStore>(
    store: &mut S,
    slot: Slot,
    readback: &mut [u8],
) -> Result<(), FirstWriteStorageError<S::Error>> {
    store
        .erase_pending(slot)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::ErasePending(slot),
            error,
        })?;
    store
        .read_pending(slot, readback)
        .map_err(|error| FirstWriteStorageError::Store {
            operation: FirstWriteIo::VerifyPending(slot),
            error,
        })?;
    if !readback.iter().all(|byte| *byte == 0xff) {
        return Err(FirstWriteStorageError::ReadbackMismatch {
            pending: true,
            slot,
        });
    }
    Ok(())
}
