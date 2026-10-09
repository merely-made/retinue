//! Inspect status, action eligibility, and exact response bytes.

use super::super::NodeId;
use super::{
    FIRST_OWNER_VERSION, FirstOwnerWireError, INSPECT_RESPONSE_LEN, NODE_ID_LEN, NONCE_LEN,
    REQUEST_ABANDON, REQUEST_CLAIM, REQUEST_INSPECT, REQUEST_RESUME, RESPONSE_BIT,
};

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
