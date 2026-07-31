#![no_std]
#![forbid(unsafe_code)]

//! Board-side radio services shared by the direct-PHY firmware images.
//!
//! `radio-face` renders the status surface; `radio-hand` works the radio. The
//! split is the same one the crate names imply: face shows, hand does.
//!
//! Today the crate holds [`store`], the persistence record format that gives a
//! board a device identity that survives power loss. The shared radio service
//! (config apply, TX, RX, diagnostics, airtime, queue policy) moves here out of
//! the two firmware `main.rs` files at gate N2. See
//! `design_docs/2026-07-31_retinue_small_plan.md`.
//!
//! Everything here is allocation-free and board-agnostic. Flash and entropy
//! peripherals stay in the firmware crates, because the T114 reaches them
//! through `embassy-nrf` and the V4 through `esp-hal`; only the byte formats and
//! the decisions over them are portable.

pub mod store;
