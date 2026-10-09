//! LXMF propagation nodes: announce, submission, fetch, and a bounded store.
//!
//! Propagation-node peering is a separate wire protocol and is not implemented.

mod client;
mod error;
mod msgpack;
mod node;
mod paper;
mod store;
#[cfg(test)]
mod tests;
mod wire;

pub use client::{
    FetchedPropagation, PreparedPropagation, PropagationFetchReceipt, PropagationSubmitReceipt,
    fetch, fetch_with_resource_config, prepare_propagation, submit, submit_with_resource_config,
};
pub use error::PropagationError;
pub use node::{
    ReceivedPropagationBatch, ServedFetch, announce_propagation, propagation_destination,
    propagation_name, receive_submission, register_propagation, serve_fetch,
};
pub use paper::{PAPER_MDU, PreparedPaper, URI_SCHEMA, prepare_paper};
pub use store::{PropagationStore, PropagationStoreLimits, StoreReceipt, StoreRestoreReceipt};
pub use wire::{
    PropagationAnnounce, PropagationBatch, PropagationCosts, PropagationEntry, PropagationMessage,
};

pub const DEFAULT_MAX_PROPAGATION_ANNOUNCE_BYTES: usize = 4 * 1024;
pub const DEFAULT_MAX_PROPAGATION_BATCH_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_PROPAGATION_ENTRIES: usize = 4_096;
pub const MIN_ENCRYPTED_MESSAGE_BYTES: usize = 96;
pub const PROPAGATION_METADATA_NAME: u64 = 1;
pub const FETCH_LIMIT: u64 = 1_000;
pub const FETCH_PATH_HASH: [u8; 16] = [
    0x9d, 0xc1, 0xa7, 0x28, 0x83, 0x46, 0x8f, 0x57, 0xfe, 0xd5, 0x71, 0xe7, 0x96, 0xe9, 0xce, 0x98,
];
pub const DEFAULT_MAX_STORED_MESSAGE_BYTES: usize = 240;
pub const DEFAULT_MAX_PROPAGATION_STORE_SNAPSHOT_BYTES: usize = 16 * 1024 * 1024;
