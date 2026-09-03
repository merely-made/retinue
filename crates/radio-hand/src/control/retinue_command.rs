//! The WN1 bridge from Retinue's verified command envelope into control state.
//!
//! A carrier may hand this module only a [`retinue::command::VerifiedCommand`]. The outer
//! verifier has already checked its target, allowlist, counter window, and signature. This
//! bridge narrows that authority to one node-addressed WN0 request and does not decide grants,
//! semantic tags, or carriage confidentiality.

use core::fmt;

use retinue::{
    command::{HEADER_LEN, MAX_COMMAND_LEN, TargetClass, VerifiedCommand, Verifier},
    hash::AddressHash,
    identity::{Identity, SIGNATURE_LEN},
};

use super::{
    COMMAND_OPCODE, ControllerId, ControllerRole, DecodeError, DurableState, EncodeError,
    MAX_OWNER_GRANTS, MAX_RESPONSE_LEN, NodeId, OwnerGrant, Request, Response, VerifiedController,
    decode_request, decode_response, encode_response,
};

/// The node-only outer verifier sized to the durable grant table, so a board names the
/// verifier without depending on Retinue directly.
pub type ControlVerifier = Verifier<MAX_OWNER_GRANTS>;

/// Tag at the start of a KISS frame that carries one signed outer command on a local
/// byte-stream carrier. The frame body after the tag is the exact `retinue::command` wire.
///
/// KISS framing itself belongs to `selvage`; the tag keeps a signed command distinct from the
/// unauthenticated status diagnostic and from ordinary direct-PHY traffic sharing the stream.
pub const CONTROL_COMMAND_FRAME_TAG: u8 = 0x56;
/// Tag at the start of a KISS frame that carries one WN0 response to a verified command.
///
/// The response is not signed by the board. Its authority is that the board produced it only
/// after verifying and journaling the outer command it answers.
pub const CONTROL_RESPONSE_FRAME_TAG: u8 = 0x52;
/// Smallest unescaped command frame: tag, header, and signature with an empty payload.
pub const MIN_CONTROL_COMMAND_FRAME_LEN: usize = 1 + HEADER_LEN + SIGNATURE_LEN;
/// Largest unescaped command frame a carrier must be able to reassemble.
pub const MAX_CONTROL_COMMAND_FRAME_LEN: usize = 1 + MAX_COMMAND_LEN;
/// Largest unescaped response frame a carrier must be able to reassemble.
pub const MAX_CONTROL_RESPONSE_FRAME_LEN: usize = 1 + MAX_RESPONSE_LEN;

/// Fail-closed errors from the local-carrier frame tagging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlFrameError {
    Length { found: usize },
    UnexpectedFrameTag(u8),
    Encode(EncodeError),
    Decode(DecodeError),
}

/// Tags one already-signed outer command for a local carrier. `out` receives the unescaped
/// frame body; the carrier KISS-escapes it afterwards.
pub fn encode_command_frame(command: &[u8], out: &mut [u8]) -> Result<usize, ControlFrameError> {
    let length = 1 + command.len();
    if command.len() + 1 < MIN_CONTROL_COMMAND_FRAME_LEN
        || length > MAX_CONTROL_COMMAND_FRAME_LEN
        || out.len() < length
    {
        return Err(ControlFrameError::Length { found: length });
    }
    out[0] = CONTROL_COMMAND_FRAME_TAG;
    out[1..length].copy_from_slice(command);
    Ok(length)
}

/// Strips the tag from one unescaped command frame and returns the exact signed wire bytes.
///
/// This checks only tag and shape. Target, allowlist, counter window, and signature remain
/// the verifier's, and nothing here creates authority.
pub fn decode_command_frame(frame: &[u8]) -> Result<&[u8], ControlFrameError> {
    if frame.len() < MIN_CONTROL_COMMAND_FRAME_LEN || frame.len() > MAX_CONTROL_COMMAND_FRAME_LEN {
        return Err(ControlFrameError::Length { found: frame.len() });
    }
    if frame[0] != CONTROL_COMMAND_FRAME_TAG {
        return Err(ControlFrameError::UnexpectedFrameTag(frame[0]));
    }
    Ok(&frame[1..])
}

/// Tags one WN0 response for a local carrier.
pub fn encode_response_frame(
    response: &Response,
    out: &mut [u8],
) -> Result<usize, ControlFrameError> {
    if out.len() < MAX_CONTROL_RESPONSE_FRAME_LEN {
        return Err(ControlFrameError::Length { found: out.len() });
    }
    out[0] = CONTROL_RESPONSE_FRAME_TAG;
    let length = encode_response(response, &mut out[1..]).map_err(ControlFrameError::Encode)?;
    Ok(1 + length)
}

/// Decodes one unescaped, tagged response frame.
pub fn decode_response_frame(frame: &[u8]) -> Result<Response, ControlFrameError> {
    if frame.len() < 2 || frame.len() > MAX_CONTROL_RESPONSE_FRAME_LEN {
        return Err(ControlFrameError::Length { found: frame.len() });
    }
    if frame[0] != CONTROL_RESPONSE_FRAME_TAG {
        return Err(ControlFrameError::UnexpectedFrameTag(frame[0]));
    }
    decode_response(&frame[1..]).map_err(ControlFrameError::Decode)
}

