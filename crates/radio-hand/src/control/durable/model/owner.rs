use core::fmt;

use sha2::{Digest, Sha256};

use super::super::super::{ControllerId, ControllerRole, RETINUE_PUBLIC_IDENTITY_LEN};

/// A durable controller grant. The Retinue identity is retained verbatim so firmware can
/// reconstruct the outer verifier before accepting network traffic after reboot.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct OwnerGrant {
    pub(super) controller: ControllerId,
    pub(super) retinue_public_identity: [u8; RETINUE_PUBLIC_IDENTITY_LEN],
    pub(super) role: ControllerRole,
    pub(super) accepted_outer_counter: u64,
    pub(super) accepted_mutation_sequence: u64,
}

impl OwnerGrant {
    /// Creates a fresh durable grant from canonical Retinue public identity bytes. The
    /// controller identifier is always `trunc16(SHA-256(public_identity))` and both replay
    /// counters begin at zero.
    pub fn from_public_identity(
        retinue_public_identity: [u8; RETINUE_PUBLIC_IDENTITY_LEN],
        role: ControllerRole,
    ) -> Self {
        let digest = Sha256::digest(retinue_public_identity);
        let mut controller = [0; 16];
        controller.copy_from_slice(&digest[..16]);
        Self::from_durable_parts(
            ControllerId(controller),
            retinue_public_identity,
            role,
            0,
            0,
        )
    }

    /// Decoded durable fields. Only the codec and in-crate invariant tests may bypass the
    /// identity-derived constructor; durable validation rechecks the binding before use.
    pub(crate) const fn from_durable_parts(
        controller: ControllerId,
        retinue_public_identity: [u8; RETINUE_PUBLIC_IDENTITY_LEN],
        role: ControllerRole,
        accepted_outer_counter: u64,
        accepted_mutation_sequence: u64,
    ) -> Self {
        Self {
            controller,
            retinue_public_identity,
            role,
            accepted_outer_counter,
            accepted_mutation_sequence,
        }
    }

    pub const fn controller(&self) -> ControllerId {
        self.controller
    }
    pub const fn role(&self) -> ControllerRole {
        self.role
    }
    pub const fn retinue_public_identity(&self) -> &[u8; RETINUE_PUBLIC_IDENTITY_LEN] {
        &self.retinue_public_identity
    }
    pub const fn accepted_outer_counter(&self) -> u64 {
        self.accepted_outer_counter
    }
    pub const fn accepted_mutation_sequence(&self) -> u64 {
        self.accepted_mutation_sequence
    }
}

impl fmt::Debug for OwnerGrant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OwnerGrant")
            .field("controller", &self.controller)
            .field("retinue_public_identity", &"[redacted]")
            .field("role", &self.role)
            .field("accepted_outer_counter", &self.accepted_outer_counter)
            .field(
                "accepted_mutation_sequence",
                &self.accepted_mutation_sequence,
            )
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifiedCounterError {
    UnknownController,
    NotMonotonic,
}
