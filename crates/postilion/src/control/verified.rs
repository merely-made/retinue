//! Normal-runtime, controller-authenticated control over a local byte-stream carrier.
//!
//! This is the host half of the V4's signed control carrier. The controller signs one WN0
//! request with `retinue::command`, the carrier frames it for the board's ordinary modem
//! stream, and the board answers only after its verifier has accepted the envelope and its
//! runtime has journaled the accepted counter. The response is not signed by the board:
//! what this module checks is that the answer names the node and transaction it asked
//! about and carries `VerifiedController` authority, which the board produces for nothing
//! else.
//!
//! The outer counter is the controller's to remember. The board refuses a counter at or
//! below its last accepted value and one more than `COUNTER_WINDOW` ahead, so the
//! application persists what it last used; Postilion never stores it.

mod client;
#[cfg(test)]
mod tests;
mod usb;

pub use client::{
    AppliedReceipt, ControlClient, ControlClientError, ControlExchange, Mutation,
    ProvisionalReceipt, VerifiedStatus,
};
pub use usb::{UsbControlConfig, UsbControlError, UsbControlTransport};
