//! Retained, bounded Sennet text-leaf state for an embedded protocol instance.
//!
//! This module owns protocol state only. The board owns radio activation,
//! persistence, physical transmit completion, and any activation-tagged action
//! queue. Packet IDs arrive in a caller-reserved exclusive interval because a
//! reset must never reuse an AES-CTR nonce identity.

mod error;
mod lease;
mod state;
#[cfg(test)]
mod tests;
mod types;

pub use error::InstanceError;
pub use lease::{LeaseError, PacketIdLease, PacketIdReservation, ReservationProof};
pub use state::SennetInstance;
pub use types::{
    InstanceEvent, LossPermission, OutboundText, PauseOutcome, ReceiveOutcome, SennetInstanceConfig,
};
