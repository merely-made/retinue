//! Controller and literal USB carrier for a wall node's first owner.
//!
//! This sits below a Signalman face and above serial/KISS. A carrier exchanges
//! already-built portable requests; it never receives a private identity or
//! decides what a prospective owner should configure.

mod controller;
mod plan;
#[cfg(test)]
mod tests;
mod usb;

pub use controller::{
    ClaimOutcome, FirstOwnerController, FirstOwnerError, FirstOwnerExchange, Inspection,
    ResumeOutcome,
};
pub use plan::{ClaimPlan, V4UsbPlanError, v4_usb_claim_plan};
pub use usb::{UsbFirstOwnerConfig, UsbFirstOwnerError, UsbFirstOwnerTransport};