impl OwnerGrant {
    /// Converts Retinue's canonical public identity to the durable grant form. Only firmware
    /// setup should call this; unverified carrier bytes never become an `Identity` here.
    pub fn from_retinue_identity(identity: &Identity, role: ControllerRole) -> Self {
        Self::from_public_identity(identity.to_public_bytes(), role)
    }
}

/// Why durable grants could not safely reconstruct Retinue's outer verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifierRestoreError {
    InvalidIdentity,
    ControllerMismatch,
    DuplicateController,
    MissingOwner,
    Capacity,
}

/// [`restore_verifier`] at the durable grant bound.
pub fn restore_control_verifier(
    state: &DurableState,
) -> Result<ControlVerifier, VerifierRestoreError> {
    restore_verifier(state)
}

/// Rebuilds the node-only Retinue verifier from validated durable grants. Any malformed public
/// identity, mismatched hash, duplicate, missing owner, or capacity mismatch is fail-closed.
pub fn restore_verifier<const N: usize>(
    state: &DurableState,
) -> Result<Verifier<N>, VerifierRestoreError> {
    let mut verifier = Verifier::new(AddressHash::from_bytes(state.node().0));
    let mut has_owner = false;
    for (index, grant) in state.owner_grants().iter().enumerate() {
        let identity = Identity::from_public_bytes(grant.retinue_public_identity())
            .map_err(|_| VerifierRestoreError::InvalidIdentity)?;
        if ControllerId(*identity.hash().as_bytes()) != grant.controller() {
            return Err(VerifierRestoreError::ControllerMismatch);
        }
        if state.owner_grants()[..index]
            .iter()
            .any(|prior| prior.controller() == grant.controller())
        {
            return Err(VerifierRestoreError::DuplicateController);
        }
        has_owner |= grant.role() == ControllerRole::Owner;
        verifier
            .authorize(identity)
            .map_err(|_| VerifierRestoreError::Capacity)?;
        if !verifier.restore(identity.hash(), grant.accepted_outer_counter()) {
            return Err(VerifierRestoreError::ControllerMismatch);
        }
    }
    if !has_owner {
        return Err(VerifierRestoreError::MissingOwner);
    }
    Ok(verifier)
}

/// A verified outer command that is suitable for the board control state machine.
///
/// Its fields are private so callers cannot turn arbitrary decoded payloads into authority.
/// [`decode_verified_command`] is its only constructor, and requires Retinue's verification
/// witness.
pub struct InboundControl {
    node: NodeId,
    controller: VerifiedController,
    counter: u64,
    request: Request,
}

impl fmt::Debug for InboundControl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InboundControl")
            .field("node", &self.node)
            .field("controller", &self.controller_id())
            .field("counter", &self.counter)
            .field("transaction", &self.request.transaction)
            .field("expected_generation", &self.request.expected_generation)
            .field("operation", &self.request.operation)
            .field("arguments_len", &self.request.arguments.len())
            .finish()
    }
}

impl InboundControl {
    /// The authenticated control node addressed by the outer command.
    pub const fn node(&self) -> NodeId {
        self.node
    }

    /// The authenticated controller identifier.
    pub const fn controller_id(&self) -> ControllerId {
        self.controller.0
    }

    /// The authority witness for state-machine admission and durable grants.
    pub const fn verified_controller(&self) -> VerifiedController {
        self.controller
    }

    /// The accepted outer replay counter.
    pub const fn counter(&self) -> u64 {
        self.counter
    }

    /// The bounded, decoded WN0 request.
    pub const fn request(&self) -> &Request {
        &self.request
    }
}

/// Why a verified Retinue command cannot enter the WN0 control state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboundControlError {
    /// Fleet authority must not be treated as authority for an individual control node.
    NonNodeTarget,
    /// The verified command carried a different application payload.
    WrongOpcode(u8),
    /// The WN0 request payload was malformed or outside its fixed bounds.
    InvalidRequest(DecodeError),
}

/// Decode one node-addressed WN0 request from Retinue's verified authority witness.
pub fn decode_verified_command(
    command: &VerifiedCommand<'_>,
) -> Result<InboundControl, InboundControlError> {
    if command.class() != TargetClass::Node {
        return Err(InboundControlError::NonNodeTarget);
    }
    if command.opcode() != COMMAND_OPCODE {
        return Err(InboundControlError::WrongOpcode(command.opcode()));
    }
    let request = decode_request(command.payload()).map_err(InboundControlError::InvalidRequest)?;
    let node = NodeId(*command.target().as_bytes());
    let controller_id = ControllerId(*command.key_id().as_bytes());
    Ok(InboundControl {
        node,
        controller: VerifiedController::from_verified_key(controller_id),
        counter: command.counter(),
        request,
    })
}
