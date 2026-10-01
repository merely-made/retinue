#![forbid(unsafe_code)]

//! Browser realization of `radio-face`.
//!
//! - [`framebuffer`]: any `embedded-graphics` color into RGBA bytes; no
//!   radio-face or wasm types, so it can split out as its own crate.
//! - [`mirror`]: a simulated radio whose pages and buttons are the firmware's
//!   own `render` and `Controller`.
//! - `input` (feature `json`): JSON documents for the status types.
//! - `web` (wasm32 only): `wasm-bindgen` exports over the above.

extern crate alloc;

pub mod framebuffer;
#[cfg(feature = "json")]
pub mod input;
pub mod mirror;
pub mod names;
#[cfg(target_arch = "wasm32")]
mod web;

pub use framebuffer::RgbaFramebuffer;
#[cfg(feature = "png")]
pub use mirror::render_png;
pub use mirror::{Mirror, mono_theme, receipt_theme, render_rgba, theme_from_rgb};
