//! The LXMF message codec, written without `std` and without a MessagePack value tree.
//!
//! [`crate::codec`] reads the payload into an `rmpv::Value`, and `rmpv` has no `no_std`
//! mode. Nothing in the crate reads inside `fields`, so here `fields` is the **original
//! bytes**, sliced out and spliced back verbatim: more faithful than re-encoding, since a map
//! that arrived in a wider encoding than it needed goes back out the way it came.
//!
//! Built beside the shipping codec rather than replacing it, because that codec is
//! byte-exact against a stock LXMF oracle. The tests hold this one to the same bar: the
//! oracle capture's bytes, message id and signing preimage, and agreement with
//! [`crate::codec`] on what it produces. The swap waits only on a board that wants it.
//!
//! ```text
//! cargo build -p outrider --no-default-features --target thumbv7em-none-eabihf
//! ```
//!
//! builds this module and [`crate::stamp`] and nothing else, with `sha2` held level with
//! retinue's so a board links one SHA-256.

mod message;
mod msgpack;
#[cfg(test)]
mod tests;

pub use message::{
    CodecError, DESTINATION_LEN, Decoded, HEADER_LEN, Payload, SIGNATURE_LEN, SOURCE_LEN, decode,
    encode_payload, message_id, signing_bytes,
};
