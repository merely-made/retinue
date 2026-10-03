//! Bounded argument bodies for the mutable WN0 operations a carrier serves.
//!
//! The outer command authenticates the controller and the inner `Request` names the
//! transaction, sequence, expected generation, and operation. These are the exact bodies
//! carried in `Request::arguments` for the three configuration lifecycle operations this
//! slice implements. The change id ties one apply, its commit, and its revert together; it
//! is chosen by the controller, never by the board. The board mints only the commit token.

use core::fmt;

use super::{
    CHANGE_ID_LEN, COMMIT_TOKEN_LEN, ChangeId, ConfigGeneration, PUBLIC_CONFIGURATION_V1_LEN,
    PublicConfigurationError, PublicConfigurationV1,
};

/// Exact bytes of a `ProvisionalApply` argument body.
pub const PROVISIONAL_APPLY_ARGUMENTS_LEN: usize = CHANGE_ID_LEN + PUBLIC_CONFIGURATION_V1_LEN + 8;
/// Exact bytes of a `Commit` argument body.
pub const COMMIT_ARGUMENTS_LEN: usize = CHANGE_ID_LEN + 8 + COMMIT_TOKEN_LEN;
/// Exact bytes of a `Revert` argument body.
pub const REVERT_ARGUMENTS_LEN: usize = CHANGE_ID_LEN;

/// Fail-closed errors from the argument codecs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentsError {
    Length { found: usize },
    Configuration(PublicConfigurationError),
}

/// Stage one public configuration as the provisional candidate and apply it.
///
/// `lifetime_ms` is how long the candidate may stay unconfirmed, measured by the board from
/// the moment it applies. The board bounds it; an out-of-range lifetime is refused as invalid
/// arguments after the outer counter is journaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionalApplyArguments {
    pub change: ChangeId,
    pub public: PublicConfigurationV1,
    pub lifetime_ms: u64,
}

impl ProvisionalApplyArguments {
    pub fn encode(self) -> [u8; PROVISIONAL_APPLY_ARGUMENTS_LEN] {
        let mut out = [0_u8; PROVISIONAL_APPLY_ARGUMENTS_LEN];
        out[..CHANGE_ID_LEN].copy_from_slice(&self.change.0);
        out[CHANGE_ID_LEN..CHANGE_ID_LEN + PUBLIC_CONFIGURATION_V1_LEN]
            .copy_from_slice(&self.public.encode());
        out[CHANGE_ID_LEN + PUBLIC_CONFIGURATION_V1_LEN..]
            .copy_from_slice(&self.lifetime_ms.to_le_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ArgumentsError> {
        if bytes.len() != PROVISIONAL_APPLY_ARGUMENTS_LEN {
            return Err(ArgumentsError::Length { found: bytes.len() });
        }
        let mut change = [0_u8; CHANGE_ID_LEN];
        change.copy_from_slice(&bytes[..CHANGE_ID_LEN]);
        let public = PublicConfigurationV1::decode(
            &bytes[CHANGE_ID_LEN..CHANGE_ID_LEN + PUBLIC_CONFIGURATION_V1_LEN],
        )
        .map_err(ArgumentsError::Configuration)?;
        let mut lifetime = [0_u8; 8];
        lifetime.copy_from_slice(&bytes[CHANGE_ID_LEN + PUBLIC_CONFIGURATION_V1_LEN..]);
        Ok(Self {
            change: ChangeId(change),
            public,
            lifetime_ms: u64::from_le_bytes(lifetime),
        })
    }
}

/// Confirm the exact armed candidate: the same change, the generation the board allocated
/// for it, and the token the board minted when it applied.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CommitArguments {
    pub change: ChangeId,
    pub candidate_generation: ConfigGeneration,
    pub commit_token: [u8; COMMIT_TOKEN_LEN],
}

impl fmt::Debug for CommitArguments {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CommitArguments")
            .field("change", &self.change)
            .field("candidate_generation", &self.candidate_generation)
            .field("commit_token", &"[redacted]")
            .finish()
    }
}

