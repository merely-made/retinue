//! Radio-free coordination for deliberate personality excursions.
//!
//! A decision model, not a radio owner or protocol adapter. Callers own adapter
//! state, query adapters for readiness without changing it, and report explicit
//! hardware completion. `radio-hand` owns hardware, keeper scheduling and recovery.
//! Caller-supplied coverage proves neither RF reception nor authentication.
//!
//! Configuration is immutable. A pin must equal home: it means a dedicated
//! selected home, rather than silently restoring a different configured
//! personality. All timestamps are caller-monotonic milliseconds; construction
//! assumes the caller's hardware owner has already confirmed home.

mod controller;
mod steps;
mod types;

pub use controller::Controller;
pub use types::{
    Acknowledgement, ConfigError, ControllerConfig, ControllerError, ControllerEvent,
    ControllerState, CoverageEvidence, CoveragePolicy, Excursion, InstalledPersonalitySet,
    InterruptionPolicy, MAX_INSTALLED_PERSONALITIES, PauseOutcome, PersonalityId, ReturnReason,
    StopCapability, Transition,
};
