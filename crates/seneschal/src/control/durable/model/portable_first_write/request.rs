//! Claim challenge, proof transcript, and exact request bytes.

use ed25519_dalek::{Signature, VerifyingKey};

use super::super::{NodeId, OWNER_CLAIM_LEN, OwnerClaim};
use super::{
    CLAIM_DOMAIN, CLAIM_PROOF_LEN, CLAIM_REQUEST_LEN, FIRST_OWNER_VERSION, NODE_ID_LEN, NONCE_LEN,
    REQUEST_ABANDON, REQUEST_CLAIM, REQUEST_INSPECT, REQUEST_RESUME, SIGNATURE_LEN,
};

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
// Requests have a fixed wire bound and stay inline on allocator-free board paths.
#[allow(clippy::large_enum_variant)]
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
