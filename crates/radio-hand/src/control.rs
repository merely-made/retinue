//! Radio-free, carrier-neutral wall-node control contract.
//!
//! The semantic request lives inside `retinue::command::Command`; the outer command retains
//! target, signer, counter, signature, and verification authority. The grammar, replies,
//! capability facts, and durable journal are `seneschal::control`, re-exported here so board
//! code keeps one path. What stays in this crate is board-side: the async runtime that orders
//! journal writes against flash and radio, the WN0 volatile admission seam, and the position
//! disclosure table.

mod admission;
mod position_disclosure;
mod runtime;
#[cfg(test)]
mod test_authority;

pub use seneschal::control::*;

pub use admission::{Admission, RequestAdmission};
pub use position_disclosure::{
    AbsentPolicy, BlindedPositionAcl, DisclosureTier, POSITION_ACL_ENTRY_LEN,
    POSITION_ACL_HASH_LEN, POSITION_ACL_HEADER_LEN, POSITION_ACL_SECRET_LEN, POSITION_ACL_TAG_LEN,
    POSITION_ACL_V1_VERSION, PositionAclEntry, PositionAclError, PositionAclV1, Resolved,
};
pub use runtime::{
    BootState, ConfigApplier, ControlRuntime, DurableScratch, DurableScratchError, LiveOutcome,
    MAX_PROVISIONAL_LIFETIME_MS, MIN_DURABLE_SLOT_BYTES, MIN_PROVISIONAL_LIFETIME_MS,
    PreparedCommit, PreparedProvisional, QuietExit, QuietGuard, QuietWindow, RuntimeError,
};
