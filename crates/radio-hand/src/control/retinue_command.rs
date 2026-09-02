//! The WN1 bridge from Retinue's verified command envelope into control state.
//!
//! A carrier may hand this module only a [`retinue::command::VerifiedCommand`]. The outer
//! verifier has already checked its target, allowlist, counter window, and signature. This
//! bridge narrows that authority to one node-addressed WN0 request and does not decide grants,
//! semantic tags, or carriage confidentiality.

use core::fmt;

use retinue::{
    command::{TargetClass, VerifiedCommand, Verifier},
    hash::AddressHash,
    identity::Identity,
};

use super::{
    COMMAND_OPCODE, ControllerId, ControllerRole, DecodeError, DurableState, NodeId, OwnerGrant,
    Request, VerifiedController, decode_request,
};

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
