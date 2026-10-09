//! Tokio serial transport for an RNode-backed [`crate::link::RadioLink`].
//!
//! This is the real-I/O edge around Tulle's sans-I/O radio state machine. It owns the
//! serial port, initialization retries, transmit pacing, and the clock used by the shared
//! airtime budget. Protocol crates see complete frames through [`RNodeSerialLink`]; they do
//! not depend on RNode's KISS framing or serial details.

mod config;
mod error;
mod link;
mod pump;
#[cfg(test)]
mod tests;

pub use config::{PumpStatus, SerialPumpConfig};
pub use error::{PumpError, TransmitError};
pub use link::RNodeSerialLink;
