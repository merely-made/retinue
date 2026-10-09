//! Host-driven, exclusive direct-PHY runtime for [`crate::personality::Controller`].
//!
//! This keeps one serial pump open and owns its monotonic epoch. Protocol callers
//! retain all protocol/session state and report side-effect-free readiness to the
//! controller. The runtime acknowledges a transition only after the firmware's
//! profile acknowledgement. It does not implement autonomous board switching.
//!
//! A direct-PHY profile request already taken by the pump cannot be cancelled.
//! An outer deadline or serial failure therefore makes the physical profile
//! uncertain. The runtime retires that session, latches recovery, and rejects
//! every later send, receive, or transition instead of restarting a stale pump.

mod error;
mod runtime;
#[cfg(test)]
mod tests;

pub use error::PersonalitySerialError;
pub use runtime::{PersonalityProfile, PersonalitySerialRuntime, TransitionApplied};