impl CommitArguments {
    pub fn encode(self) -> [u8; COMMIT_ARGUMENTS_LEN] {
        let mut out = [0_u8; COMMIT_ARGUMENTS_LEN];
        out[..CHANGE_ID_LEN].copy_from_slice(&self.change.0);
        out[CHANGE_ID_LEN..CHANGE_ID_LEN + 8]
            .copy_from_slice(&self.candidate_generation.0.to_le_bytes());
        out[CHANGE_ID_LEN + 8..].copy_from_slice(&self.commit_token);
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ArgumentsError> {
        if bytes.len() != COMMIT_ARGUMENTS_LEN {
            return Err(ArgumentsError::Length { found: bytes.len() });
        }
        let mut change = [0_u8; CHANGE_ID_LEN];
        change.copy_from_slice(&bytes[..CHANGE_ID_LEN]);
        let mut generation = [0_u8; 8];
        generation.copy_from_slice(&bytes[CHANGE_ID_LEN..CHANGE_ID_LEN + 8]);
        let mut commit_token = [0_u8; COMMIT_TOKEN_LEN];
        commit_token.copy_from_slice(&bytes[CHANGE_ID_LEN + 8..]);
        Ok(Self {
            change: ChangeId(change),
            candidate_generation: ConfigGeneration(u64::from_le_bytes(generation)),
            commit_token,
        })
    }
}

/// Abandon the armed candidate named by its change id and restore known-good now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevertArguments {
    pub change: ChangeId,
}

impl RevertArguments {
    pub fn encode(self) -> [u8; REVERT_ARGUMENTS_LEN] {
        self.change.0
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ArgumentsError> {
        if bytes.len() != REVERT_ARGUMENTS_LEN {
            return Err(ArgumentsError::Length { found: bytes.len() });
        }
        let mut change = [0_u8; CHANGE_ID_LEN];
        change.copy_from_slice(bytes);
        Ok(Self {
            change: ChangeId(change),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ManagementCarrierSet, ReticulumTransportPolicy};
    use crate::region::Region;

    fn public() -> PublicConfigurationV1 {
        PublicConfigurationV1::new(
            Region::Us915,
            selvage::PhyProfile::meshtastic_long_fast(906_875_000),
            ReticulumTransportPolicy::new(false, false, 0).unwrap(),
            ManagementCarrierSet::from_mask(1).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn lifecycle_arguments_round_trip_exactly() {
        let apply = ProvisionalApplyArguments {
            change: ChangeId([0x21; CHANGE_ID_LEN]),
            public: public(),
            lifetime_ms: 60_000,
        };
        let bytes = apply.encode();
        assert_eq!(bytes.len(), 45);
        assert_eq!(ProvisionalApplyArguments::decode(&bytes), Ok(apply));
        assert!(matches!(
            ProvisionalApplyArguments::decode(&bytes[..44]),
            Err(ArgumentsError::Length { found: 44 })
        ));
        let mut bad = bytes;
        bad[CHANGE_ID_LEN] = 0xEE;
        assert!(matches!(
            ProvisionalApplyArguments::decode(&bad),
            Err(ArgumentsError::Configuration(_))
        ));

        let commit = CommitArguments {
            change: ChangeId([0x21; CHANGE_ID_LEN]),
            candidate_generation: ConfigGeneration(8),
            commit_token: [0xA5; COMMIT_TOKEN_LEN],
        };
        let bytes = commit.encode();
        assert_eq!(bytes.len(), 40);
        assert_eq!(CommitArguments::decode(&bytes), Ok(commit));
        let mut rendered = heapless::String::<512>::new();
        core::fmt::Write::write_fmt(&mut rendered, format_args!("{commit:?}")).unwrap();
        assert!(rendered.contains("redacted"));
        assert!(!rendered.contains("A5"));

        let revert = RevertArguments {
            change: ChangeId([0x21; CHANGE_ID_LEN]),
        };
        assert_eq!(RevertArguments::decode(&revert.encode()), Ok(revert));
        assert!(matches!(
            RevertArguments::decode(&[0; 3]),
            Err(ArgumentsError::Length { found: 3 })
        ));
    }
}
