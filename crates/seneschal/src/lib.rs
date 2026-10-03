#![no_std]
#![forbid(unsafe_code)]

//! The board-management contract for Retinue wall nodes: what a controller may ask of a board,
//! and what the board keeps durable about who may ask.
//!
//! - [`control`]: the WN0 request grammar and replies, the WN1 durable configuration journal
//!   (owner grants, recovery policy, provisional apply and commit), first-owner claiming, and
//!   the public status record.
//! - [`store`]: the CRC-protected A/B record format the journal persists through.
//! - [`region`]: the regulatory regions a public configuration names.
//!
//! Everything here is radio-free, allocation-free, and board-independent. The firmware's
//! async runtime, flash, and radio stay in the board crates; this is the shared vocabulary
//! between a board and the hosts that manage it.
//!
//! The `retinue` feature adds the bridge from Retinue's verified command envelope
//! (`retinue::command::VerifiedCommand`) into control authority, and the local-carrier frame
//! tags. It links only Retinue's allocation-free floor.
//!
//! The crate began as `radio-hand`'s control modules. Its wire anchors (record magics and
//! domain separators) keep their original bytes, because boards in the field hold them.

#[cfg(test)]
extern crate std;

pub mod control;
pub mod region;
pub mod store;
