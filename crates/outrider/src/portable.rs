//! The LXMF message codec, written without `std` and without a MessagePack value tree.
//!
//! `rmpv` has no `no_std` mode, so here `fields` is the **original bytes**, sliced out and
//! spliced back verbatim: more faithful than re-encoding, since a map that arrived in a wider
//! encoding than it needed goes back out the way it came.
//!
//! [`crate::codec`] parses with this module and reads only the field map into an
//! `rmpv::Value`. The tests hold both to a stock LXMF oracle: the capture's bytes, message id and signing
//! preimage, and agreement with each other on what they produce.
//!
//! ```text
//! cargo build -p outrider --no-default-features --target thumbv7em-none-eabihf
//! ```
//!
//! builds this module, [`crate::stamp`] and [`crate::fields`] and nothing else, with `sha2` held level with
//! retinue's so a board links one SHA-256.

mod message;
mod msgpack;
#[cfg(test)]
mod tests;

pub use message::{
    CodecError, DESTINATION_LEN, Decoded, HEADER_LEN, Payload, SIGNATURE_LEN, SOURCE_LEN, StrParts,
    decode, encode_payload, message_id, signing_bytes,
};
#[cfg(feature = "std")]
pub(crate) use message::{hashed_identity, parse};
#[cfg(feature = "std")]
pub(crate) use msgpack::{write_bin, write_f64, write_text};
