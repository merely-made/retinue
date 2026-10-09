#![no_std]
#![forbid(unsafe_code)]

//! Exact verification for the pinned tinySSB v0 signed-feed wire format.
//!
//! This crate verifies a single next main entry for caller-owned feed state and
//! advances a caller-owned side-chain cursor. It deliberately has no radio,
//! retry, persistence, clock, entropy, feed-table, or allocator dependency.
//! The wire rules are independently implemented from tinySSB's MIT ESP32 core
//! at revision `39896b72c97b51159d46610c5f11ff7f5a279031`; see `NOTICE` and
//! `fixtures/manifest.toml`.

#[cfg(test)]
mod tests;
mod types;
mod verify;

pub use types::{
    ChunkCursor, ChunkFrame, ChunkProgress, EntryContent, FeedId, Frontier, MainFrame, MessageId,
    Refusal, SidechainRequirement, VerifiedEntry,
};
pub use verify::{expected_dmx, verify_chunk, verify_next};

/// The domain prepended to every tinySSB v0 main-entry hash and signature.
pub const DOMAIN: &[u8; 10] = b"tinyssb-v0";
/// Bytes in every tinySSB main entry and side-chain chunk.
pub const FRAME_LEN: usize = 120;
/// Bytes in an Ed25519 feed public key.
pub const FEED_ID_LEN: usize = 32;
/// Bytes in the truncated SHA-256 message and chunk identifiers.
pub const MESSAGE_ID_LEN: usize = 20;
/// Bytes in a derived DMX header.
pub const DMX_LEN: usize = 7;
/// Bytes of the signed main-entry prefix before its signature.
pub const SIGNED_MAIN_LEN: usize = 56;
/// Bytes of plain inline content.
pub const PLAIN_CONTENT_LEN: usize = 48;
/// Bytes in a side-chain chunk before its successor hash.
pub const CHUNK_CONTENT_LEN: usize = 100;

const TYPE_OFFSET: usize = DMX_LEN;
const CONTENT_OFFSET: usize = TYPE_OFFSET + 1;
const SIGNATURE_OFFSET: usize = SIGNED_MAIN_LEN;
const SIDECHAIN_POINTER_OFFSET: usize = CONTENT_OFFSET + PLAIN_CONTENT_LEN - MESSAGE_ID_LEN;
const SIGNED_INPUT_LEN: usize = DOMAIN.len() + FEED_ID_LEN + 4 + MESSAGE_ID_LEN + SIGNED_MAIN_LEN;
