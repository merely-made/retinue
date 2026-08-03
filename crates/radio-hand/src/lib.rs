#![no_std]
#![forbid(unsafe_code)]

//! Board-side radio services shared by the direct-PHY firmware images.
//!
//! `radio-face` renders the status surface; `radio-hand` works the radio. The
//! split is the same one the crate names imply: face shows, hand does.
//!
//! The crate holds what every board does identically: [`store`] and [`settings`], the
//! persisted record that gives a board an identity surviving power loss; [`service`] and
//! [`dispatch`], the radio work and command loop that used to be duplicated in two `main.rs`
//! files; and [`executive`] and [`channel`], the hardware owner and the personality trait
//! above it. See `design_docs/2026-07-31_retinue_small_plan.md`.
//!
//! Everything here is board-agnostic. Flash and entropy peripherals stay in the firmware
//! crates, because the T114 reaches them through `embassy-nrf` and the V4 through `esp-hal`;
//! only the byte formats and the decisions over them are portable.

pub mod board_status;
#[cfg(feature = "radio")]
pub mod channel;
#[cfg(feature = "radio")]
pub mod dispatch;
#[cfg(feature = "radio")]
pub mod executive;
pub mod link;
pub mod phy;
#[cfg(feature = "radio")]
pub mod service;
pub mod settings;
pub mod store;
